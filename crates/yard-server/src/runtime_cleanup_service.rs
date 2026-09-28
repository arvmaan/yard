use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tracing::{debug, info, warn};
use yard_store::{ClaimedRuntimeCleanupBatch, PendingRuntimeCleanup, ProjectStoreError, YardStore};

use crate::allocation_service::{RuntimeControl, RuntimeRetirementRequest};

pub const RUNTIME_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);
const RUNTIME_CLEANUP_BATCH_SIZE: usize = 100;
const RUNTIME_CLEANUP_CLAIM_TTL_MS: u64 = 30_000;
const TRANSPORT_RETRY_BASE_MS: u64 = 1_000;
const TRANSPORT_RETRY_CAP_MS: u64 = 5 * 60_000;
const IDENTITY_RETRY_BASE_MS: u64 = 60_000;
const IDENTITY_RETRY_CAP_MS: u64 = 60 * 60_000;
/// A repeated, unchanged failure is logged at WARN at most this often.
pub(crate) const WARN_REPEAT_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Retry state for jobs that stopped being claimed (for example because
/// another path completed them) is forgotten after this long.
const RETRY_STATE_IDLE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
/// Throttle key for failures of a whole pass rather than of one job.
const PASS_THROTTLE_KEY: &str = "\u{0}runtime-cleanup-pass";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeCleanupReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
}

#[derive(Clone)]
pub struct RuntimeCleanupService {
    runtime: Arc<dyn RuntimeControl>,
    persistence: Arc<dyn RuntimeCleanupPersistence>,
    retry_state: Arc<Mutex<RetryState>>,
}

/// In-memory retry bookkeeping shared by every cleanup service of one store.
///
/// Transport failures (Herdr unreachable) back off per job from
/// `TRANSPORT_RETRY_BASE_MS` to `TRANSPORT_RETRY_CAP_MS`. The streak is not
/// persisted: it resets when the job succeeds, whenever any retirement call
/// reaches Herdr (success or an identity answer), and on restart.
#[derive(Debug, Default)]
struct RetryState {
    jobs: HashMap<String, JobRetryState>,
    warnings: WarnThrottle,
}

#[derive(Debug, Default)]
struct JobRetryState {
    transport_failures: u32,
    last_seen: Option<Instant>,
}

impl RetryState {
    fn runtime_reachable(&mut self) {
        for job in self.jobs.values_mut() {
            job.transport_failures = 0;
        }
    }

    fn record_transport_failure(&mut self, cleanup_id: &str, now: Instant) -> u32 {
        let job = self.jobs.entry(cleanup_id.to_owned()).or_default();
        let streak = job.transport_failures;
        job.transport_failures = job.transport_failures.saturating_add(1);
        job.last_seen = Some(now);
        streak
    }

    fn forget(&mut self, cleanup_id: &str) -> Option<u32> {
        self.jobs.remove(cleanup_id);
        self.warnings.forget(cleanup_id)
    }

    fn prune(&mut self, now: Instant) {
        let idle = |seen: Option<Instant>| {
            seen.is_none_or(|seen| now.saturating_duration_since(seen) >= RETRY_STATE_IDLE_TTL)
        };
        self.jobs.retain(|_, job| !idle(job.last_seen));
        self.warnings
            .entries
            .retain(|_, entry| !idle(Some(entry.last_seen)));
    }
}

/// Retry state per store. Services that archive, delete, end sessions or
/// replace orchestrators each build a `RuntimeCleanupService` and attempt
/// their command's jobs inline, while the background runner retries the same
/// jobs. They all share this entry, so one job has one transport streak and
/// one WARN throttle however many services touch it.
static SHARED_RETRY_STATES: LazyLock<Mutex<HashMap<usize, Weak<Mutex<RetryState>>>>> =
    LazyLock::new(Mutex::default);

fn shared_retry_state(store: &Arc<dyn YardStore>) -> Arc<Mutex<RetryState>> {
    let key = Arc::as_ptr(store).cast::<()>() as usize;
    let mut states = SHARED_RETRY_STATES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A dropped store's address can be reused; its Weak no longer upgrades.
    states.retain(|_, state| state.strong_count() > 0);
    if let Some(state) = states.get(&key).and_then(Weak::upgrade) {
        return state;
    }
    let state = Arc::default();
    states.insert(key, Arc::downgrade(&state));
    state
}

/// Decides whether a failure is worth a WARN line: on the first failure, when
/// the error changes, and otherwise at most once per `WARN_REPEAT_INTERVAL`.
#[derive(Debug, Default)]
pub(crate) struct WarnThrottle {
    entries: HashMap<String, WarnEntry>,
}

#[derive(Debug)]
struct WarnEntry {
    state: String,
    last_warned_at: Instant,
    last_seen: Instant,
    suppressed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WarnDecision {
    /// Emit a WARN; `suppressed` identical failures were logged at DEBUG since
    /// the previous WARN.
    Warn {
        suppressed: u32,
    },
    Suppress,
}

impl WarnThrottle {
    pub(crate) fn observe(&mut self, key: &str, state: &str, now: Instant) -> WarnDecision {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_seen = now;
            let due = now.saturating_duration_since(entry.last_warned_at) >= WARN_REPEAT_INTERVAL;
            if entry.state == state && !due {
                entry.suppressed = entry.suppressed.saturating_add(1);
                return WarnDecision::Suppress;
            }
            let suppressed = entry.suppressed;
            state.clone_into(&mut entry.state);
            entry.last_warned_at = now;
            entry.suppressed = 0;
            return WarnDecision::Warn { suppressed };
        }
        self.entries.insert(
            key.to_owned(),
            WarnEntry {
                state: state.to_owned(),
                last_warned_at: now,
                last_seen: now,
                suppressed: 0,
            },
        );
        WarnDecision::Warn { suppressed: 0 }
    }

    /// Clear a recovered key, returning how many failures went unlogged at
    /// WARN since its last WARN.
    pub(crate) fn forget(&mut self, key: &str) -> Option<u32> {
        self.entries.remove(key).map(|entry| entry.suppressed)
    }
}

#[async_trait]
trait RuntimeCleanupPersistence: Send + Sync {
    async fn claim(
        &self,
        command_id: Option<&str>,
        limit: usize,
        claim_ttl_ms: u64,
    ) -> Result<ClaimedRuntimeCleanupBatch, ProjectStoreError>;
    async fn succeed(&self, cleanup_id: &str, claim_token: &str) -> Result<(), ProjectStoreError>;
    async fn fail(
        &self,
        cleanup_id: &str,
        claim_token: &str,
        message: &str,
        retry_after_ms: u64,
    ) -> Result<(), ProjectStoreError>;
}

struct YardStoreRuntimeCleanupPersistence {
    store: Arc<dyn YardStore>,
}

#[async_trait]
impl RuntimeCleanupPersistence for YardStoreRuntimeCleanupPersistence {
    async fn claim(
        &self,
        command_id: Option<&str>,
        limit: usize,
        claim_ttl_ms: u64,
    ) -> Result<ClaimedRuntimeCleanupBatch, ProjectStoreError> {
        self.store
            .claim_pending_runtime_cleanups(command_id, limit, claim_ttl_ms)
            .await
    }

    async fn succeed(&self, cleanup_id: &str, claim_token: &str) -> Result<(), ProjectStoreError> {
        self.store
            .succeed_runtime_cleanup(cleanup_id, claim_token)
            .await
    }

    async fn fail(
        &self,
        cleanup_id: &str,
        claim_token: &str,
        message: &str,
        retry_after_ms: u64,
    ) -> Result<(), ProjectStoreError> {
        self.store
            .fail_runtime_cleanup(cleanup_id, claim_token, message, retry_after_ms)
            .await
    }
}

impl RuntimeCleanupService {
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeControl>, store: Arc<dyn YardStore>) -> Self {
        Self {
            runtime,
            retry_state: shared_retry_state(&store),
            persistence: Arc::new(YardStoreRuntimeCleanupPersistence { store }),
        }
    }

    #[cfg(test)]
    fn with_persistence(
        runtime: Arc<dyn RuntimeControl>,
        persistence: Arc<dyn RuntimeCleanupPersistence>,
    ) -> Self {
        Self {
            runtime,
            persistence,
            retry_state: Arc::default(),
        }
    }

    /// Attempt all due cleanup jobs created by one committed command.
    ///
    /// Runtime failures are persisted for retry and returned in the report. A
    /// storage failure is returned because retry durability could not be
    /// confirmed.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when pending jobs cannot be loaded or their
    /// success/failure result cannot be persisted.
    pub async fn process_command(
        &self,
        command_id: &str,
    ) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        self.process(Some(command_id), Instant::now()).await
    }

    /// Attempt one bounded batch of all due runtime cleanup jobs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when pending jobs cannot be loaded or their
    /// success/failure result cannot be persisted.
    pub async fn process_pending(&self) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        self.process(None, Instant::now()).await
    }

    /// Retry durable cleanup work forever. Each pass is independent so a store
    /// or runtime outage cannot terminate future retirement attempts.
    pub async fn run(self) {
        loop {
            match self.process_pending().await {
                Ok(report) if report.attempted > 0 => {
                    debug!(
                        attempted = report.attempted,
                        succeeded = report.succeeded,
                        failed = report.failed,
                        "Processed displaced Herdr runtime cleanup jobs"
                    );
                }
                Ok(_) => {
                    let recovered = self.lock_retry_state().forget(PASS_THROTTLE_KEY);
                    if let Some(suppressed) = recovered {
                        info!(
                            suppressed_warnings = suppressed,
                            "Runtime cleanup pass recovered"
                        );
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    let decision = self.lock_retry_state().warnings.observe(
                        PASS_THROTTLE_KEY,
                        &message,
                        Instant::now(),
                    );
                    match decision {
                        WarnDecision::Warn { suppressed } => warn!(
                            error = %message,
                            suppressed_warnings = suppressed,
                            "Runtime cleanup pass failed; durable jobs will be retried"
                        ),
                        WarnDecision::Suppress => debug!(
                            error = %message,
                            "Runtime cleanup pass failed again; durable jobs will be retried"
                        ),
                    }
                }
            }
            tokio::time::sleep(RUNTIME_CLEANUP_INTERVAL).await;
        }
    }

    fn lock_retry_state(&self) -> std::sync::MutexGuard<'_, RetryState> {
        // The state is advisory; a poisoned lock still holds usable counters.
        self.retry_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn process(
        &self,
        command_id: Option<&str>,
        now: Instant,
    ) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        self.lock_retry_state().prune(now);
        let batch = self
            .persistence
            .claim(
                command_id,
                RUNTIME_CLEANUP_BATCH_SIZE,
                RUNTIME_CLEANUP_CLAIM_TTL_MS,
            )
            .await?;
        let mut report = RuntimeCleanupReport {
            attempted: batch.jobs.len() + batch.rejected,
            failed: batch.rejected,
            ..RuntimeCleanupReport::default()
        };
        for job in batch.jobs {
            match self.runtime.retire_runtime(retirement_request(&job)).await {
                Ok(()) => {
                    self.lock_retry_state().runtime_reachable();
                    if claim_lost(
                        self.persistence.succeed(&job.id, &job.claim_token).await,
                        &job,
                    )? {
                        self.lock_retry_state().forget(&job.id);
                        report.failed += 1;
                        continue;
                    }
                    report.succeeded += 1;
                    if let Some(suppressed) = self.lock_retry_state().forget(&job.id) {
                        info!(
                            cleanup_id = %job.id,
                            command_id = %job.command_id,
                            worker_id = %job.worker_id,
                            attempt = job.attempts.saturating_add(1),
                            suppressed_warnings = suppressed,
                            "Destructive runtime retirement succeeded after earlier failures"
                        );
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    let (retry_after_ms, decision) = {
                        let mut state = self.lock_retry_state();
                        let transport_streak = if is_transport_error(&error) {
                            state.record_transport_failure(&job.id, now)
                        } else {
                            state.runtime_reachable();
                            0
                        };
                        let retry_after_ms = retry_after_ms(&error, job.attempts, transport_streak);
                        let decision = state.warnings.observe(&job.id, &message, now);
                        (retry_after_ms, decision)
                    };
                    report.failed += 1;
                    if claim_lost(
                        self.persistence
                            .fail(&job.id, &job.claim_token, &message, retry_after_ms)
                            .await,
                        &job,
                    )? {
                        self.lock_retry_state().forget(&job.id);
                        continue;
                    }
                    match decision {
                        WarnDecision::Warn { suppressed } => warn!(
                            cleanup_id = %job.id,
                            command_id = %job.command_id,
                            worker_id = %job.worker_id,
                            reason = %job.reason,
                            attempt = job.attempts.saturating_add(1),
                            retry_after_ms,
                            suppressed_warnings = suppressed,
                            error = %message,
                            "Destructive runtime retirement failed; cleanup remains pending"
                        ),
                        WarnDecision::Suppress => debug!(
                            cleanup_id = %job.id,
                            command_id = %job.command_id,
                            worker_id = %job.worker_id,
                            reason = %job.reason,
                            attempt = job.attempts.saturating_add(1),
                            retry_after_ms,
                            error = %message,
                            "Destructive runtime retirement failed again; cleanup remains pending"
                        ),
                    }
                }
            }
        }
        Ok(report)
    }
}

/// Whether persisting a job's result found its claim gone: Restore cancelled
/// the job, or the claim expired and another pass took it. That job is no
/// longer this pass's to record, so the rest of the batch still runs.
fn claim_lost(
    result: Result<(), ProjectStoreError>,
    job: &PendingRuntimeCleanup,
) -> Result<bool, ProjectStoreError> {
    match result {
        Ok(()) => Ok(false),
        Err(ProjectStoreError::RuntimeCleanupClaimLost) => {
            debug!(
                cleanup_id = %job.id,
                command_id = %job.command_id,
                worker_id = %job.worker_id,
                "Runtime cleanup claim was lost before its result was recorded; skipping"
            );
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

/// Whether Herdr never answered the retirement check (transport/adapter
/// failure) as opposed to answering with an identity verdict.
fn is_transport_error(error: &crate::allocation_service::RuntimeRetirementError) -> bool {
    !matches!(
        error,
        crate::allocation_service::RuntimeRetirementError::AtomicIdentityGuardUnavailable
            | crate::allocation_service::RuntimeRetirementError::IdentityNotObserved
    )
}

/// Retry delay for a failed retirement.
///
/// Identity verdicts back off from 1 min to 1 h by the job's persisted
/// `attempts`. Transport failures back off from 1 s to 5 min by
/// `transport_streak`, the number of earlier consecutive transport failures
/// of this job since Herdr last answered.
fn retry_after_ms(
    error: &crate::allocation_service::RuntimeRetirementError,
    attempts: u32,
    transport_streak: u32,
) -> u64 {
    if is_transport_error(error) {
        capped_backoff_ms(
            TRANSPORT_RETRY_BASE_MS,
            TRANSPORT_RETRY_CAP_MS,
            transport_streak,
        )
    } else {
        capped_backoff_ms(IDENTITY_RETRY_BASE_MS, IDENTITY_RETRY_CAP_MS, attempts)
    }
}

fn capped_backoff_ms(base_ms: u64, cap_ms: u64, exponent: u32) -> u64 {
    let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    base_ms.saturating_mul(multiplier).min(cap_ms)
}

fn retirement_request(job: &PendingRuntimeCleanup) -> RuntimeRetirementRequest {
    RuntimeRetirementRequest {
        cleanup_id: job.id.clone(),
        adapter: job.adapter.clone(),
        session: job.session.clone(),
        workspace_id: job.workspace_id.clone(),
        terminal_id: job.terminal_id.clone(),
        tab_id: job.tab_id.clone(),
        pane_id: job.pane_id.clone(),
        provider_session: job.provider_session.clone(),
        owns_tab: job.owns_tab,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use yard_store::{ClaimedRuntimeCleanupBatch, PendingRuntimeCleanup, ProjectStoreError};

    use super::{
        RETRY_STATE_IDLE_TTL, RuntimeCleanupPersistence, RuntimeCleanupService,
        WARN_REPEAT_INTERVAL, WarnDecision, WarnThrottle,
    };
    use crate::allocation_service::{
        RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeRetirementError,
        RuntimeRetirementRequest,
    };

    fn pending_job(claim_token: String, attempts: u32) -> PendingRuntimeCleanup {
        PendingRuntimeCleanup {
            id: "cleanup-1".to_owned(),
            command_id: "command-1".to_owned(),
            worker_id: "worker-1".to_owned(),
            reason: "worker_session_end".to_owned(),
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            tab_id: Some("tab-1".to_owned()),
            pane_id: "pane-1".to_owned(),
            provider_session: None,
            owns_tab: true,
            claim_token,
            attempts,
            last_error: None,
        }
    }

    struct FailingSuccessPersistence {
        claim_count: AtomicUsize,
        fail_first_success: AtomicBool,
        succeeded: AtomicBool,
    }

    impl FailingSuccessPersistence {
        fn new() -> Self {
            Self {
                claim_count: AtomicUsize::new(0),
                fail_first_success: AtomicBool::new(true),
                succeeded: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl RuntimeCleanupPersistence for FailingSuccessPersistence {
        async fn claim(
            &self,
            _command_id: Option<&str>,
            _limit: usize,
            _claim_ttl_ms: u64,
        ) -> Result<ClaimedRuntimeCleanupBatch, ProjectStoreError> {
            if self.succeeded.load(Ordering::SeqCst) {
                return Ok(ClaimedRuntimeCleanupBatch {
                    jobs: Vec::new(),
                    rejected: 0,
                });
            }
            let claim = self.claim_count.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(ClaimedRuntimeCleanupBatch {
                jobs: vec![pending_job(
                    format!("claim-{claim}"),
                    u32::try_from(claim - 1).unwrap(),
                )],
                rejected: 0,
            })
        }

        async fn succeed(
            &self,
            cleanup_id: &str,
            claim_token: &str,
        ) -> Result<(), ProjectStoreError> {
            assert_eq!(cleanup_id, "cleanup-1");
            if self.fail_first_success.swap(false, Ordering::SeqCst) {
                assert_eq!(claim_token, "claim-1");
                return Err(ProjectStoreError::RuntimeCleanupClaimLost);
            }
            assert_eq!(claim_token, "claim-2");
            self.succeeded.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn fail(
            &self,
            _cleanup_id: &str,
            _claim_token: &str,
            _message: &str,
            _retry_after_ms: u64,
        ) -> Result<(), ProjectStoreError> {
            panic!("the idempotent runtime must not fail")
        }
    }

    #[derive(Default)]
    struct IdempotentRetirement {
        state: Mutex<RetirementState>,
    }

    #[derive(Default)]
    struct RetirementState {
        present: bool,
        physical_closes: usize,
        already_absent: usize,
    }

    #[async_trait]
    impl RuntimeControl for IdempotentRetirement {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
            Err(RuntimeProvisionError::BeforeWorker(
                "not used by this test".to_owned(),
            ))
        }

        async fn retire_runtime(
            &self,
            _request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            let mut state = self.state.lock().unwrap();
            if state.present {
                state.present = false;
                state.physical_closes += 1;
            } else {
                state.already_absent += 1;
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn persistence_failure_after_close_retries_and_converges_on_absence() {
        let runtime = Arc::new(IdempotentRetirement {
            state: Mutex::new(RetirementState {
                present: true,
                ..RetirementState::default()
            }),
        });
        let persistence = Arc::new(FailingSuccessPersistence::new());
        let service = RuntimeCleanupService::with_persistence(runtime.clone(), persistence.clone());

        // The lost claim is skipped, not a pass failure; the job stays due.
        let first = service.process_pending().await.unwrap();
        assert_eq!((first.attempted, first.succeeded, first.failed), (1, 0, 1));
        {
            let first = runtime.state.lock().unwrap();
            assert_eq!(first.physical_closes, 1);
            assert_eq!(first.already_absent, 0);
        }

        let report = service.process_pending().await.unwrap();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 0);
        let converged = runtime.state.lock().unwrap();
        assert_eq!(converged.physical_closes, 1);
        assert_eq!(converged.already_absent, 1);
        assert!(persistence.succeeded.load(Ordering::SeqCst));
    }

    /// Two due jobs; recording the first one's result finds its claim gone
    /// (Restore cancelled it mid-pass).
    struct ClaimLostPersistence {
        lose_on_fail: bool,
        recorded: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl RuntimeCleanupPersistence for ClaimLostPersistence {
        async fn claim(
            &self,
            _command_id: Option<&str>,
            _limit: usize,
            _claim_ttl_ms: u64,
        ) -> Result<ClaimedRuntimeCleanupBatch, ProjectStoreError> {
            let jobs = ["cleanup-a", "cleanup-b"]
                .into_iter()
                .map(|id| PendingRuntimeCleanup {
                    id: id.to_owned(),
                    ..pending_job(format!("claim-{id}"), 0)
                })
                .collect();
            Ok(ClaimedRuntimeCleanupBatch { jobs, rejected: 0 })
        }

        async fn succeed(
            &self,
            cleanup_id: &str,
            _claim_token: &str,
        ) -> Result<(), ProjectStoreError> {
            if !self.lose_on_fail && cleanup_id == "cleanup-a" {
                return Err(ProjectStoreError::RuntimeCleanupClaimLost);
            }
            self.recorded
                .lock()
                .unwrap()
                .push(format!("{cleanup_id}:ok"));
            Ok(())
        }

        async fn fail(
            &self,
            cleanup_id: &str,
            _claim_token: &str,
            _message: &str,
            _retry_after_ms: u64,
        ) -> Result<(), ProjectStoreError> {
            if self.lose_on_fail && cleanup_id == "cleanup-a" {
                return Err(ProjectStoreError::RuntimeCleanupClaimLost);
            }
            self.recorded
                .lock()
                .unwrap()
                .push(format!("{cleanup_id}:fail"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_lost_claim_skips_that_job_and_finishes_the_batch() {
        for lose_on_fail in [false, true] {
            let runtime = Arc::new(OutageRuntime {
                reachable: AtomicBool::new(!lose_on_fail),
                retires: if lose_on_fail { "none" } else { "cleanup-a" },
            });
            let persistence = Arc::new(ClaimLostPersistence {
                lose_on_fail,
                recorded: Mutex::new(Vec::new()),
            });
            let service = RuntimeCleanupService::with_persistence(runtime, persistence.clone());
            let report = service.process_pending().await.unwrap();
            assert_eq!(report.attempted, 2, "lose_on_fail={lose_on_fail}");
            // cleanup-b still ran (identity verdict or Herdr down) and was recorded.
            assert_eq!(*persistence.recorded.lock().unwrap(), ["cleanup-b:fail"]);
            assert_eq!(report.succeeded, 0);
            assert_eq!(report.failed, 2);
        }
    }

    #[derive(Default)]
    struct RecordingFailurePersistence {
        claimed: AtomicBool,
        failure: Mutex<Option<(String, u64)>>,
    }

    #[async_trait]
    impl RuntimeCleanupPersistence for RecordingFailurePersistence {
        async fn claim(
            &self,
            _command_id: Option<&str>,
            _limit: usize,
            _claim_ttl_ms: u64,
        ) -> Result<ClaimedRuntimeCleanupBatch, ProjectStoreError> {
            let jobs = if self.claimed.swap(true, Ordering::SeqCst) {
                Vec::new()
            } else {
                vec![pending_job("claim-1".to_owned(), 0)]
            };
            Ok(ClaimedRuntimeCleanupBatch { jobs, rejected: 0 })
        }

        async fn succeed(
            &self,
            _cleanup_id: &str,
            _claim_token: &str,
        ) -> Result<(), ProjectStoreError> {
            panic!("guard-unavailable retirement must not be persisted as success")
        }

        async fn fail(
            &self,
            cleanup_id: &str,
            claim_token: &str,
            message: &str,
            retry_after_ms: u64,
        ) -> Result<(), ProjectStoreError> {
            assert_eq!(cleanup_id, "cleanup-1");
            assert_eq!(claim_token, "claim-1");
            *self.failure.lock().unwrap() = Some((message.to_owned(), retry_after_ms));
            Ok(())
        }
    }

    struct GuardUnavailableRetirement;

    #[async_trait]
    impl RuntimeControl for GuardUnavailableRetirement {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
            Err(RuntimeProvisionError::BeforeWorker(
                "not used by this test".to_owned(),
            ))
        }

        async fn retire_runtime(
            &self,
            _request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            Err(RuntimeRetirementError::AtomicIdentityGuardUnavailable)
        }
    }

    #[tokio::test]
    async fn unavailable_identity_guard_remains_pending_for_retry() {
        let persistence = Arc::new(RecordingFailurePersistence::default());
        let service = RuntimeCleanupService::with_persistence(
            Arc::new(GuardUnavailableRetirement),
            persistence.clone(),
        );

        let report = service.process_pending().await.unwrap();

        assert_eq!(report.attempted, 1);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(
            persistence.failure.lock().unwrap().as_ref(),
            Some(&(
                "the installed Herdr protocol cannot atomically guard tab.close or pane.close by runtime identity"
                    .to_owned(),
                60_000,
            ))
        );
    }

    #[test]
    fn identity_retry_backoff_doubles_and_caps() {
        for error in [
            RuntimeRetirementError::AtomicIdentityGuardUnavailable,
            RuntimeRetirementError::IdentityNotObserved,
        ] {
            // The transport streak never affects identity verdicts.
            assert_eq!(super::retry_after_ms(&error, 0, 9), 60_000);
            assert_eq!(super::retry_after_ms(&error, 1, 0), 120_000);
            assert_eq!(super::retry_after_ms(&error, 5, 0), 1_920_000);
            assert_eq!(super::retry_after_ms(&error, 6, 0), 3_600_000);
            assert_eq!(super::retry_after_ms(&error, u32::MAX, 0), 3_600_000);
        }
    }

    #[test]
    fn transport_retry_backoff_doubles_from_one_second_and_caps_at_five_minutes() {
        for error in [
            RuntimeRetirementError::Runtime("Herdr socket unavailable".to_owned()),
            RuntimeRetirementError::UnsupportedAdapter("other".to_owned()),
        ] {
            // Persisted attempts never affect the transport schedule.
            assert_eq!(super::retry_after_ms(&error, 12, 0), 1_000);
            assert_eq!(super::retry_after_ms(&error, 0, 1), 2_000);
            assert_eq!(super::retry_after_ms(&error, 0, 2), 4_000);
            assert_eq!(super::retry_after_ms(&error, 0, 8), 256_000);
            assert_eq!(super::retry_after_ms(&error, 0, 9), 300_000);
            assert_eq!(super::retry_after_ms(&error, 0, u32::MAX), 300_000);
        }
    }

    #[test]
    fn warn_throttle_logs_changes_and_then_every_interval() {
        let start = Instant::now();
        let mut throttle = WarnThrottle::default();

        assert_eq!(
            throttle.observe("job", "unreachable", start),
            WarnDecision::Warn { suppressed: 0 }
        );
        for second in 1..=3 {
            assert_eq!(
                throttle.observe("job", "unreachable", start + Duration::from_secs(second)),
                WarnDecision::Suppress
            );
        }
        // Another job is throttled independently.
        assert_eq!(
            throttle.observe("other", "unreachable", start + Duration::from_secs(3)),
            WarnDecision::Warn { suppressed: 0 }
        );
        // A changed error is a state change and is logged immediately.
        assert_eq!(
            throttle.observe("job", "identity", start + Duration::from_secs(4)),
            WarnDecision::Warn { suppressed: 3 }
        );
        assert_eq!(
            throttle.observe("job", "identity", start + Duration::from_secs(5)),
            WarnDecision::Suppress
        );
        let due = start + Duration::from_secs(4) + WARN_REPEAT_INTERVAL;
        let just_before_due = start + Duration::from_secs(3) + WARN_REPEAT_INTERVAL;
        assert_eq!(
            throttle.observe("job", "identity", just_before_due),
            WarnDecision::Suppress
        );
        assert_eq!(
            throttle.observe("job", "identity", due),
            WarnDecision::Warn { suppressed: 2 }
        );
        assert_eq!(throttle.forget("job"), Some(0));
        assert_eq!(throttle.forget("job"), None);
        assert_eq!(
            throttle.observe("job", "identity", due),
            WarnDecision::Warn { suppressed: 0 }
        );
    }

    /// Serves one due job per claim and records every persisted retry delay.
    struct ScriptedPersistence {
        jobs: Mutex<Vec<&'static str>>,
        attempts: AtomicUsize,
        delays: Mutex<Vec<(String, u64)>>,
        succeeded: Mutex<Vec<String>>,
    }

    impl ScriptedPersistence {
        fn new(jobs: &[&'static str]) -> Self {
            Self {
                jobs: Mutex::new(jobs.to_vec()),
                attempts: AtomicUsize::new(0),
                delays: Mutex::new(Vec::new()),
                succeeded: Mutex::new(Vec::new()),
            }
        }

        fn delays_for(&self, id: &str) -> Vec<u64> {
            self.delays
                .lock()
                .unwrap()
                .iter()
                .filter(|(job, _)| job == id)
                .map(|(_, delay)| *delay)
                .collect()
        }
    }

    #[async_trait]
    impl RuntimeCleanupPersistence for ScriptedPersistence {
        async fn claim(
            &self,
            _command_id: Option<&str>,
            _limit: usize,
            _claim_ttl_ms: u64,
        ) -> Result<ClaimedRuntimeCleanupBatch, ProjectStoreError> {
            let attempts = u32::try_from(self.attempts.fetch_add(1, Ordering::SeqCst)).unwrap();
            let jobs = self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .map(|id| PendingRuntimeCleanup {
                    id: (*id).to_owned(),
                    ..pending_job(format!("claim-{id}"), attempts)
                })
                .collect();
            Ok(ClaimedRuntimeCleanupBatch { jobs, rejected: 0 })
        }

        async fn succeed(
            &self,
            cleanup_id: &str,
            _claim_token: &str,
        ) -> Result<(), ProjectStoreError> {
            self.jobs.lock().unwrap().retain(|id| *id != cleanup_id);
            self.succeeded.lock().unwrap().push(cleanup_id.to_owned());
            Ok(())
        }

        async fn fail(
            &self,
            cleanup_id: &str,
            _claim_token: &str,
            _message: &str,
            retry_after_ms: u64,
        ) -> Result<(), ProjectStoreError> {
            self.delays
                .lock()
                .unwrap()
                .push((cleanup_id.to_owned(), retry_after_ms));
            Ok(())
        }
    }

    /// Herdr is unreachable until `reachable` is set; then the listed job
    /// retires and every other job gets an identity verdict.
    struct OutageRuntime {
        reachable: AtomicBool,
        retires: &'static str,
    }

    #[async_trait]
    impl RuntimeControl for OutageRuntime {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
            Err(RuntimeProvisionError::BeforeWorker(
                "not used by this test".to_owned(),
            ))
        }

        async fn retire_runtime(
            &self,
            request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            if !self.reachable.load(Ordering::SeqCst) {
                return Err(RuntimeRetirementError::Runtime(
                    "failed to connect to Herdr socket".to_owned(),
                ));
            }
            if request.cleanup_id == self.retires {
                Ok(())
            } else {
                Err(RuntimeRetirementError::IdentityNotObserved)
            }
        }
    }

    #[tokio::test]
    async fn herdr_outage_backs_off_per_job_and_resets_once_herdr_answers() {
        let runtime = Arc::new(OutageRuntime {
            reachable: AtomicBool::new(false),
            retires: "cleanup-a",
        });
        let persistence = Arc::new(ScriptedPersistence::new(&["cleanup-a", "cleanup-b"]));
        let service = RuntimeCleanupService::with_persistence(runtime.clone(), persistence.clone());
        let start = Instant::now();

        for pass in 0..11 {
            let report = service
                .process(None, start + Duration::from_secs(pass))
                .await
                .unwrap();
            assert_eq!(report.failed, 2);
        }
        let outage = [
            1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 128_000, 256_000, 300_000, 300_000,
        ];
        assert_eq!(persistence.delays_for("cleanup-a"), outage);
        assert_eq!(persistence.delays_for("cleanup-b"), outage);

        // Herdr answers again: cleanup-a retires first, cleanup-b gets an
        // identity verdict (identity policy, by persisted attempts).
        runtime.reachable.store(true, Ordering::SeqCst);
        let report = service
            .process(None, start + Duration::from_secs(20))
            .await
            .unwrap();
        assert_eq!((report.succeeded, report.failed), (1, 1));
        assert_eq!(*persistence.succeeded.lock().unwrap(), ["cleanup-a"]);
        assert_eq!(persistence.delays_for("cleanup-b").last(), Some(&3_600_000));

        // A later outage starts from 1 s again instead of the old 5 min cap.
        runtime.reachable.store(false, Ordering::SeqCst);
        service
            .process(None, start + Duration::from_secs(21))
            .await
            .unwrap();
        service
            .process(None, start + Duration::from_secs(22))
            .await
            .unwrap();
        let delays = persistence.delays_for("cleanup-b");
        assert_eq!(delays[delays.len() - 2..], [1_000, 2_000]);
    }

    #[tokio::test]
    async fn repeated_job_failures_warn_once_per_interval_and_state_is_pruned() {
        let runtime = Arc::new(OutageRuntime {
            reachable: AtomicBool::new(false),
            retires: "cleanup-a",
        });
        let persistence = Arc::new(ScriptedPersistence::new(&["cleanup-a"]));
        let service = RuntimeCleanupService::with_persistence(runtime.clone(), persistence.clone());
        let start = Instant::now();

        for pass in 0..5 {
            service
                .process(None, start + Duration::from_secs(pass))
                .await
                .unwrap();
        }
        {
            let state = service.lock_retry_state();
            let entry = &state.warnings.entries["cleanup-a"];
            assert_eq!(entry.last_warned_at, start);
            assert_eq!(entry.suppressed, 4);
            assert_eq!(state.jobs["cleanup-a"].transport_failures, 5);
        }

        service
            .process(None, start + WARN_REPEAT_INTERVAL)
            .await
            .unwrap();
        {
            let state = service.lock_retry_state();
            let entry = &state.warnings.entries["cleanup-a"];
            assert_eq!(entry.last_warned_at, start + WARN_REPEAT_INTERVAL);
            assert_eq!(entry.suppressed, 0);
        }

        // A job that stops being claimed is forgotten after the idle TTL.
        persistence.jobs.lock().unwrap().clear();
        service
            .process(None, start + WARN_REPEAT_INTERVAL + RETRY_STATE_IDLE_TTL)
            .await
            .unwrap();
        let state = service.lock_retry_state();
        assert!(state.jobs.is_empty());
        assert!(state.warnings.entries.is_empty());
    }

    #[tokio::test]
    async fn success_clears_job_retry_and_warning_state() {
        let runtime = Arc::new(OutageRuntime {
            reachable: AtomicBool::new(false),
            retires: "cleanup-a",
        });
        let persistence = Arc::new(ScriptedPersistence::new(&["cleanup-a"]));
        let service = RuntimeCleanupService::with_persistence(runtime.clone(), persistence.clone());
        let start = Instant::now();

        service.process(None, start).await.unwrap();
        runtime.reachable.store(true, Ordering::SeqCst);
        let report = service
            .process(None, start + Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(report.succeeded, 1);
        let state = service.lock_retry_state();
        assert!(state.jobs.is_empty());
        assert!(state.warnings.entries.is_empty());
    }

    #[tokio::test]
    async fn cleanup_services_of_one_store_share_retry_and_warning_state() {
        use yard_store::{SqliteProjectStore, YardStore};

        let temp = tempfile::tempdir().unwrap();
        let store: Arc<dyn YardStore> = Arc::new(
            SqliteProjectStore::open(temp.path().join("a.sqlite3"))
                .await
                .unwrap(),
        );
        let other: Arc<dyn YardStore> = Arc::new(
            SqliteProjectStore::open(temp.path().join("b.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(OutageRuntime {
            reachable: AtomicBool::new(false),
            retires: "cleanup-a",
        });
        // An inline attempt (e.g. ProjectService after archive) and the
        // background runner are separate services over the same store.
        let inline = RuntimeCleanupService::new(runtime.clone(), Arc::clone(&store));
        let background = RuntimeCleanupService::new(runtime.clone(), Arc::clone(&store));
        let unrelated = RuntimeCleanupService::new(runtime, other);
        let now = Instant::now();

        {
            let mut state = inline.lock_retry_state();
            assert_eq!(state.record_transport_failure("cleanup-a", now), 0);
            assert_eq!(
                state.warnings.observe("cleanup-a", "unreachable", now),
                WarnDecision::Warn { suppressed: 0 }
            );
        }
        {
            let mut state = background.lock_retry_state();
            assert_eq!(state.record_transport_failure("cleanup-a", now), 1);
            assert_eq!(
                state.warnings.observe("cleanup-a", "unreachable", now),
                WarnDecision::Suppress
            );
        }
        let state = unrelated.lock_retry_state();
        assert!(state.jobs.is_empty());
        assert!(state.warnings.entries.is_empty());
    }
}

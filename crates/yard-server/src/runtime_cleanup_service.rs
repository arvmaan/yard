use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tracing::{debug, warn};
use yard_store::{ClaimedRuntimeCleanupBatch, PendingRuntimeCleanup, ProjectStoreError, YardStore};

use crate::allocation_service::{RuntimeControl, RuntimeRetirementRequest};

pub const RUNTIME_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);
const RUNTIME_CLEANUP_BATCH_SIZE: usize = 100;
const RUNTIME_CLEANUP_CLAIM_TTL_MS: u64 = 30_000;
const RUNTIME_CLEANUP_RETRY_MS: u64 = 1_000;
const IDENTITY_RETRY_BASE_MS: u64 = 60_000;
const IDENTITY_RETRY_CAP_MS: u64 = 60 * 60_000;

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
        self.process(Some(command_id)).await
    }

    /// Attempt one bounded batch of all due runtime cleanup jobs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when pending jobs cannot be loaded or their
    /// success/failure result cannot be persisted.
    pub async fn process_pending(&self) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        self.process(None).await
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
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        error = %error,
                        "Runtime cleanup pass failed; durable jobs will be retried"
                    );
                }
            }
            tokio::time::sleep(RUNTIME_CLEANUP_INTERVAL).await;
        }
    }

    async fn process(
        &self,
        command_id: Option<&str>,
    ) -> Result<RuntimeCleanupReport, ProjectStoreError> {
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
                    self.persistence.succeed(&job.id, &job.claim_token).await?;
                    report.succeeded += 1;
                }
                Err(error) => {
                    let retry_after_ms = retry_after_ms(&error, job.attempts);
                    self.persistence
                        .fail(
                            &job.id,
                            &job.claim_token,
                            &error.to_string(),
                            retry_after_ms,
                        )
                        .await?;
                    report.failed += 1;
                    warn!(
                        cleanup_id = %job.id,
                        command_id = %job.command_id,
                        worker_id = %job.worker_id,
                        reason = %job.reason,
                        attempt = job.attempts.saturating_add(1),
                        retry_after_ms,
                        error = %error,
                        "Destructive runtime retirement failed; cleanup remains pending"
                    );
                }
            }
        }
        Ok(report)
    }
}

fn retry_after_ms(error: &crate::allocation_service::RuntimeRetirementError, attempts: u32) -> u64 {
    if matches!(
        error,
        crate::allocation_service::RuntimeRetirementError::AtomicIdentityGuardUnavailable
            | crate::allocation_service::RuntimeRetirementError::IdentityNotObserved
    ) {
        let multiplier = 1_u64.checked_shl(attempts).unwrap_or(u64::MAX);
        return IDENTITY_RETRY_BASE_MS
            .saturating_mul(multiplier)
            .min(IDENTITY_RETRY_CAP_MS);
    }
    RUNTIME_CLEANUP_RETRY_MS
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
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use yard_store::{ClaimedRuntimeCleanupBatch, PendingRuntimeCleanup, ProjectStoreError};

    use super::{RuntimeCleanupPersistence, RuntimeCleanupService};
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

        let error = service.process_pending().await.unwrap_err();
        assert!(matches!(error, ProjectStoreError::RuntimeCleanupClaimLost));
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
                "Herdr protocol 19 cannot atomically guard tab.close or pane.close by runtime identity"
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
            assert_eq!(super::retry_after_ms(&error, 0), 60_000);
            assert_eq!(super::retry_after_ms(&error, 1), 120_000);
            assert_eq!(super::retry_after_ms(&error, 5), 1_920_000);
            assert_eq!(super::retry_after_ms(&error, 6), 3_600_000);
            assert_eq!(super::retry_after_ms(&error, u32::MAX), 3_600_000);
        }

        assert_eq!(
            super::retry_after_ms(&RuntimeRetirementError::Runtime("transient".to_owned()), 12),
            1_000
        );
    }
}

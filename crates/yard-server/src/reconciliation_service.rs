use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, info, warn};
use yard_domain::{RuntimeInventory, RuntimeReconciliation};
use yard_store::{ProjectStoreError, YardStore};

use crate::inventory_service::{InventoryServiceError, InventorySource};
use crate::runtime_cleanup_service::{WarnDecision, WarnThrottle};

pub const RECONCILIATION_INTERVAL: Duration = Duration::from_millis(500);
/// Throttle key for session discovery; per-session keys are the session name
/// behind this prefix so they cannot collide with it.
const DISCOVERY_THROTTLE_KEY: &str = "\u{0}session-discovery";

#[derive(Clone)]
pub struct ReconciliationService {
    source: Arc<dyn InventorySource>,
    store: Arc<dyn YardStore>,
    sessions: Arc<Mutex<HashMap<String, Arc<SessionReconciliation>>>>,
}

#[derive(Default)]
struct SessionReconciliation {
    operation: AsyncMutex<()>,
    cached: Mutex<Option<CachedReconciliation>>,
}

#[derive(Clone)]
struct CachedReconciliation {
    completed_at: Instant,
    inventory: RuntimeInventory,
    result: RuntimeReconciliation,
}

impl ReconciliationService {
    #[must_use]
    pub fn new(source: Arc<dyn InventorySource>, store: Arc<dyn YardStore>) -> Self {
        Self {
            source,
            store,
            sessions: Arc::default(),
        }
    }

    /// Snapshot one Herdr session and atomically reconcile its workers before
    /// returning the observed inventory.
    ///
    /// # Errors
    ///
    /// Returns an inventory or storage error without replacing the last
    /// durable projection with an inferred result.
    pub async fn inventory(
        &self,
        session: &str,
    ) -> Result<(RuntimeInventory, RuntimeReconciliation), ReconciliationServiceError> {
        self.reconcile(session, true).await
    }

    /// Force a new Herdr snapshot and durable reconciliation for one session.
    ///
    /// # Errors
    ///
    /// Returns an inventory or storage error without replacing the last
    /// durable projection with an inferred result.
    pub async fn refresh(
        &self,
        session: &str,
    ) -> Result<(RuntimeInventory, RuntimeReconciliation), ReconciliationServiceError> {
        self.reconcile(session, false).await
    }

    async fn reconcile(
        &self,
        session: &str,
        allow_cached: bool,
    ) -> Result<(RuntimeInventory, RuntimeReconciliation), ReconciliationServiceError> {
        let session_state = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                sessions
                    .entry(session.to_owned())
                    .or_insert_with(|| Arc::new(SessionReconciliation::default())),
            )
        };

        if allow_cached && let Some(cached) = fresh_reconciliation(&session_state) {
            return Ok((cached.inventory, cached.result));
        }

        let _operation = session_state.operation.lock().await;
        if allow_cached && let Some(cached) = fresh_reconciliation(&session_state) {
            return Ok((cached.inventory, cached.result));
        }

        let inventory = self.source.inventory(session).await?;
        let result = self
            .store
            .reconcile_runtime_inventory(inventory.clone())
            .await?;
        *session_state
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CachedReconciliation {
            completed_at: Instant::now(),
            inventory: inventory.clone(),
            result: result.clone(),
        });
        Ok((inventory, result))
    }

    /// Continuously resnapshot all running sessions. Each cycle is independent,
    /// so socket failure or sequence uncertainty is recovered by the next full
    /// snapshot rather than by replaying an untrusted partial event stream.
    ///
    /// While Herdr is unreachable every cycle fails the same way; that is
    /// logged at WARN on the first failure, when the error changes and then at
    /// most once per `WARN_REPEAT_INTERVAL` (DEBUG otherwise), with one INFO
    /// when it recovers.
    pub async fn run(self) {
        let mut warnings = WarnThrottle::default();
        loop {
            self.cycle(&mut warnings, Instant::now()).await;
            tokio::time::sleep(RECONCILIATION_INTERVAL).await;
        }
    }

    async fn cycle(&self, warnings: &mut WarnThrottle, now: Instant) -> CycleLog {
        let mut log = CycleLog::default();
        let sessions = match self.source.sessions().await {
            Ok(sessions) => sessions,
            Err(error) => {
                let message = error.to_string();
                match warnings.observe(DISCOVERY_THROTTLE_KEY, &message, now) {
                    WarnDecision::Warn { suppressed } => {
                        log.warned += 1;
                        warn!(
                            error = %message,
                            suppressed_warnings = suppressed,
                            "Herdr session discovery failed; the next cycle will retry"
                        );
                    }
                    WarnDecision::Suppress => debug!(
                        error = %message,
                        "Herdr session discovery failed again; the next cycle will retry"
                    ),
                }
                return log;
            }
        };
        if let Some(suppressed) = warnings.forget(DISCOVERY_THROTTLE_KEY) {
            log.recovered += 1;
            info!(
                suppressed_warnings = suppressed,
                "Herdr session discovery recovered"
            );
        }
        for session in sessions
            .sessions
            .into_iter()
            .filter(|session| session.running)
        {
            let key = format!("{DISCOVERY_THROTTLE_KEY}:{}", session.name);
            match self.inventory(&session.name).await {
                Ok((_, result)) => {
                    if let Some(suppressed) = warnings.forget(&key) {
                        log.recovered += 1;
                        info!(
                            session = %session.name,
                            suppressed_warnings = suppressed,
                            "Herdr reconciliation recovered"
                        );
                    }
                    if result.adopted_workers > 0
                        || result.updated_bindings > 0
                        || result.missing_bindings > 0
                        || result.ambiguous_bindings > 0
                        || result.exited_processes > 0
                    {
                        debug!(
                            session = %session.name,
                            adopted = result.adopted_workers,
                            updated = result.updated_bindings,
                            missing = result.missing_bindings,
                            ambiguous = result.ambiguous_bindings,
                            exited = result.exited_processes,
                            "Reconciled Herdr runtime"
                        );
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    match warnings.observe(&key, &message, now) {
                        WarnDecision::Warn { suppressed } => {
                            log.warned += 1;
                            warn!(
                                session = %session.name,
                                error = %message,
                                suppressed_warnings = suppressed,
                                "Herdr reconciliation failed; the next cycle will resnapshot"
                            );
                        }
                        WarnDecision::Suppress => debug!(
                            session = %session.name,
                            error = %message,
                            "Herdr reconciliation failed again; the next cycle will resnapshot"
                        ),
                    }
                }
            }
        }
        log
    }
}

/// What one reconciliation cycle logged above DEBUG (for tests).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CycleLog {
    warned: usize,
    recovered: usize,
}

fn fresh_reconciliation(state: &SessionReconciliation) -> Option<CachedReconciliation> {
    state
        .cached
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .filter(|cached| cached.completed_at.elapsed() < RECONCILIATION_INTERVAL)
        .cloned()
}

#[derive(Debug, Error)]
pub enum ReconciliationServiceError {
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use yard_domain::{RuntimeInventory, RuntimeSession, RuntimeSessions};
    use yard_herdr::HerdrError;
    use yard_store::SqliteProjectStore;

    use super::{CycleLog, ReconciliationService};
    use crate::inventory_service::{InventoryServiceError, InventorySource};
    use crate::runtime_cleanup_service::{WARN_REPEAT_INTERVAL, WarnThrottle};

    /// Discovery fails while `reachable` is false; once reachable it lists
    /// one running session whose snapshot fails while `snapshot_fails`.
    struct OutageInventory {
        reachable: AtomicBool,
        snapshot_fails: AtomicBool,
    }

    #[async_trait]
    impl InventorySource for OutageInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            if !self.reachable.load(Ordering::SeqCst) {
                return Err(HerdrError::DiscoveryTimeout.into());
            }
            let sessions = if self.snapshot_fails.load(Ordering::SeqCst) {
                vec![RuntimeSession {
                    name: "default".to_owned(),
                    is_default: true,
                    running: true,
                }]
            } else {
                Vec::new()
            };
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions,
            })
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            Err(HerdrError::SessionNotRunning(session_name.to_owned()).into())
        }
    }

    #[tokio::test]
    async fn herdr_outage_warns_once_per_interval_and_reports_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let source = Arc::new(OutageInventory {
            reachable: AtomicBool::new(false),
            snapshot_fails: AtomicBool::new(false),
        });
        let service = ReconciliationService::new(source.clone(), store);
        let mut warnings = WarnThrottle::default();
        let start = Instant::now();
        let warned = |log: CycleLog| (log.warned, log.recovered);

        // Two cycles per second for a minute: one WARN, not 120.
        let mut total = 0;
        for tick in 0..120 {
            let log = service
                .cycle(&mut warnings, start + Duration::from_millis(500 * tick))
                .await;
            total += log.warned;
        }
        assert_eq!(total, 1);
        assert_eq!(
            warned(
                service
                    .cycle(&mut warnings, start + WARN_REPEAT_INTERVAL)
                    .await
            ),
            (1, 0)
        );

        // Discovery answers again but the session snapshot fails: discovery
        // recovers once and the session failure is throttled on its own.
        source.reachable.store(true, Ordering::SeqCst);
        source.snapshot_fails.store(true, Ordering::SeqCst);
        let later = start + WARN_REPEAT_INTERVAL + Duration::from_secs(1);
        assert_eq!(warned(service.cycle(&mut warnings, later).await), (1, 1));
        assert_eq!(
            warned(
                service
                    .cycle(&mut warnings, later + Duration::from_secs(1))
                    .await
            ),
            (0, 0)
        );
        source.snapshot_fails.store(false, Ordering::SeqCst);
        // The session is no longer listed; its entry stays until it is next
        // seen, and discovery itself stays quiet.
        assert_eq!(
            warned(
                service
                    .cycle(&mut warnings, later + Duration::from_secs(2))
                    .await
            ),
            (0, 0)
        );
    }
}

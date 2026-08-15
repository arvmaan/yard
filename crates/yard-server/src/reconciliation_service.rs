use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, warn};
use yard_domain::{RuntimeInventory, RuntimeReconciliation};
use yard_store::{ProjectStoreError, YardStore};

use crate::inventory_service::{InventoryServiceError, InventorySource};

pub const RECONCILIATION_INTERVAL: Duration = Duration::from_millis(500);

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
    pub async fn run(self) {
        loop {
            match self.source.sessions().await {
                Ok(sessions) => {
                    for session in sessions
                        .sessions
                        .into_iter()
                        .filter(|session| session.running)
                    {
                        match self.inventory(&session.name).await {
                            Ok((_, result)) => {
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
                                warn!(
                                    session = %session.name,
                                    error = %error,
                                    "Herdr reconciliation failed; the next cycle will resnapshot"
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    warn!(
                        error = %error,
                        "Herdr session discovery failed; the next cycle will retry"
                    );
                }
            }
            tokio::time::sleep(RECONCILIATION_INTERVAL).await;
        }
    }
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

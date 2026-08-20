pub mod allocation_service;
pub mod artifact_service;
pub mod automation_service;
pub mod config;
pub mod coordination_node_service;
mod http;
pub mod intervention_service;
pub mod inventory_service;
pub mod orchestrator_replacement_service;
pub mod orchestrator_workflow_profile_service;
pub mod profile_service;
pub mod project_service;
mod provider_agents;
pub mod reconciliation_service;
pub mod runtime_cleanup_service;
mod status_protocol;
pub mod terminal_service;
pub mod worker_session_service;
pub mod yard_orchestrator_service;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use allocation_service::RuntimeControl;
use artifact_service::ArtifactService;
use automation_service::AutomationService;
use axum::Router;
use coordination_node_service::CoordinationNodeService;
use intervention_service::{InterventionService, RuntimeIntervention};
use inventory_service::InventorySource;
use reconciliation_service::ReconciliationService;
use std::path::PathBuf;
use terminal_service::RuntimeTerminal;
use tokio::sync::{Notify, watch};
use yard_store::YardStore;

#[derive(Clone, Default)]
pub struct ConnectionTracker {
    inner: Arc<ConnectionTrackerInner>,
}

#[derive(Default)]
struct ConnectionTrackerInner {
    active: AtomicUsize,
    closed: AtomicBool,
    idle: Notify,
}

pub(crate) struct ConnectionGuard {
    inner: Arc<ConnectionTrackerInner>,
}

impl ConnectionTracker {
    pub(crate) fn track(&self) -> Option<ConnectionGuard> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return None;
        }
        self.inner.active.fetch_add(1, Ordering::SeqCst);
        let guard = ConnectionGuard {
            inner: Arc::clone(&self.inner),
        };
        if self.inner.closed.load(Ordering::SeqCst) {
            drop(guard);
            None
        } else {
            Some(guard)
        }
    }

    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.idle.notify_waiters();
    }

    pub async fn wait_for_idle(&self) {
        loop {
            let idle = self.inner.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.inner.active.load(Ordering::SeqCst) == 0 {
                return;
            }
            idle.await;
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if self.inner.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

pub fn app(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
) -> Router {
    let reconciliation = ReconciliationService::new(Arc::clone(&source), Arc::clone(&store));
    app_with_reconciliation_and_orchestrator_cwd(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifact_path,
        reconciliation,
        default_orchestrator_cwd(),
    )
}

pub fn app_with_reconciliation(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
    reconciliation: ReconciliationService,
) -> Router {
    app_with_reconciliation_and_orchestrator_cwd(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifact_path,
        reconciliation,
        default_orchestrator_cwd(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn app_with_reconciliation_and_orchestrator_cwd(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
    reconciliation: ReconciliationService,
    orchestrator_cwd: PathBuf,
) -> Router {
    let data_root = artifact_path
        .parent()
        .map_or_else(|| PathBuf::from(".yard"), PathBuf::from);
    app_with_reconciliation_and_paths(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifact_path,
        reconciliation,
        orchestrator_cwd,
        data_root.join("coordination"),
        data_root.join("knowledge"),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn app_with_reconciliation_and_paths(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
    reconciliation: ReconciliationService,
    orchestrator_cwd: PathBuf,
    coordination_path: PathBuf,
    knowledge_path: PathBuf,
) -> Router {
    app_with_reconciliation_and_paths_and_automation(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifact_path,
        reconciliation,
        orchestrator_cwd,
        coordination_path,
        knowledge_path,
    )
    .0
}

/// Build the production router and its shared automation dispatcher.
///
/// The returned service must drive the scheduler. Sharing this exact clone
/// with HTTP keeps manual and scheduled runs behind one operation lock.
#[allow(clippy::too_many_arguments)]
pub fn app_with_reconciliation_and_paths_and_automation(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
    reconciliation: ReconciliationService,
    orchestrator_cwd: PathBuf,
    coordination_path: PathBuf,
    knowledge_path: PathBuf,
) -> (Router, AutomationService) {
    build_app_with_reconciliation_and_paths_and_automation(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifact_path,
        reconciliation,
        orchestrator_cwd,
        coordination_path,
        knowledge_path,
        None,
        ConnectionTracker::default(),
    )
}

/// Build the production router with coordinated connection shutdown.
#[allow(clippy::too_many_arguments)]
pub fn app_with_reconciliation_and_paths_and_automation_and_shutdown(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
    reconciliation: ReconciliationService,
    orchestrator_cwd: PathBuf,
    coordination_path: PathBuf,
    knowledge_path: PathBuf,
    shutdown: watch::Receiver<bool>,
) -> (Router, AutomationService, ConnectionTracker) {
    let connections = ConnectionTracker::default();
    let (router, automations) = build_app_with_reconciliation_and_paths_and_automation(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifact_path,
        reconciliation,
        orchestrator_cwd,
        coordination_path,
        knowledge_path,
        Some(shutdown),
        connections.clone(),
    );
    (router, automations, connections)
}

#[allow(clippy::too_many_arguments)]
fn build_app_with_reconciliation_and_paths_and_automation(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifact_path: PathBuf,
    reconciliation: ReconciliationService,
    orchestrator_cwd: PathBuf,
    coordination_path: PathBuf,
    knowledge_path: PathBuf,
    shutdown: Option<watch::Receiver<bool>>,
    connections: ConnectionTracker,
) -> (Router, AutomationService) {
    let artifacts = ArtifactService::new(artifact_path, Arc::clone(&store));
    let coordination_nodes = CoordinationNodeService::new(
        Arc::clone(&source),
        Arc::clone(&runtime),
        Arc::clone(&intervention),
        Arc::clone(&store),
        reconciliation.clone(),
        coordination_path.clone(),
        knowledge_path.clone(),
    );
    let interventions = InterventionService::new(
        Arc::clone(&source),
        Arc::clone(&intervention),
        Arc::clone(&store),
    );
    let automations = AutomationService::new(Arc::clone(&store), interventions, coordination_nodes);
    let router = http::router_with_reconciliation_and_shutdown(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifacts,
        reconciliation,
        orchestrator_cwd
            .into_os_string()
            .to_string_lossy()
            .into_owned(),
        coordination_path,
        knowledge_path,
        automations.clone(),
        shutdown,
        connections,
    );
    (router, automations)
}

fn default_orchestrator_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::ConnectionTracker;

    #[tokio::test]
    async fn closed_connection_tracker_rejects_new_work_and_drains_existing_work() {
        let tracker = ConnectionTracker::default();
        let guard = tracker.track().expect("initial connection");
        tracker.close();
        assert!(tracker.track().is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), tracker.wait_for_idle())
                .await
                .is_err()
        );
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), tracker.wait_for_idle())
            .await
            .expect("tracker should drain");
    }
}

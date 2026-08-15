pub mod allocation_service;
pub mod artifact_service;
pub mod automation_service;
pub mod config;
pub mod coordination_node_service;
mod http;
pub mod intervention_service;
pub mod inventory_service;
pub mod profile_service;
pub mod project_service;
mod provider_agents;
pub mod reconciliation_service;
pub mod runtime_cleanup_service;
mod status_protocol;
pub mod terminal_service;
pub mod worker_session_service;
pub mod yard_orchestrator_service;

use std::sync::Arc;

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
use yard_store::YardStore;

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
    let router = http::router_with_reconciliation(
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
    );
    (router, automations)
}

fn default_orchestrator_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

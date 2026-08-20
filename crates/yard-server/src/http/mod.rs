use std::{env, ffi::OsString, io, path::PathBuf, process::Stdio, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::Serialize;
use tokio::sync::watch;
use yard_domain::{
    AgentProfile, AgentProfiles, Artifact, ArtifactContent, Assignments, Automation,
    AutomationCommandResult, AutomationRun, AutomationRunCommandResult, AutomationRuns,
    Automations, ConfigureYardOrchestrator, ConfiguredYardOrchestrator, ConfirmProfileAllocation,
    ConfirmWorkerAllocation, ConfirmWorkerHandoff, ConfirmedAllocation, ConfirmedProjectCreation,
    ConfirmedWorkerHandoff, CoordinationNode, CoordinationNodeCommandResult,
    CoordinationNodePromptAcknowledgement, CoordinationNodeRoute, CoordinationNodeRoutes,
    CoordinationNodeTerminalOutput, CoordinationNodes, CoordinationSnapshot, CoordinationSnapshots,
    CreateAgentProfile, CreateAutomation, CreateCoordinationNode, CreateProject,
    CreateProjectFromProfile, CreateProjectRelationship, CreateWorkerProfile,
    CreateWorkspaceProjectFromProfile, CreatedProjectRelationship, DeleteProjectRelationship,
    DeletedProjectRelationship, EndWorkerSession, EndedWorkerSession,
    OrchestratorPromptAcknowledgement, OrchestratorTerminalOutput, OrchestratorWorkflowProfile,
    Project, ProjectRelationships, Projects, PromptAcknowledgement, ProvisionCoordinationNode,
    ProvisionYardOrchestrator, RecordCompletionReceipt, RecordedCompletionReceipt,
    RecoverYardOrchestrator, RecoveredYardOrchestrator, ReplaceProjectOrchestrator,
    ReplacedProjectOrchestrator, RequestCoordinationSnapshot, ResetOrchestratorWorkflowProfile,
    RunAutomationNow, RuntimeInventory, RuntimeSessions, SendAssignmentPrompt,
    SendCoordinationNodePrompt, SendCoordinationNodeRoute, SendOrchestratorPrompt,
    SendYardOrchestratorPrompt, SendYardOrchestratorRoute, SetAutomationPaused, TerminalOutput,
    TokenSpendSettings, TransferProjectOrchestrator, TransferredProjectOrchestrator,
    UpdateAgentProfile, UpdateAutomation, UpdateAutomationPlacement, UpdateCoordinationNode,
    UpdateCoordinationNodePlacement, UpdateOrchestratorWorkflowProfile, UpdateProjectPlacement,
    UpdateTokenSpendSettings, UpdateWorkerProfile, UploadArtifact, WorkerCandidates, WorkerProfile,
    WorkerProfiles, YardOrchestrator, YardOrchestratorPromptAcknowledgement, YardOrchestratorRoute,
    YardOrchestratorRoutes, YardOrchestratorTerminalOutput,
};
use yard_herdr::HerdrError;
use yard_store::{ProjectStoreError, YardStore};

use crate::ConnectionTracker;
use crate::allocation_service::{AllocationService, AllocationServiceError, RuntimeControl};
use crate::artifact_service::{ArtifactService, ArtifactServiceError, StoredArtifact};
use crate::automation_service::{AutomationService, AutomationServiceError};
use crate::coordination_node_service::{CoordinationNodeService, CoordinationNodeServiceError};
use crate::intervention_service::{
    InterventionService, InterventionServiceError, RuntimeIntervention, RuntimeInterventionError,
};
use crate::inventory_service::{InventoryServiceError, InventorySource};
use crate::orchestrator_replacement_service::{
    OrchestratorReplacementService, OrchestratorReplacementServiceError,
};
use crate::orchestrator_workflow_profile_service::{
    OrchestratorWorkflowProfileService, OrchestratorWorkflowProfileServiceError,
};
use crate::profile_service::{AgentProfileServiceError, ProfileService, ProfileServiceError};
use crate::project_orchestrator_transfer_service::{
    ProjectOrchestratorTransferService, ProjectOrchestratorTransferServiceError,
};
use crate::project_service::{ProjectService, ProjectServiceError};
use crate::reconciliation_service::{ReconciliationService, ReconciliationServiceError};
use crate::terminal_service::{RuntimeTerminal, TerminalService};
use crate::worker_session_service::{WorkerSessionService, WorkerSessionServiceError};
use crate::yard_orchestrator_service::{YardOrchestratorService, YardOrchestratorServiceError};

mod terminal;
mod web;

#[derive(Clone)]
struct AppState {
    store: Arc<dyn YardStore>,
    source: Arc<dyn InventorySource>,
    reconciliation: ReconciliationService,
    projects: ProjectService,
    profiles: ProfileService,
    orchestrator_workflow_profiles: OrchestratorWorkflowProfileService,
    allocations: AllocationService,
    orchestrator_replacements: OrchestratorReplacementService,
    orchestrator_transfers: ProjectOrchestratorTransferService,
    worker_sessions: WorkerSessionService,
    interventions: InterventionService,
    terminals: TerminalService,
    artifacts: ArtifactService,
    automations: AutomationService,
    yard_orchestrator: YardOrchestratorService,
    coordination_nodes: CoordinationNodeService,
    shutdown: Option<watch::Receiver<bool>>,
    connections: ConnectionTracker,
}

#[cfg(test)]
pub(crate) fn router(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifacts: ArtifactService,
) -> Router {
    test_router_with_shutdown(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifacts,
        None,
        ConnectionTracker::default(),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn test_router_with_shutdown(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifacts: ArtifactService,
    shutdown: Option<watch::Receiver<bool>>,
    connections: ConnectionTracker,
) -> Router {
    let reconciliation = ReconciliationService::new(Arc::clone(&source), Arc::clone(&store));
    let managed_root = env::temp_dir().join(format!("yard-http-{}", uuid::Uuid::now_v7()));
    let coordination_nodes = CoordinationNodeService::new(
        Arc::clone(&source),
        Arc::clone(&runtime),
        Arc::clone(&intervention),
        Arc::clone(&store),
        reconciliation.clone(),
        managed_root.join("coordination"),
        managed_root.join("knowledge"),
    );
    let interventions = InterventionService::new(
        Arc::clone(&source),
        Arc::clone(&intervention),
        Arc::clone(&store),
    );
    let automations = AutomationService::new(Arc::clone(&store), interventions, coordination_nodes);
    router_with_reconciliation_and_shutdown(
        source,
        runtime,
        intervention,
        terminal,
        store,
        artifacts,
        reconciliation,
        default_orchestrator_cwd(),
        managed_root.join("coordination"),
        managed_root.join("knowledge"),
        automations,
        shutdown,
        connections,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn router_with_reconciliation_and_shutdown(
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    terminal: Arc<dyn RuntimeTerminal>,
    store: Arc<dyn YardStore>,
    artifacts: ArtifactService,
    reconciliation: ReconciliationService,
    orchestrator_cwd: String,
    coordination_path: PathBuf,
    knowledge_path: PathBuf,
    automations: AutomationService,
    shutdown: Option<watch::Receiver<bool>>,
    connections: ConnectionTracker,
) -> Router {
    let projects = ProjectService::new(
        Arc::clone(&source),
        Arc::clone(&runtime),
        Arc::clone(&store),
    );
    let profiles = ProfileService::new(Arc::clone(&store));
    let orchestrator_workflow_profiles =
        OrchestratorWorkflowProfileService::new(Arc::clone(&store));
    let worker_sessions = WorkerSessionService::new(Arc::clone(&runtime), Arc::clone(&store));
    let allocations = AllocationService::new(
        Arc::clone(&source),
        Arc::clone(&runtime),
        Arc::clone(&intervention),
        Arc::clone(&store),
    );
    let orchestrator_replacements = OrchestratorReplacementService::new(
        Arc::clone(&source),
        Arc::clone(&runtime),
        Arc::clone(&store),
    );
    let orchestrator_transfers =
        ProjectOrchestratorTransferService::new(Arc::clone(&source), Arc::clone(&store));
    let coordination_nodes = CoordinationNodeService::new(
        Arc::clone(&source),
        Arc::clone(&runtime),
        Arc::clone(&intervention),
        Arc::clone(&store),
        reconciliation.clone(),
        coordination_path,
        knowledge_path,
    );
    let yard_orchestrator_intervention = Arc::clone(&intervention);
    let interventions =
        InterventionService::new(Arc::clone(&source), intervention, Arc::clone(&store));
    let terminals =
        TerminalService::new(interventions.clone(), coordination_nodes.clone(), terminal);
    let yard_orchestrator = YardOrchestratorService::new(
        runtime,
        yard_orchestrator_intervention,
        Arc::clone(&store),
        reconciliation.clone(),
        orchestrator_cwd,
    );
    Router::new()
        .route("/health", get(health))
        .route(
            "/api/v1/orchestrator-workflow-profile",
            get(get_orchestrator_workflow_profile).put(update_orchestrator_workflow_profile),
        )
        .route(
            "/api/v1/orchestrator-workflow-profile/reset",
            axum::routing::post(reset_orchestrator_workflow_profile),
        )
        .route(
            "/api/v1/token-spend-settings",
            get(get_token_spend_settings).put(update_token_spend_settings),
        )
        .route(
            "/api/v1/automations",
            get(list_automations).post(create_automation),
        )
        .route(
            "/api/v1/automations/{automation_id}",
            get(get_automation).put(update_automation),
        )
        .route(
            "/api/v1/automations/{automation_id}/placement",
            put(update_automation_placement),
        )
        .route(
            "/api/v1/automations/{automation_id}/state",
            put(set_automation_state),
        )
        .route(
            "/api/v1/automations/{automation_id}/runs",
            get(list_automation_runs).post(run_automation_now),
        )
        .route(
            "/api/v1/automations/{automation_id}/runs/{run_id}",
            get(get_automation_run),
        )
        .route(
            "/api/v1/coordination-nodes",
            get(list_coordination_nodes).post(create_coordination_node),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}",
            get(get_coordination_node).put(update_coordination_node),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/placement",
            put(update_coordination_node_placement),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/provision",
            axum::routing::post(provision_coordination_node),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/prompts",
            axum::routing::post(prompt_coordination_node),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/terminal-output",
            get(read_coordination_node_output),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/terminal",
            get(terminal::coordination_node_terminal),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/routes",
            get(list_coordination_node_routes).post(route_coordination_node),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/snapshots",
            get(list_coordination_snapshots).post(request_coordination_snapshot),
        )
        .route(
            "/api/v1/coordination-nodes/{node_id}/snapshots/{snapshot_id}",
            get(get_coordination_snapshot),
        )
        .route(
            "/api/v1/yard/orchestrator",
            get(get_yard_orchestrator)
                .post(provision_yard_orchestrator)
                .put(configure_yard_orchestrator),
        )
        .route(
            "/api/v1/yard/orchestrator/prompts",
            axum::routing::post(prompt_yard_orchestrator),
        )
        .route(
            "/api/v1/yard/orchestrator/recover",
            axum::routing::post(recover_yard_orchestrator),
        )
        .route(
            "/api/v1/yard/orchestrator/routes",
            get(list_yard_orchestrator_routes).post(route_yard_orchestrator),
        )
        .route(
            "/api/v1/yard/orchestrator/terminal-output",
            get(read_yard_orchestrator_output),
        )
        .route(
            "/api/v1/yard/orchestrator/terminal",
            get(terminal::yard_orchestrator_terminal),
        )
        .route("/api/v1/runtimes/herdr/sessions", get(sessions))
        .route(
            "/api/v1/runtimes/herdr/sessions/{session}/inventory",
            get(inventory),
        )
        .route(
            "/api/v1/runtimes/herdr/sessions/{session}/terminals/{terminal_id}/open-ghostty",
            axum::routing::post(open_terminal_in_ghostty),
        )
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route(
            "/api/v1/project-relationships",
            get(list_project_relationships).post(create_project_relationship),
        )
        .route(
            "/api/v1/project-relationships/{relationship_id}/delete",
            axum::routing::post(delete_project_relationship),
        )
        .route(
            "/api/v1/projects/from-profile",
            axum::routing::post(create_project_from_profile),
        )
        .route(
            "/api/v1/projects/from-profile/workspace",
            axum::routing::post(create_project_with_workspace),
        )
        .route("/api/v1/workers", get(list_workers))
        .route(
            "/api/v1/workers/{worker_id}/end-session",
            axum::routing::post(end_worker_session),
        )
        .route("/api/v1/projects/{project_id}", get(get_project))
        .route(
            "/api/v1/projects/{project_id}/orchestrator",
            put(transfer_project_orchestrator),
        )
        .route(
            "/api/v1/projects/{project_id}/placement",
            put(update_project_placement),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments",
            get(list_project_assignments).post(confirm_allocation),
        )
        .route(
            "/api/v1/projects/{project_id}/orchestrator/prompts",
            axum::routing::post(prompt_orchestrator),
        )
        .route(
            "/api/v1/projects/{project_id}/orchestrator/replace",
            axum::routing::post(replace_project_orchestrator),
        )
        .route(
            "/api/v1/projects/{project_id}/orchestrator/terminal-output",
            get(read_orchestrator_output),
        )
        .route(
            "/api/v1/projects/{project_id}/orchestrator/terminal",
            get(terminal::orchestrator_terminal),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/completion-receipts",
            axum::routing::post(record_completion_receipt),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/handoffs",
            axum::routing::post(confirm_worker_handoff),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/prompts",
            axum::routing::post(prompt_assignment),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal-output",
            get(read_assignment_output),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal",
            get(terminal::assignment_terminal),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/artifacts/{artifact_id}",
            get(get_artifact).put(put_artifact),
        )
        .route(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/artifacts/{artifact_id}/content",
            get(get_artifact_content),
        )
        .route(
            "/api/v1/worker-profiles",
            get(list_worker_profiles).post(create_worker_profile),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}",
            get(get_worker_profile).put(update_worker_profile),
        )
        .route(
            "/api/v1/agent-profiles",
            get(list_agent_profiles).post(create_agent_profile),
        )
        .route(
            "/api/v1/agent-profiles/{profile_id}",
            get(get_agent_profile).put(update_agent_profile),
        )
        .route(
            "/api/v1/agent-profiles/{profile_id}/revisions/{profile_version}",
            get(get_agent_profile_revision),
        )
        .fallback(web::serve)
        .with_state(AppState {
            store,
            source,
            reconciliation,
            projects,
            profiles,
            orchestrator_workflow_profiles,
            allocations,
            orchestrator_replacements,
            orchestrator_transfers,
            worker_sessions,
            interventions,
            terminals,
            artifacts,
            automations,
            yard_orchestrator,
            coordination_nodes,
            shutdown,
            connections,
        })
}

#[cfg(test)]
fn default_orchestrator_cwd() -> String {
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("/"))
        .to_string_lossy()
        .into_owned()
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn get_orchestrator_workflow_profile(
    State(state): State<AppState>,
) -> Result<NoStoreJson<OrchestratorWorkflowProfile>, ApiError> {
    state
        .orchestrator_workflow_profiles
        .get()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_orchestrator_workflow_profile(
    State(state): State<AppState>,
    Json(command): Json<UpdateOrchestratorWorkflowProfile>,
) -> Result<NoStoreJson<OrchestratorWorkflowProfile>, ApiError> {
    state
        .orchestrator_workflow_profiles
        .update(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn reset_orchestrator_workflow_profile(
    State(state): State<AppState>,
    Json(command): Json<ResetOrchestratorWorkflowProfile>,
) -> Result<NoStoreJson<OrchestratorWorkflowProfile>, ApiError> {
    state
        .orchestrator_workflow_profiles
        .reset(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_token_spend_settings(
    State(state): State<AppState>,
) -> Result<NoStoreJson<TokenSpendSettings>, ApiError> {
    state
        .automations
        .token_spend_settings()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_token_spend_settings(
    State(state): State<AppState>,
    Json(command): Json<UpdateTokenSpendSettings>,
) -> Result<NoStoreJson<TokenSpendSettings>, ApiError> {
    state
        .automations
        .update_token_spend_settings(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_automations(
    State(state): State<AppState>,
) -> Result<NoStoreJson<Automations>, ApiError> {
    state
        .automations
        .list()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_automation(
    State(state): State<AppState>,
    Path(automation_id): Path<String>,
) -> Result<NoStoreJson<Automation>, ApiError> {
    state
        .automations
        .get(&automation_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn create_automation(
    State(state): State<AppState>,
    Json(mut command): Json<CreateAutomation>,
) -> Result<CreatedAutomation, ApiError> {
    command.automation_id = uuid::Uuid::now_v7().to_string();
    state
        .automations
        .create(command)
        .await
        .map(CreatedAutomation)
        .map_err(ApiError::from)
}

async fn update_automation(
    State(state): State<AppState>,
    Path(automation_id): Path<String>,
    Json(mut command): Json<UpdateAutomation>,
) -> Result<NoStoreJson<AutomationCommandResult>, ApiError> {
    command.automation_id = automation_id;
    state
        .automations
        .update(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_automation_placement(
    State(state): State<AppState>,
    Path(automation_id): Path<String>,
    Json(mut command): Json<UpdateAutomationPlacement>,
) -> Result<NoStoreJson<AutomationCommandResult>, ApiError> {
    command.automation_id = automation_id;
    state
        .automations
        .update_placement(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn set_automation_state(
    State(state): State<AppState>,
    Path(automation_id): Path<String>,
    Json(mut command): Json<SetAutomationPaused>,
) -> Result<NoStoreJson<AutomationCommandResult>, ApiError> {
    command.automation_id = automation_id;
    state
        .automations
        .set_paused(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_automation_runs(
    State(state): State<AppState>,
    Path(automation_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<RouteListQuery>,
) -> Result<NoStoreJson<AutomationRuns>, ApiError> {
    state
        .automations
        .list_runs(&automation_id, query.limit)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn run_automation_now(
    State(state): State<AppState>,
    Path(automation_id): Path<String>,
    Json(mut command): Json<RunAutomationNow>,
) -> Result<CreatedAutomationRun, ApiError> {
    command.automation_id = automation_id;
    state
        .automations
        .run_now(command)
        .await
        .map(CreatedAutomationRun)
        .map_err(ApiError::from)
}

async fn get_automation_run(
    State(state): State<AppState>,
    Path((automation_id, run_id)): Path<(String, String)>,
) -> Result<NoStoreJson<AutomationRun>, ApiError> {
    state
        .automations
        .get_run(&automation_id, &run_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_coordination_nodes(
    State(state): State<AppState>,
) -> Result<NoStoreJson<CoordinationNodes>, ApiError> {
    state
        .coordination_nodes
        .list()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_coordination_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<NoStoreJson<CoordinationNode>, ApiError> {
    state
        .coordination_nodes
        .get(&node_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn create_coordination_node(
    State(state): State<AppState>,
    Json(command): Json<CreateCoordinationNode>,
) -> Result<NoStoreJson<CoordinationNodeCommandResult>, ApiError> {
    state
        .coordination_nodes
        .create(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_coordination_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(command): Json<UpdateCoordinationNode>,
) -> Result<NoStoreJson<CoordinationNodeCommandResult>, ApiError> {
    state
        .coordination_nodes
        .update(&node_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_coordination_node_placement(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(command): Json<UpdateCoordinationNodePlacement>,
) -> Result<NoStoreJson<CoordinationNodeCommandResult>, ApiError> {
    state
        .coordination_nodes
        .update_placement(&node_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn provision_coordination_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(command): Json<ProvisionCoordinationNode>,
) -> Result<NoStoreJson<CoordinationNodeCommandResult>, ApiError> {
    state
        .coordination_nodes
        .provision(&node_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn prompt_coordination_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(command): Json<SendCoordinationNodePrompt>,
) -> Result<NoStoreJson<CoordinationNodePromptAcknowledgement>, ApiError> {
    state
        .coordination_nodes
        .prompt(&node_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn read_coordination_node_output(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<OutputQuery>,
) -> Result<NoStoreJson<CoordinationNodeTerminalOutput>, ApiError> {
    state
        .coordination_nodes
        .read_output(&node_id, query.lines)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_coordination_node_routes(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<RouteListQuery>,
) -> Result<NoStoreJson<CoordinationNodeRoutes>, ApiError> {
    state
        .coordination_nodes
        .list_routes(&node_id, query.limit)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn route_coordination_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(command): Json<SendCoordinationNodeRoute>,
) -> Result<NoStoreJson<CoordinationNodeRoute>, ApiError> {
    state
        .coordination_nodes
        .route(&node_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn request_coordination_snapshot(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(command): Json<RequestCoordinationSnapshot>,
) -> Result<NoStoreJson<CoordinationSnapshot>, ApiError> {
    state
        .coordination_nodes
        .request_snapshot(&node_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_coordination_snapshots(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<NoStoreJson<CoordinationSnapshots>, ApiError> {
    state
        .coordination_nodes
        .list_snapshots(&node_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_coordination_snapshot(
    State(state): State<AppState>,
    Path((node_id, snapshot_id)): Path<(String, String)>,
) -> Result<NoStoreJson<CoordinationSnapshot>, ApiError> {
    state
        .coordination_nodes
        .get_snapshot(&node_id, &snapshot_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_yard_orchestrator(
    State(state): State<AppState>,
) -> Result<NoStoreJson<YardOrchestrator>, ApiError> {
    state
        .store
        .get_yard_orchestrator()
        .await
        .map(NoStoreJson)
        .map_err(yard_orchestrator_store_error)
}

async fn configure_yard_orchestrator(
    State(state): State<AppState>,
    Json(command): Json<ConfigureYardOrchestrator>,
) -> Result<NoStoreJson<ConfiguredYardOrchestrator>, ApiError> {
    state
        .yard_orchestrator
        .configure(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn provision_yard_orchestrator(
    State(state): State<AppState>,
    Json(command): Json<ProvisionYardOrchestrator>,
) -> Result<NoStoreJson<ConfiguredYardOrchestrator>, ApiError> {
    state
        .yard_orchestrator
        .provision(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn recover_yard_orchestrator(
    State(state): State<AppState>,
    Json(command): Json<RecoverYardOrchestrator>,
) -> Result<NoStoreJson<RecoveredYardOrchestrator>, ApiError> {
    state
        .yard_orchestrator
        .recover(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn prompt_yard_orchestrator(
    State(state): State<AppState>,
    Json(command): Json<SendYardOrchestratorPrompt>,
) -> Result<NoStoreJson<YardOrchestratorPromptAcknowledgement>, ApiError> {
    state
        .interventions
        .prompt_yard_orchestrator(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

#[derive(Debug, serde::Deserialize)]
struct RouteListQuery {
    #[serde(default = "default_route_limit")]
    limit: usize,
}

const fn default_route_limit() -> usize {
    100
}

async fn list_yard_orchestrator_routes(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<RouteListQuery>,
) -> Result<NoStoreJson<YardOrchestratorRoutes>, ApiError> {
    if !(1..=500).contains(&query.limit) {
        return Err(ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_route_limit",
            message: "Route limit must be between 1 and 500".to_owned(),
        });
    }
    state
        .store
        .list_yard_orchestrator_routes(query.limit)
        .await
        .map(NoStoreJson)
        .map_err(coordination_store_error)
}

async fn route_yard_orchestrator(
    State(state): State<AppState>,
    Json(command): Json<SendYardOrchestratorRoute>,
) -> Result<NoStoreJson<YardOrchestratorRoute>, ApiError> {
    state
        .interventions
        .route_yard_orchestrator(command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn read_yard_orchestrator_output(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<OutputQuery>,
) -> Result<NoStoreJson<YardOrchestratorTerminalOutput>, ApiError> {
    state
        .interventions
        .read_yard_orchestrator_output(query.lines)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_project_relationships(
    State(state): State<AppState>,
) -> Result<NoStoreJson<ProjectRelationships>, ApiError> {
    state
        .store
        .list_project_relationships()
        .await
        .map(NoStoreJson)
        .map_err(coordination_store_error)
}

async fn create_project_relationship(
    State(state): State<AppState>,
    Json(command): Json<CreateProjectRelationship>,
) -> Result<NoStoreJson<CreatedProjectRelationship>, ApiError> {
    state
        .store
        .create_project_relationship(command)
        .await
        .map(NoStoreJson)
        .map_err(coordination_store_error)
}

async fn delete_project_relationship(
    State(state): State<AppState>,
    Path(relationship_id): Path<String>,
    Json(command): Json<DeleteProjectRelationship>,
) -> Result<NoStoreJson<DeletedProjectRelationship>, ApiError> {
    state
        .store
        .delete_project_relationship(&relationship_id, command)
        .await
        .map(NoStoreJson)
        .map_err(coordination_store_error)
}

async fn sessions(State(state): State<AppState>) -> Result<NoStoreJson<RuntimeSessions>, ApiError> {
    state
        .source
        .sessions()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn inventory(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> Result<NoStoreJson<RuntimeInventory>, ApiError> {
    state
        .reconciliation
        .inventory(&session)
        .await
        .map(|(inventory, _)| NoStoreJson(inventory))
        .map_err(ApiError::from)
}

#[derive(Serialize)]
struct ExternalTerminalLaunch {
    application: &'static str,
    terminal_id: String,
    command: Vec<String>,
}

async fn open_terminal_in_ghostty(
    State(state): State<AppState>,
    Path((session, terminal_id)): Path<(String, String)>,
) -> Result<NoStoreJson<ExternalTerminalLaunch>, ApiError> {
    let inventory = state.source.inventory(&session).await?;
    let mut matches = inventory
        .workers
        .iter()
        .filter(|worker| worker.terminal_id == terminal_id);
    let Some(worker) = matches.next() else {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            code: "terminal_not_found",
            message: "The terminal is no longer present in this Herdr session".to_owned(),
        });
    };
    if matches.next().is_some() {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "terminal_ambiguous",
            message: "More than one live worker reported this terminal identity".to_owned(),
        });
    }
    if !worker.interactive_ready {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "terminal_not_ready",
            message: "The worker terminal is not ready for an interactive attachment".to_owned(),
        });
    }

    let ghostty = ghostty_binary();
    let herdr = env::var_os("YARD_HERDR_BIN").unwrap_or_else(|| OsString::from("herdr"));
    let command = vec![
        herdr.to_string_lossy().into_owned(),
        "--session".to_owned(),
        session.clone(),
        "agent".to_owned(),
        "attach".to_owned(),
        terminal_id.clone(),
    ];
    tokio::process::Command::new(&ghostty)
        .arg("-e")
        .args(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| external_terminal_error(&ghostty, &error))?;

    Ok(NoStoreJson(ExternalTerminalLaunch {
        application: "ghostty",
        terminal_id,
        command,
    }))
}

fn ghostty_binary() -> OsString {
    if let Some(binary) = env::var_os("YARD_GHOSTTY_BIN") {
        return binary;
    }
    let macos_binary = PathBuf::from("/Applications/Ghostty.app/Contents/MacOS/ghostty");
    if macos_binary.is_file() {
        return macos_binary.into_os_string();
    }
    OsString::from("ghostty")
}

fn external_terminal_error(binary: &OsString, error: &io::Error) -> ApiError {
    if error.kind() == io::ErrorKind::NotFound {
        return ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "ghostty_unavailable",
            message: format!(
                "Ghostty was not found on the Yard host (tried '{}')",
                binary.to_string_lossy()
            ),
        };
    }
    ApiError {
        status: StatusCode::BAD_GATEWAY,
        code: "ghostty_launch_failed",
        message: format!("Yard could not launch Ghostty: {error}"),
    }
}

async fn list_projects(State(state): State<AppState>) -> Result<NoStoreJson<Projects>, ApiError> {
    state
        .projects
        .list()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn create_project(
    State(state): State<AppState>,
    Json(project): Json<CreateProject>,
) -> Result<CreatedProject, ApiError> {
    state
        .projects
        .create(project)
        .await
        .map(CreatedProject)
        .map_err(ApiError::from)
}

async fn create_project_from_profile(
    State(state): State<AppState>,
    Json(command): Json<CreateProjectFromProfile>,
) -> Result<CreatedProfileProject, ApiError> {
    state
        .projects
        .create_from_profile(command)
        .await
        .map(CreatedProfileProject)
        .map_err(ApiError::from)
}

async fn create_project_with_workspace(
    State(state): State<AppState>,
    Json(command): Json<CreateWorkspaceProjectFromProfile>,
) -> Result<CreatedProfileProject, ApiError> {
    state
        .projects
        .create_with_workspace(command)
        .await
        .map(CreatedProfileProject)
        .map_err(ApiError::from)
}

async fn get_project(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<NoStoreJson<Project>, ApiError> {
    state
        .projects
        .get(&project_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_project_placement(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(update): Json<UpdateProjectPlacement>,
) -> Result<NoStoreJson<Project>, ApiError> {
    state
        .projects
        .update_placement(&project_id, update)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_worker_profiles(
    State(state): State<AppState>,
) -> Result<NoStoreJson<WorkerProfiles>, ApiError> {
    state
        .profiles
        .list()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_workers(
    State(state): State<AppState>,
) -> Result<NoStoreJson<WorkerCandidates>, ApiError> {
    state
        .allocations
        .list_workers()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn end_worker_session(
    State(state): State<AppState>,
    Path(worker_id): Path<String>,
    Json(command): Json<EndWorkerSession>,
) -> Result<NoStoreJson<EndedWorkerSession>, ApiError> {
    state
        .worker_sessions
        .end(&worker_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn create_worker_profile(
    State(state): State<AppState>,
    Json(profile): Json<CreateWorkerProfile>,
) -> Result<CreatedWorkerProfile, ApiError> {
    state
        .profiles
        .create(profile)
        .await
        .map(CreatedWorkerProfile)
        .map_err(ApiError::from)
}

async fn get_worker_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<NoStoreJson<WorkerProfile>, ApiError> {
    state
        .profiles
        .get(&profile_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_worker_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Json(update): Json<UpdateWorkerProfile>,
) -> Result<NoStoreJson<WorkerProfile>, ApiError> {
    state
        .profiles
        .update(&profile_id, update)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_agent_profiles(
    State(state): State<AppState>,
) -> Result<NoStoreJson<AgentProfiles>, ApiError> {
    state
        .profiles
        .list_agents()
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn create_agent_profile(
    State(state): State<AppState>,
    Json(profile): Json<CreateAgentProfile>,
) -> Result<CreatedAgentProfile, ApiError> {
    state
        .profiles
        .create_agent(profile)
        .await
        .map(CreatedAgentProfile)
        .map_err(ApiError::from)
}

async fn get_agent_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<NoStoreJson<AgentProfile>, ApiError> {
    state
        .profiles
        .get_agent(&profile_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_agent_profile_revision(
    State(state): State<AppState>,
    Path((profile_id, profile_version)): Path<(String, String)>,
) -> Result<NoStoreJson<AgentProfile>, ApiError> {
    let profile_version = profile_version
        .parse::<u64>()
        .ok()
        .filter(|version| *version > 0)
        .ok_or(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_agent_profile_revision",
            message: "Agent profile revision must be a positive integer".to_owned(),
        })?;
    state
        .profiles
        .get_agent_revision(&profile_id, profile_version)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn update_agent_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Json(update): Json<UpdateAgentProfile>,
) -> Result<NoStoreJson<AgentProfile>, ApiError> {
    state
        .profiles
        .update_agent(&profile_id, update)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn list_project_assignments(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<NoStoreJson<Assignments>, ApiError> {
    state
        .allocations
        .list_project(&project_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ConfirmAllocation {
    Worker(ConfirmWorkerAllocation),
    Profile(ConfirmProfileAllocation),
}

async fn confirm_allocation(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(command): Json<ConfirmAllocation>,
) -> Result<CreatedAllocation, ApiError> {
    let result = match command {
        ConfirmAllocation::Worker(command) => {
            state.allocations.confirm_worker(&project_id, command).await
        }
        ConfirmAllocation::Profile(command) => {
            state.allocations.confirm(&project_id, command).await
        }
    };
    result.map(CreatedAllocation).map_err(ApiError::from)
}

async fn record_completion_receipt(
    State(state): State<AppState>,
    Path((project_id, assignment_id)): Path<(String, String)>,
    Json(command): Json<RecordCompletionReceipt>,
) -> Result<CreatedCompletionReceipt, ApiError> {
    state
        .allocations
        .complete(&project_id, &assignment_id, command)
        .await
        .map(CreatedCompletionReceipt)
        .map_err(ApiError::from)
}

async fn put_artifact(
    State(state): State<AppState>,
    Path((project_id, assignment_id, artifact_id)): Path<(String, String, String)>,
    Json(upload): Json<UploadArtifact>,
) -> Result<PutArtifactResponse, ApiError> {
    state
        .artifacts
        .put(&project_id, &assignment_id, &artifact_id, upload)
        .await
        .map(PutArtifactResponse)
        .map_err(ApiError::from)
}

async fn get_artifact(
    State(state): State<AppState>,
    Path((project_id, assignment_id, artifact_id)): Path<(String, String, String)>,
) -> Result<NoStoreJson<Artifact>, ApiError> {
    state
        .artifacts
        .get(&project_id, &assignment_id, &artifact_id)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn get_artifact_content(
    State(state): State<AppState>,
    Path((project_id, assignment_id, artifact_id)): Path<(String, String, String)>,
) -> Result<UntrustedJson<ArtifactContent>, ApiError> {
    state
        .artifacts
        .content(&project_id, &assignment_id, &artifact_id)
        .await
        .map(UntrustedJson)
        .map_err(ApiError::from)
}

async fn confirm_worker_handoff(
    State(state): State<AppState>,
    Path((project_id, assignment_id)): Path<(String, String)>,
    Json(command): Json<ConfirmWorkerHandoff>,
) -> Result<CreatedWorkerHandoff, ApiError> {
    state
        .allocations
        .confirm_handoff(&project_id, &assignment_id, command)
        .await
        .map(CreatedWorkerHandoff)
        .map_err(ApiError::from)
}

async fn prompt_assignment(
    State(state): State<AppState>,
    Path((project_id, assignment_id)): Path<(String, String)>,
    Json(command): Json<SendAssignmentPrompt>,
) -> Result<NoStoreJson<PromptAcknowledgement>, ApiError> {
    state
        .interventions
        .prompt(&project_id, &assignment_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn prompt_orchestrator(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(command): Json<SendOrchestratorPrompt>,
) -> Result<NoStoreJson<OrchestratorPromptAcknowledgement>, ApiError> {
    state
        .interventions
        .prompt_orchestrator(&project_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn replace_project_orchestrator(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(command): Json<ReplaceProjectOrchestrator>,
) -> Result<NoStoreJson<ReplacedProjectOrchestrator>, ApiError> {
    state
        .orchestrator_replacements
        .replace_project(&project_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn transfer_project_orchestrator(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(command): Json<TransferProjectOrchestrator>,
) -> Result<NoStoreJson<TransferredProjectOrchestrator>, ApiError> {
    state
        .orchestrator_transfers
        .transfer(&project_id, command)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

#[derive(Debug, serde::Deserialize)]
struct OutputQuery {
    #[serde(default = "default_output_lines")]
    lines: u32,
}

const fn default_output_lines() -> u32 {
    120
}

async fn read_assignment_output(
    State(state): State<AppState>,
    Path((project_id, assignment_id)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<OutputQuery>,
) -> Result<NoStoreJson<TerminalOutput>, ApiError> {
    state
        .interventions
        .read_output(&project_id, &assignment_id, query.lines)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

async fn read_orchestrator_output(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<OutputQuery>,
) -> Result<NoStoreJson<OrchestratorTerminalOutput>, ApiError> {
    state
        .interventions
        .read_orchestrator_output(&project_id, query.lines)
        .await
        .map(NoStoreJson)
        .map_err(ApiError::from)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

struct NoStoreJson<T>(T);

impl<T: Serialize> IntoResponse for NoStoreJson<T> {
    fn into_response(self) -> Response {
        let mut response = Json(self.0).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

struct UntrustedJson<T>(T);

impl<T: Serialize> IntoResponse for UntrustedJson<T> {
    fn into_response(self) -> Response {
        let mut response = NoStoreJson(self.0).into_response();
        response.headers_mut().insert(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
        response.headers_mut().insert(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'self'; sandbox"),
        );
        response
    }
}

struct PutArtifactResponse(StoredArtifact);

impl IntoResponse for PutArtifactResponse {
    fn into_response(self) -> Response {
        let artifact = self.0.artifact;
        if self.0.replayed {
            return NoStoreJson(artifact).into_response();
        }
        let location = format!(
            "/api/v1/projects/{}/assignments/{}/artifacts/{}",
            artifact.project_id, artifact.assignment_id, artifact.id
        );
        created_response(&location, artifact)
    }
}

struct CreatedProject(Project);

impl IntoResponse for CreatedProject {
    fn into_response(self) -> Response {
        let location = format!("/api/v1/projects/{}", self.0.id);
        let mut response = (StatusCode::CREATED, Json(self.0)).into_response();
        response.headers_mut().insert(
            header::LOCATION,
            HeaderValue::from_str(&location).expect("UUID project locations are valid headers"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

struct CreatedWorkerProfile(WorkerProfile);

impl IntoResponse for CreatedWorkerProfile {
    fn into_response(self) -> Response {
        let location = format!("/api/v1/worker-profiles/{}", self.0.id);
        created_response(&location, self.0)
    }
}

struct CreatedAgentProfile(AgentProfile);

impl IntoResponse for CreatedAgentProfile {
    fn into_response(self) -> Response {
        let location = format!("/api/v1/agent-profiles/{}", self.0.id);
        created_response(&location, self.0)
    }
}

struct CreatedProfileProject(ConfirmedProjectCreation);

impl IntoResponse for CreatedProfileProject {
    fn into_response(self) -> Response {
        if self.0.replayed {
            return NoStoreJson(self.0).into_response();
        }
        let location = format!("/api/v1/projects/{}", self.0.project.id);
        created_response(&location, self.0)
    }
}

struct CreatedAllocation(ConfirmedAllocation);

impl IntoResponse for CreatedAllocation {
    fn into_response(self) -> Response {
        if self.0.replayed {
            return NoStoreJson(self.0).into_response();
        }
        let location = format!(
            "/api/v1/projects/{}/assignments",
            self.0.assignment.project_id
        );
        created_response(&location, self.0)
    }
}

struct CreatedWorkerHandoff(ConfirmedWorkerHandoff);

impl IntoResponse for CreatedWorkerHandoff {
    fn into_response(self) -> Response {
        if self.0.replayed {
            return NoStoreJson(self.0).into_response();
        }
        let location = format!(
            "/api/v1/projects/{}/assignments/{}",
            self.0.assignment.project_id, self.0.assignment.id
        );
        created_response(&location, self.0)
    }
}

struct CreatedCompletionReceipt(RecordedCompletionReceipt);

impl IntoResponse for CreatedCompletionReceipt {
    fn into_response(self) -> Response {
        if self.0.replayed {
            return NoStoreJson(self.0).into_response();
        }
        let location = format!(
            "/api/v1/projects/{}/assignments/{}/completion-receipts/{}",
            self.0.assignment.project_id, self.0.assignment.id, self.0.receipt.id
        );
        created_response(&location, self.0)
    }
}

struct CreatedAutomation(AutomationCommandResult);

impl IntoResponse for CreatedAutomation {
    fn into_response(self) -> Response {
        if self.0.replayed {
            return NoStoreJson(self.0).into_response();
        }
        let location = format!("/api/v1/automations/{}", self.0.automation.id);
        created_response(&location, self.0)
    }
}

struct CreatedAutomationRun(AutomationRunCommandResult);

impl IntoResponse for CreatedAutomationRun {
    fn into_response(self) -> Response {
        if self.0.replayed {
            return NoStoreJson(self.0).into_response();
        }
        let location = format!(
            "/api/v1/automations/{}/runs/{}",
            self.0.run.automation_id, self.0.run.id
        );
        created_response(&location, self.0)
    }
}

fn created_response<T: Serialize>(location: &str, body: T) -> Response {
    let mut response = (StatusCode::CREATED, Json(body)).into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("resource locations are valid headers"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl From<InventoryServiceError> for ApiError {
    fn from(error: InventoryServiceError) -> Self {
        match error {
            InventoryServiceError::Herdr(HerdrError::SessionNotFound(message)) => Self {
                status: StatusCode::NOT_FOUND,
                code: "session_not_found",
                message: format!("Herdr session '{message}' was not found"),
            },
            InventoryServiceError::Herdr(
                error @ (HerdrError::DiscoveryIo(_)
                | HerdrError::DiscoveryTimeout
                | HerdrError::DiscoveryResponseTooLarge { .. }
                | HerdrError::DiscoveryFailed { .. }),
            ) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "herdr_unavailable",
                message: error.to_string(),
            },
            InventoryServiceError::Herdr(error) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "herdr_snapshot_failed",
                message: error.to_string(),
            },
        }
    }
}

impl From<ArtifactServiceError> for ApiError {
    fn from(error: ArtifactServiceError) -> Self {
        match error {
            ArtifactServiceError::InvalidArtifact(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_artifact",
                message: error.to_string(),
            },
            ArtifactServiceError::InvalidArtifactId => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_artifact_id",
                message: error.to_string(),
            },
            ArtifactServiceError::ArtifactIdConflict
            | ArtifactServiceError::Store(ProjectStoreError::ArtifactIdConflict) => Self {
                status: StatusCode::CONFLICT,
                code: "artifact_id_conflict",
                message: error.to_string(),
            },
            ArtifactServiceError::Store(ProjectStoreError::ArtifactNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "artifact_not_found",
                message: "Artifact was not found in this assignment".to_owned(),
            },
            ArtifactServiceError::ContentMissing
            | ArtifactServiceError::ContentInvalid
            | ArtifactServiceError::ContentMismatch
            | ArtifactServiceError::ContentNotUtf8 => Self {
                status: StatusCode::CONFLICT,
                code: "artifact_content_unavailable",
                message: error.to_string(),
            },
            ArtifactServiceError::Store(error) => allocation_store_error(error),
            ArtifactServiceError::Io(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "artifact_storage_error",
                message: "Yard artifact storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<AutomationServiceError> for ApiError {
    fn from(error: AutomationServiceError) -> Self {
        match error {
            AutomationServiceError::InvalidCommand(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_automation_command",
                message: error.to_string(),
            },
            AutomationServiceError::InvalidRunLimit
            | AutomationServiceError::InvalidTimezone
            | AutomationServiceError::InvalidTimestamp
            | AutomationServiceError::ScheduleUnavailable => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_automation_schedule",
                message: error.to_string(),
            },
            AutomationServiceError::MissingNextRun => Self {
                status: StatusCode::CONFLICT,
                code: "automation_schedule_unavailable",
                message: error.to_string(),
            },
            AutomationServiceError::ScopeNotProvisioned => Self {
                status: StatusCode::CONFLICT,
                code: "automation_scope_not_provisioned",
                message: error.to_string(),
            },
            AutomationServiceError::Store(error) => automation_store_error(error),
        }
    }
}

impl From<ReconciliationServiceError> for ApiError {
    fn from(error: ReconciliationServiceError) -> Self {
        match error {
            ReconciliationServiceError::Inventory(error) => Self::from(error),
            ReconciliationServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            ReconciliationServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "runtime_reconciliation_failed",
                message: "Yard could not reconcile the Herdr snapshot".to_owned(),
            },
        }
    }
}

#[allow(clippy::too_many_lines)]
impl From<ProjectServiceError> for ApiError {
    fn from(error: ProjectServiceError) -> Self {
        match error {
            ProjectServiceError::InvalidProject(error)
            | ProjectServiceError::Store(ProjectStoreError::InvalidProject(error)) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_project",
                message: error.to_string(),
            },
            ProjectServiceError::UnsupportedRuntimeAdapter(adapter) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "unsupported_runtime_adapter",
                message: format!("Runtime adapter '{adapter}' is not supported"),
            },
            ProjectServiceError::UnsupportedProfile(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "unsupported_worker_profile",
                message,
            },
            ProjectServiceError::RuntimeCwdUnavailable => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "runtime_cwd_unavailable",
                message: error.to_string(),
            },
            ProjectServiceError::RuntimeProvision(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "worker_provision_failed",
                message,
            },
            ProjectServiceError::RuntimeProvisionAmbiguous(message) => Self {
                status: StatusCode::CONFLICT,
                code: "command_outcome_ambiguous",
                message,
            },
            ProjectServiceError::RuntimeBindingUnverified(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "runtime_binding_unverified",
                message,
            },
            ProjectServiceError::ObjectiveDeliveryFailed(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "orchestrator_objective_delivery_failed",
                message,
            },
            error @ (ProjectServiceError::RuntimeWorkspaceNotFound { .. }
            | ProjectServiceError::RuntimeWorkerNotFound { .. }
            | ProjectServiceError::OrchestratorOutsideWorkspace { .. }) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_binding_stale",
                message: error.to_string(),
            },
            ProjectServiceError::Inventory(error) => Self::from(error),
            ProjectServiceError::Store(ProjectStoreError::RuntimeWorkspaceAlreadyBound) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_workspace_already_bound",
                message: "The Herdr workspace is already bound to a Yard project".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::RuntimeWorkspaceReserved) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_workspace_reserved",
                message: "The Herdr workspace is reserved by another project creation".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::RuntimeWorkerAlreadyBound) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_worker_already_bound",
                message: "The observed worker is already bound to a Yard worker".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::RuntimeBindingAlreadyExists) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_binding_conflict",
                message: "A runtime binding was created concurrently".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::StaleRuntimeSnapshot) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_binding_stale",
                message: "The Herdr snapshot is older than Yard's durable runtime state".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::ProjectNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "project_not_found",
                message: "Yard project was not found".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::VersionConflict { current_version }) => {
                Self {
                    status: StatusCode::CONFLICT,
                    code: "project_version_conflict",
                    message: format!(
                        "Project placement changed concurrently; current version is \
                     {current_version}"
                    ),
                }
            }
            ProjectServiceError::Store(ProjectStoreError::ProfileNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "worker_profile_not_found",
                message: "Worker profile was not found".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::ProfileVersionConflict {
                current_version,
            }) => Self {
                status: StatusCode::CONFLICT,
                code: "worker_profile_version_conflict",
                message: format!(
                    "Worker profile changed concurrently; current version is {current_version}"
                ),
            },
            ProjectServiceError::Store(ProjectStoreError::IdempotencyConflict) => Self {
                status: StatusCode::CONFLICT,
                code: "idempotency_conflict",
                message: "Command ID is already associated with different input".to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::CommandOutcomeAmbiguous(message)) => {
                Self {
                    status: StatusCode::CONFLICT,
                    code: "command_outcome_ambiguous",
                    message,
                }
            }
            ProjectServiceError::Store(ProjectStoreError::CommandPreviouslyFailed(message)) => {
                Self {
                    status: StatusCode::CONFLICT,
                    code: "command_previously_failed",
                    message,
                }
            }
            ProjectServiceError::Store(ProjectStoreError::RuntimeWorkspaceMismatch) => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_workspace_mismatch",
                message: "The provisioned worker does not belong to the reserved workspace"
                    .to_owned(),
            },
            ProjectServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            ProjectServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: "Yard storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<OrchestratorReplacementServiceError> for ApiError {
    fn from(error: OrchestratorReplacementServiceError) -> Self {
        match error {
            OrchestratorReplacementServiceError::InvalidCommand(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_orchestrator_replacement",
                message: error.to_string(),
            },
            OrchestratorReplacementServiceError::UnsupportedProfile(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "unsupported_worker_profile",
                message,
            },
            OrchestratorReplacementServiceError::RuntimeWorkspaceMissing(_)
            | OrchestratorReplacementServiceError::RuntimeIdentityChanged
            | OrchestratorReplacementServiceError::ReplacementUnverified => Self {
                status: StatusCode::CONFLICT,
                code: "orchestrator_replacement_identity_changed",
                message: error.to_string(),
            },
            OrchestratorReplacementServiceError::RuntimeCwdUnavailable => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "runtime_cwd_unavailable",
                message: error.to_string(),
            },
            OrchestratorReplacementServiceError::RuntimeProvision(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "orchestrator_replacement_provision_failed",
                message,
            },
            OrchestratorReplacementServiceError::ObjectiveDeliveryFailed(message) => Self {
                status: StatusCode::CONFLICT,
                code: "command_outcome_ambiguous",
                message,
            },
            OrchestratorReplacementServiceError::Inventory(error) => Self::from(error),
            OrchestratorReplacementServiceError::Store(ProjectStoreError::ProjectNotFound) => {
                Self {
                    status: StatusCode::NOT_FOUND,
                    code: "project_not_found",
                    message: "Yard project was not found".to_owned(),
                }
            }
            OrchestratorReplacementServiceError::Store(ProjectStoreError::ProfileNotFound) => {
                Self {
                    status: StatusCode::NOT_FOUND,
                    code: "worker_profile_not_found",
                    message: "Worker profile was not found".to_owned(),
                }
            }
            OrchestratorReplacementServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            OrchestratorReplacementServiceError::Store(
                error @ (ProjectStoreError::ProjectVersionConflict { .. }
                | ProjectStoreError::WorkerVersionConflict { .. }
                | ProjectStoreError::ProfileVersionConflict { .. }
                | ProjectStoreError::OrchestratorNotCurrent { .. }
                | ProjectStoreError::OrchestratorInterventionInProgress
                | ProjectStoreError::OrchestratorReplacementReserved
                | ProjectStoreError::OrchestratorReplacementTargetChanged
                | ProjectStoreError::OrchestratorReplacementRuntimeMissing
                | ProjectStoreError::OrchestratorReplacementRuntimeConflict
                | ProjectStoreError::RuntimeBindingMissing
                | ProjectStoreError::RuntimeWorkspaceMismatch
                | ProjectStoreError::RuntimeWorkerAlreadyBound
                | ProjectStoreError::StaleRuntimeSnapshot
                | ProjectStoreError::IdempotencyConflict
                | ProjectStoreError::CommandInProgress
                | ProjectStoreError::CommandPreviouslyFailed(_)
                | ProjectStoreError::CommandOutcomeAmbiguous(_)),
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "orchestrator_replacement_conflict",
                message: error.to_string(),
            },
            OrchestratorReplacementServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "orchestrator_replacement_storage_error",
                message: "Yard orchestrator replacement storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<ProjectOrchestratorTransferServiceError> for ApiError {
    fn from(error: ProjectOrchestratorTransferServiceError) -> Self {
        match error {
            ProjectOrchestratorTransferServiceError::InvalidCommand(error)
            | ProjectOrchestratorTransferServiceError::Store(
                ProjectStoreError::InvalidOrchestratorTransfer(error),
            ) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_project_orchestrator_transfer",
                message: error.to_string(),
            },
            ProjectOrchestratorTransferServiceError::RuntimeWorkspaceMismatch
            | ProjectOrchestratorTransferServiceError::RuntimeWorkspaceMissing
            | ProjectOrchestratorTransferServiceError::RuntimeUnavailable
            | ProjectOrchestratorTransferServiceError::RuntimeIdentityChanged
            | ProjectOrchestratorTransferServiceError::RuntimeIdentityAmbiguous => Self {
                status: StatusCode::CONFLICT,
                code: "project_orchestrator_identity_changed",
                message: error.to_string(),
            },
            ProjectOrchestratorTransferServiceError::Inventory(error) => Self::from(error),
            ProjectOrchestratorTransferServiceError::Store(ProjectStoreError::ProjectNotFound) => {
                Self {
                    status: StatusCode::NOT_FOUND,
                    code: "project_not_found",
                    message: "Yard project was not found".to_owned(),
                }
            }
            ProjectOrchestratorTransferServiceError::Store(ProjectStoreError::WorkerNotFound) => {
                Self {
                    status: StatusCode::NOT_FOUND,
                    code: "worker_not_found",
                    message: "Yard worker was not found".to_owned(),
                }
            }
            ProjectOrchestratorTransferServiceError::Store(ProjectStoreError::DatabaseBusy) => {
                Self {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    code: "database_busy",
                    message: "Yard storage is busy; retry the request".to_owned(),
                }
            }
            ProjectOrchestratorTransferServiceError::Store(
                error @ (ProjectStoreError::ProjectVersionConflict { .. }
                | ProjectStoreError::WorkerVersionConflict { .. }
                | ProjectStoreError::OrchestratorNotCurrent { .. }
                | ProjectStoreError::WorkerNotAvailable { .. }
                | ProjectStoreError::RuntimeBindingMissing
                | ProjectStoreError::RuntimeWorkspaceMismatch
                | ProjectStoreError::StaleRuntimeSnapshot
                | ProjectStoreError::OrchestratorInterventionInProgress
                | ProjectStoreError::OrchestratorTransferTargetChanged
                | ProjectStoreError::IdempotencyConflict),
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "project_orchestrator_transfer_conflict",
                message: error.to_string(),
            },
            ProjectOrchestratorTransferServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "project_orchestrator_transfer_storage_error",
                message: "Yard project orchestrator transfer storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<OrchestratorWorkflowProfileServiceError> for ApiError {
    fn from(error: OrchestratorWorkflowProfileServiceError) -> Self {
        match error {
            OrchestratorWorkflowProfileServiceError::InvalidProfile(error)
            | OrchestratorWorkflowProfileServiceError::Store(
                ProjectStoreError::InvalidOrchestratorWorkflowProfile(error),
            ) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_orchestrator_workflow_profile",
                message: error.to_string(),
            },
            OrchestratorWorkflowProfileServiceError::Store(
                ProjectStoreError::OrchestratorWorkflowProfileVersionConflict { current_version },
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "orchestrator_workflow_profile_version_conflict",
                message: format!(
                    "Orchestrator workflow profile changed concurrently; current version is \
                     {current_version}"
                ),
            },
            OrchestratorWorkflowProfileServiceError::Store(
                ProjectStoreError::OrchestratorWorkflowProfileNotFound,
            ) => Self {
                status: StatusCode::NOT_FOUND,
                code: "orchestrator_workflow_profile_not_found",
                message: "Orchestrator workflow profile revision was not found".to_owned(),
            },
            OrchestratorWorkflowProfileServiceError::Store(ProjectStoreError::DatabaseBusy) => {
                Self {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    code: "database_busy",
                    message: "Yard storage is busy; retry the request".to_owned(),
                }
            }
            OrchestratorWorkflowProfileServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: "Orchestrator workflow profile storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<ProfileServiceError> for ApiError {
    fn from(error: ProfileServiceError) -> Self {
        match error {
            ProfileServiceError::InvalidProfile(error)
            | ProfileServiceError::Store(ProjectStoreError::InvalidProfile(error)) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_worker_profile",
                message: error.to_string(),
            },
            ProfileServiceError::Store(ProjectStoreError::InvalidAgentProfile(error)) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_worker_profile",
                message: error.to_string(),
            },
            ProfileServiceError::Store(ProjectStoreError::ProfileNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "worker_profile_not_found",
                message: "Worker profile was not found".to_owned(),
            },
            ProfileServiceError::Store(ProjectStoreError::ProfileNameAlreadyExists) => Self {
                status: StatusCode::CONFLICT,
                code: "worker_profile_name_conflict",
                message: "A worker profile with this name already exists".to_owned(),
            },
            ProfileServiceError::Store(ProjectStoreError::ProfileVersionConflict {
                current_version,
            }) => Self {
                status: StatusCode::CONFLICT,
                code: "worker_profile_version_conflict",
                message: format!(
                    "Worker profile changed concurrently; current version is {current_version}"
                ),
            },
            ProfileServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            ProfileServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: "Yard storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<AgentProfileServiceError> for ApiError {
    fn from(error: AgentProfileServiceError) -> Self {
        match error {
            AgentProfileServiceError::InvalidProfile(error)
            | AgentProfileServiceError::Store(ProjectStoreError::InvalidAgentProfile(error)) => {
                Self {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: "invalid_agent_profile",
                    message: error.to_string(),
                }
            }
            AgentProfileServiceError::Store(ProjectStoreError::ProfileNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "agent_profile_not_found",
                message: "Agent profile revision was not found".to_owned(),
            },
            AgentProfileServiceError::Store(ProjectStoreError::ProfileNameAlreadyExists) => Self {
                status: StatusCode::CONFLICT,
                code: "agent_profile_name_conflict",
                message: "An agent profile with this name already exists".to_owned(),
            },
            AgentProfileServiceError::Store(ProjectStoreError::ProfileVersionConflict {
                current_version,
            }) => Self {
                status: StatusCode::CONFLICT,
                code: "agent_profile_version_conflict",
                message: format!(
                    "Agent profile changed concurrently; current version is {current_version}"
                ),
            },
            AgentProfileServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            AgentProfileServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: "Yard storage is unavailable".to_owned(),
            },
        }
    }
}

impl From<AllocationServiceError> for ApiError {
    fn from(error: AllocationServiceError) -> Self {
        match error {
            AllocationServiceError::InvalidAssignment(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_assignment",
                message: error.to_string(),
            },
            AllocationServiceError::UnsupportedProfile(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "unsupported_worker_profile",
                message,
            },
            AllocationServiceError::RuntimeWorkspaceMissing { workspace_id } => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_binding_stale",
                message: format!(
                    "Bound Herdr workspace '{workspace_id}' is not currently observed"
                ),
            },
            AllocationServiceError::RuntimeCwdUnavailable => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "runtime_cwd_unavailable",
                message: "Herdr did not report a working directory for the project workspace"
                    .to_owned(),
            },
            AllocationServiceError::RuntimeProvision(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "worker_provision_failed",
                message,
            },
            AllocationServiceError::RuntimeBindingUnverified(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "runtime_binding_unverified",
                message,
            },
            AllocationServiceError::ObjectiveDeliveryFailed(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "assignment_delivery_failed",
                message,
            },
            AllocationServiceError::RuntimeIntervention(error) => runtime_intervention_error(error),
            AllocationServiceError::Inventory(error) => Self::from(error),
            AllocationServiceError::Store(error) => allocation_store_error(error),
        }
    }
}

impl From<CoordinationNodeServiceError> for ApiError {
    fn from(error: CoordinationNodeServiceError) -> Self {
        match error {
            CoordinationNodeServiceError::InvalidCommand(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_coordination_node_command",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::WrongNodeKind => Self {
                status: StatusCode::CONFLICT,
                code: "coordination_node_kind_mismatch",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::NodeNotProvisioned => Self {
                status: StatusCode::CONFLICT,
                code: "coordination_node_not_provisioned",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::AutomaticTokenSpendDisabled => Self {
                status: StatusCode::CONFLICT,
                code: "automatic_token_spend_disabled",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::NodeChanged
            | CoordinationNodeServiceError::ProjectOrchestratorChanged
            | CoordinationNodeServiceError::RuntimeBindingMissing
            | CoordinationNodeServiceError::RuntimeBindingStale => Self {
                status: StatusCode::CONFLICT,
                code: "coordination_binding_stale",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::InvalidLineCount => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_output_lines",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::UnsupportedProfile(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "unsupported_worker_profile",
                message,
            },
            CoordinationNodeServiceError::Runtime(error) => runtime_intervention_error(error),
            CoordinationNodeServiceError::Inventory(error)
            | CoordinationNodeServiceError::Reconciliation(
                ReconciliationServiceError::Inventory(error),
            ) => Self::from(error),
            CoordinationNodeServiceError::RuntimeProvision(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "coordination_node_provision_failed",
                message,
            },
            CoordinationNodeServiceError::RuntimeBindingUnverified
            | CoordinationNodeServiceError::ReconciledWorkerMissing => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "runtime_binding_unverified",
                message: error.to_string(),
            },
            CoordinationNodeServiceError::ObjectiveDeliveryFailed(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "coordination_objective_delivery_failed",
                message,
            },
            CoordinationNodeServiceError::Reconciliation(ReconciliationServiceError::Store(
                error,
            ))
            | CoordinationNodeServiceError::Store(error) => coordination_node_store_error(error),
            CoordinationNodeServiceError::Io(_)
            | CoordinationNodeServiceError::ManagedPathMissing
            | CoordinationNodeServiceError::ManagedRootNotAbsolute
            | CoordinationNodeServiceError::ManagedPathNotUtf8
            | CoordinationNodeServiceError::UnsafeManagedPath => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "coordination_managed_path_error",
                message: error.to_string(),
            },
        }
    }
}

impl From<YardOrchestratorServiceError> for ApiError {
    fn from(error: YardOrchestratorServiceError) -> Self {
        match error {
            YardOrchestratorServiceError::InvalidCommand(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_yard_orchestrator_command",
                message: error.to_string(),
            },
            YardOrchestratorServiceError::UnsupportedProfile(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "unsupported_worker_profile",
                message,
            },
            YardOrchestratorServiceError::Store(ProjectStoreError::ProfileNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "worker_profile_not_found",
                message: "Worker profile was not found".to_owned(),
            },
            YardOrchestratorServiceError::Store(
                error @ (ProjectStoreError::ProfileVersionConflict { .. }
                | ProjectStoreError::WorkerVersionConflict { .. }
                | ProjectStoreError::YardOrchestratorVersionConflict { .. }
                | ProjectStoreError::WorkerNotAvailable { .. }
                | ProjectStoreError::WorkerProfileAlreadyPinned
                | ProjectStoreError::IdempotencyConflict),
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "yard_orchestrator_provision_conflict",
                message: error.to_string(),
            },
            YardOrchestratorServiceError::RuntimeProvision(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "yard_orchestrator_provision_failed",
                message,
            },
            YardOrchestratorServiceError::RuntimeBindingUnverified
            | YardOrchestratorServiceError::ReconciledWorkerMissing => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "runtime_binding_unverified",
                message: error.to_string(),
            },
            YardOrchestratorServiceError::RecoveryNotConfigured
            | YardOrchestratorServiceError::RecoveryNotDedicated => Self {
                status: StatusCode::CONFLICT,
                code: "yard_orchestrator_recovery_unavailable",
                message: error.to_string(),
            },
            YardOrchestratorServiceError::RecoveryBindingMissing
            | YardOrchestratorServiceError::RecoveryBindingAmbiguous
            | YardOrchestratorServiceError::RecoveryOwnershipChanged => Self {
                status: StatusCode::CONFLICT,
                code: "yard_orchestrator_recovery_conflict",
                message: error.to_string(),
            },
            YardOrchestratorServiceError::ObjectiveDeliveryFailed(message) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "yard_orchestrator_prompt_failed",
                message,
            },
            YardOrchestratorServiceError::Reconciliation(
                ReconciliationServiceError::Inventory(_),
            ) => Self {
                status: StatusCode::BAD_GATEWAY,
                code: "runtime_inventory_unavailable",
                message: error.to_string(),
            },
            YardOrchestratorServiceError::Reconciliation(ReconciliationServiceError::Store(
                ProjectStoreError::DatabaseBusy,
            ))
            | YardOrchestratorServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            YardOrchestratorServiceError::Reconciliation(_)
            | YardOrchestratorServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: "Yard orchestrator provisioning is unavailable".to_owned(),
            },
        }
    }
}

impl From<WorkerSessionServiceError> for ApiError {
    fn from(error: WorkerSessionServiceError) -> Self {
        match error {
            WorkerSessionServiceError::Store(ProjectStoreError::InvalidWorkerSession(error)) => {
                Self {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: "invalid_worker_session_command",
                    message: error.to_string(),
                }
            }
            WorkerSessionServiceError::Store(ProjectStoreError::WorkerNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "worker_not_found",
                message: "Worker was not found".to_owned(),
            },
            WorkerSessionServiceError::Store(
                store_error @ ProjectStoreError::WorkerAlreadyEnded,
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "worker_session_already_ended",
                message: store_error.to_string(),
            },
            WorkerSessionServiceError::Store(
                store_error @ ProjectStoreError::OrchestratorSessionEndForbidden,
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "orchestrator_replacement_required",
                message: store_error.to_string(),
            },
            WorkerSessionServiceError::Store(
                store_error @ ProjectStoreError::YardOrchestratorSessionEndForbidden,
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "yard_orchestrator_replacement_required",
                message: store_error.to_string(),
            },
            WorkerSessionServiceError::Store(
                store_error @ ProjectStoreError::CoordinationNodeSessionEndForbidden,
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "coordination_node_worker_protected",
                message: store_error.to_string(),
            },
            WorkerSessionServiceError::Store(
                store_error @ ProjectStoreError::WorkerHasActiveAllocation,
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "worker_has_active_allocation",
                message: store_error.to_string(),
            },
            WorkerSessionServiceError::Store(
                error @ (ProjectStoreError::WorkerVersionConflict { .. }
                | ProjectStoreError::WorkerRuntimeVersionConflict { .. }),
            ) => Self {
                status: StatusCode::CONFLICT,
                code: "worker_version_conflict",
                message: error.to_string(),
            },
            WorkerSessionServiceError::Store(ProjectStoreError::IdempotencyConflict) => Self {
                status: StatusCode::CONFLICT,
                code: "idempotency_conflict",
                message: "Command ID is already associated with different input".to_owned(),
            },
            WorkerSessionServiceError::Store(ProjectStoreError::DatabaseBusy) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_busy",
                message: "Yard storage is busy; retry the request".to_owned(),
            },
            WorkerSessionServiceError::Store(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: "Yard could not end the worker session".to_owned(),
            },
        }
    }
}

impl From<InterventionServiceError> for ApiError {
    fn from(error: InterventionServiceError) -> Self {
        match error {
            InterventionServiceError::InvalidCommand(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_intervention",
                message: error.to_string(),
            },
            InterventionServiceError::InvalidLineCount => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_output_lines",
                message: error.to_string(),
            },
            InterventionServiceError::AssignmentNotFound
            | InterventionServiceError::Store(ProjectStoreError::AssignmentNotFound) => Self {
                status: StatusCode::NOT_FOUND,
                code: "assignment_not_found",
                message: "Assignment was not found in this Yard project".to_owned(),
            },
            InterventionServiceError::AssignmentNotActive => Self {
                status: StatusCode::CONFLICT,
                code: "assignment_not_active",
                message: error.to_string(),
            },
            InterventionServiceError::AssignmentVersionConflict { .. }
            | InterventionServiceError::AttemptVersionConflict { .. }
            | InterventionServiceError::AttemptNotCurrent { .. } => Self {
                status: StatusCode::CONFLICT,
                code: "assignment_version_conflict",
                message: error.to_string(),
            },
            InterventionServiceError::RuntimeBindingMissing
            | InterventionServiceError::RuntimeBindingStale => Self {
                status: StatusCode::CONFLICT,
                code: "runtime_binding_stale",
                message: error.to_string(),
            },
            InterventionServiceError::OrchestratorChanged => Self {
                status: StatusCode::CONFLICT,
                code: "orchestrator_changed",
                message: error.to_string(),
            },
            InterventionServiceError::YardOrchestratorChanged => Self {
                status: StatusCode::CONFLICT,
                code: "yard_orchestrator_changed",
                message: error.to_string(),
            },
            InterventionServiceError::AutomaticTokenSpendDisabled => Self {
                status: StatusCode::CONFLICT,
                code: "automatic_token_spend_disabled",
                message: error.to_string(),
            },
            InterventionServiceError::Runtime(error) => runtime_intervention_error(error),
            InterventionServiceError::Inventory(error) => Self::from(error),
            InterventionServiceError::Store(error) => intervention_store_error(error),
        }
    }
}

fn runtime_intervention_error(error: RuntimeInterventionError) -> ApiError {
    let (status, code, message) = match error {
        RuntimeInterventionError::Rejected(message) => (
            StatusCode::CONFLICT,
            "runtime_intervention_rejected",
            message,
        ),
        RuntimeInterventionError::Unavailable(message) => (
            StatusCode::BAD_GATEWAY,
            "runtime_intervention_unavailable",
            message,
        ),
        RuntimeInterventionError::Ambiguous(message) => (
            StatusCode::BAD_GATEWAY,
            "runtime_intervention_ambiguous",
            message,
        ),
    };
    ApiError {
        status,
        code,
        message,
    }
}

fn yard_orchestrator_store_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::InvalidYardOrchestrator(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_yard_orchestrator_command",
            message: error.to_string(),
        },
        ProjectStoreError::WorkerNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "worker_not_found",
            message: "Worker was not found".to_owned(),
        },
        ProjectStoreError::WorkerNotAvailable { availability } => ApiError {
            status: StatusCode::CONFLICT,
            code: "worker_not_available",
            message: format!(
                "Worker cannot become the Yard orchestrator while availability is {availability:?}"
            ),
        },
        error @ (ProjectStoreError::WorkerVersionConflict { .. }
        | ProjectStoreError::YardOrchestratorVersionConflict { .. }) => ApiError {
            status: StatusCode::CONFLICT,
            code: "yard_orchestrator_version_conflict",
            message: error.to_string(),
        },
        ProjectStoreError::YardOrchestratorInterventionInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "yard_orchestrator_intervention_in_progress",
            message: "Wait for the pending Yard orchestrator intervention".to_owned(),
        },
        ProjectStoreError::RuntimeBindingMissing => ApiError {
            status: StatusCode::CONFLICT,
            code: "runtime_binding_stale",
            message: "The selected worker has no runtime binding".to_owned(),
        },
        ProjectStoreError::IdempotencyConflict => ApiError {
            status: StatusCode::CONFLICT,
            code: "idempotency_conflict",
            message: "Command ID is already associated with different input".to_owned(),
        },
        ProjectStoreError::DatabaseBusy => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_busy",
            message: "Yard storage is busy; retry the request".to_owned(),
        },
        _ => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "Yard orchestrator storage is unavailable".to_owned(),
        },
    }
}

fn coordination_store_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::InvalidCoordination(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_coordination_command",
            message: error.to_string(),
        },
        ProjectStoreError::RelationshipSourceProjectNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "relationship_source_not_found",
            message: "The source Yard project was not found".to_owned(),
        },
        ProjectStoreError::RelationshipTargetProjectNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "relationship_target_not_found",
            message: "The target Yard project was not found".to_owned(),
        },
        ProjectStoreError::ProjectRelationshipNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "project_relationship_not_found",
            message: "Project relationship was not found".to_owned(),
        },
        error @ (ProjectStoreError::ProjectRelationshipIdConflict
        | ProjectStoreError::ProjectRelationshipAlreadyExists
        | ProjectStoreError::ProjectRelationshipVersionConflict { .. }
        | ProjectStoreError::IdempotencyConflict) => ApiError {
            status: StatusCode::CONFLICT,
            code: "project_relationship_conflict",
            message: error.to_string(),
        },
        ProjectStoreError::DatabaseBusy => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_busy",
            message: "Yard storage is busy; retry the request".to_owned(),
        },
        _ => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "Yard coordination storage is unavailable".to_owned(),
        },
    }
}

fn coordination_node_store_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::InvalidCoordinationNode(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_coordination_node_command",
            message: error.to_string(),
        },
        ProjectStoreError::CoordinationNodeNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "coordination_node_not_found",
            message: error.to_string(),
        },
        ProjectStoreError::CoordinationSnapshotNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "coordination_snapshot_not_found",
            message: error.to_string(),
        },
        ProjectStoreError::AttachedProjectNotFound { .. }
        | ProjectStoreError::ProjectNotFound
        | ProjectStoreError::ProfileNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "coordination_reference_not_found",
            message: error.to_string(),
        },
        ProjectStoreError::DatabaseBusy => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_busy",
            message: "Yard storage is busy; retry the request".to_owned(),
        },
        error @ (ProjectStoreError::CoordinationNodeIdConflict
        | ProjectStoreError::CoordinationNodeKindMismatch
        | ProjectStoreError::CoordinationNodeVersionConflict { .. }
        | ProjectStoreError::CoordinationNodePlacementVersionConflict { .. }
        | ProjectStoreError::CoordinationNodeAlreadyProvisioned
        | ProjectStoreError::CoordinationNodeNotProvisioned
        | ProjectStoreError::CoordinationNodeWorkerChanged { .. }
        | ProjectStoreError::CoordinationNodeInterventionInProgress
        | ProjectStoreError::CoordinationNodeProjectNotAttached
        | ProjectStoreError::CoordinationSnapshotIdConflict
        | ProjectStoreError::SnapshotAttachmentMismatch
        | ProjectStoreError::SnapshotProjectNotFound
        | ProjectStoreError::WorkerNotAvailable { .. }
        | ProjectStoreError::WorkerVersionConflict { .. }
        | ProjectStoreError::WorkerProfileAlreadyPinned
        | ProjectStoreError::WorkerProfileRevisionMismatch
        | ProjectStoreError::ProfileVersionConflict { .. }
        | ProjectStoreError::ProjectVersionConflict { .. }
        | ProjectStoreError::OrchestratorNotCurrent { .. }
        | ProjectStoreError::IdempotencyConflict
        | ProjectStoreError::CommandInProgress
        | ProjectStoreError::CommandPreviouslyFailed(_)
        | ProjectStoreError::CommandOutcomeAmbiguous(_)) => ApiError {
            status: StatusCode::CONFLICT,
            code: "coordination_node_conflict",
            message: error.to_string(),
        },
        _ => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "coordination_storage_error",
            message: "Yard coordination storage is unavailable".to_owned(),
        },
    }
}

fn automation_store_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::InvalidTokenSpendSettings(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_token_spend_settings",
            message: error.to_string(),
        },
        error @ ProjectStoreError::TokenSpendSettingsVersionConflict { .. } => ApiError {
            status: StatusCode::CONFLICT,
            code: "token_spend_settings_conflict",
            message: error.to_string(),
        },
        ProjectStoreError::AutomaticSummaryTargetInvalid => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_automatic_summary_target",
            message: error.to_string(),
        },
        ProjectStoreError::InvalidAutomation(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_automation_command",
            message: error.to_string(),
        },
        ProjectStoreError::AutomationNotFound
        | ProjectStoreError::AutomationRunNotFound
        | ProjectStoreError::AutomationScopeProjectNotFound
        | ProjectStoreError::AutomationScopeNodeNotFound
        | ProjectStoreError::AutomationSelectedProjectNotFound { .. }
        | ProjectStoreError::ProjectNotFound
        | ProjectStoreError::CoordinationNodeNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "automation_not_found",
            message: error.to_string(),
        },
        ProjectStoreError::DatabaseBusy => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_busy",
            message: "Yard storage is busy; retry the request".to_owned(),
        },
        error @ (ProjectStoreError::AutomationIdConflict
        | ProjectStoreError::AutomationVersionConflict { .. }
        | ProjectStoreError::AutomationPlacementVersionConflict { .. }
        | ProjectStoreError::AutomationScopeNodeKindMismatch
        | ProjectStoreError::AutomationProjectNotAttached
        | ProjectStoreError::AutomationNextRunInvalid
        | ProjectStoreError::AutomationPaused
        | ProjectStoreError::AutomationScheduleConflict { .. }
        | ProjectStoreError::AutomationRunIdConflict
        | ProjectStoreError::AutomationDispatchCommandIdConflict
        | ProjectStoreError::AutomationRunInProgress
        | ProjectStoreError::ScheduledAutomaticSummariesDisabled
        | ProjectStoreError::IdempotencyConflict
        | ProjectStoreError::CommandInProgress
        | ProjectStoreError::CommandPreviouslyFailed(_)
        | ProjectStoreError::CommandOutcomeAmbiguous(_)) => ApiError {
            status: StatusCode::CONFLICT,
            code: "automation_conflict",
            message: error.to_string(),
        },
        ProjectStoreError::AutomationListLimitInvalid { .. }
        | ProjectStoreError::AutomationSubmittedAtInvalid => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_automation_request",
            message: error.to_string(),
        },
        _ => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "automation_storage_error",
            message: "Yard automation storage is unavailable".to_owned(),
        },
    }
}

#[allow(clippy::too_many_lines)]
fn intervention_store_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::InvalidIntervention(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_intervention",
            message: error.to_string(),
        },
        ProjectStoreError::InvalidYardOrchestrator(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_yard_orchestrator_command",
            message: error.to_string(),
        },
        ProjectStoreError::InvalidCoordination(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_coordination_command",
            message: error.to_string(),
        },
        ProjectStoreError::AssignmentNotActive | ProjectStoreError::AttemptNotActive => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_not_active",
            message: "Only the active assignment attempt can receive prompts".to_owned(),
        },
        error @ (ProjectStoreError::AssignmentVersionConflict { .. }
        | ProjectStoreError::AttemptVersionConflict { .. }
        | ProjectStoreError::AttemptNotCurrent { .. }) => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_version_conflict",
            message: error.to_string(),
        },
        error @ (ProjectStoreError::ProjectVersionConflict { .. }
        | ProjectStoreError::WorkerVersionConflict { .. }
        | ProjectStoreError::YardOrchestratorVersionConflict { .. }) => ApiError {
            status: StatusCode::CONFLICT,
            code: "orchestrator_version_conflict",
            message: error.to_string(),
        },
        ProjectStoreError::OrchestratorNotCurrent { current_worker_id } => ApiError {
            status: StatusCode::CONFLICT,
            code: "orchestrator_changed",
            message: format!("Project orchestrator changed; current worker is {current_worker_id}"),
        },
        ProjectStoreError::OrchestratorInterventionInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "orchestrator_intervention_in_progress",
            message: "Wait for the pending orchestrator intervention or replacement".to_owned(),
        },
        ProjectStoreError::YardOrchestratorNotCurrent { current_worker_id } => ApiError {
            status: StatusCode::CONFLICT,
            code: "yard_orchestrator_changed",
            message: format!("Yard orchestrator changed; current worker is {current_worker_id}"),
        },
        ProjectStoreError::YardOrchestratorNotConfigured => ApiError {
            status: StatusCode::CONFLICT,
            code: "yard_orchestrator_not_configured",
            message: "Configure the Yard orchestrator before sending commands".to_owned(),
        },
        ProjectStoreError::YardOrchestratorInterventionInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "yard_orchestrator_intervention_in_progress",
            message: "Wait for the pending Yard orchestrator intervention".to_owned(),
        },
        ProjectStoreError::RuntimeBindingMissing => ApiError {
            status: StatusCode::CONFLICT,
            code: "runtime_binding_stale",
            message: "Worker has no runtime binding".to_owned(),
        },
        ProjectStoreError::IdempotencyConflict => ApiError {
            status: StatusCode::CONFLICT,
            code: "idempotency_conflict",
            message: "Command ID is already associated with different input".to_owned(),
        },
        ProjectStoreError::CommandPreviouslyFailed(message) => ApiError {
            status: StatusCode::CONFLICT,
            code: "command_previously_failed",
            message,
        },
        ProjectStoreError::CommandOutcomeAmbiguous(message) => ApiError {
            status: StatusCode::CONFLICT,
            code: "command_outcome_ambiguous",
            message,
        },
        ProjectStoreError::ProjectNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "project_not_found",
            message: "Yard project was not found".to_owned(),
        },
        ProjectStoreError::AssignmentNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "assignment_not_found",
            message: "Assignment was not found in this Yard project".to_owned(),
        },
        ProjectStoreError::AssignmentInterventionInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_intervention_in_progress",
            message: "Wait for the pending worker prompt before recording completion".to_owned(),
        },
        ProjectStoreError::DatabaseBusy => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_busy",
            message: "Yard storage is busy; retry the request".to_owned(),
        },
        _ => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "Yard storage is unavailable".to_owned(),
        },
    }
}

fn allocation_store_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::InvalidAssignment(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_assignment",
            message: error.to_string(),
        },
        ProjectStoreError::InvalidCompletionReceipt(error) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_completion_receipt",
            message: error.to_string(),
        },
        ProjectStoreError::ProjectNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "project_not_found",
            message: "Yard project was not found".to_owned(),
        },
        ProjectStoreError::ProfileNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "worker_profile_not_found",
            message: "Worker profile was not found".to_owned(),
        },
        ProjectStoreError::WorkerNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "worker_not_found",
            message: "Worker was not found".to_owned(),
        },
        ProjectStoreError::AssignmentNotFound => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "assignment_not_found",
            message: "Assignment was not found in this Yard project".to_owned(),
        },
        ProjectStoreError::ArtifactNotFound => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "artifact_not_found",
            message: "A completion artifact was not found".to_owned(),
        },
        ProjectStoreError::ArtifactScopeMismatch => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "artifact_scope_mismatch",
            message: "A completion artifact belongs to another assignment or attempt".to_owned(),
        },
        ProjectStoreError::AssignmentNotActive | ProjectStoreError::AttemptNotActive => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_not_active",
            message: "Only the active assignment attempt can be completed".to_owned(),
        },
        ProjectStoreError::AssignmentInterventionInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_intervention_in_progress",
            message: "Wait for the pending worker prompt before recording completion".to_owned(),
        },
        ProjectStoreError::AssignmentVersionConflict { current_version } => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_version_conflict",
            message: format!(
                "Assignment changed concurrently; current version is {current_version}"
            ),
        },
        ProjectStoreError::AttemptVersionConflict { current_version } => ApiError {
            status: StatusCode::CONFLICT,
            code: "attempt_version_conflict",
            message: format!(
                "Assignment attempt changed concurrently; current version is {current_version}"
            ),
        },
        ProjectStoreError::AttemptNotCurrent { current_attempt_id } => ApiError {
            status: StatusCode::CONFLICT,
            code: "attempt_not_current",
            message: format!(
                "Assignment attempt is stale; current attempt is {current_attempt_id}"
            ),
        },
        error => allocation_store_command_error(error),
    }
}

#[allow(clippy::too_many_lines)]
fn allocation_store_command_error(error: ProjectStoreError) -> ApiError {
    match error {
        ProjectStoreError::ProfileVersionConflict { current_version } => ApiError {
            status: StatusCode::CONFLICT,
            code: "worker_profile_version_conflict",
            message: format!(
                "Worker profile changed concurrently; current version is {current_version}"
            ),
        },
        ProjectStoreError::ProjectVersionConflict { current_version } => ApiError {
            status: StatusCode::CONFLICT,
            code: "project_version_conflict",
            message: format!("Project changed concurrently; current version is {current_version}"),
        },
        ProjectStoreError::WorkerVersionConflict { current_version } => ApiError {
            status: StatusCode::CONFLICT,
            code: "worker_version_conflict",
            message: format!("Worker changed concurrently; current version is {current_version}"),
        },
        ProjectStoreError::WorkerNotAvailable { availability } => ApiError {
            status: StatusCode::CONFLICT,
            code: "worker_not_available",
            message: format!("Worker is not allocatable while availability is {availability:?}"),
        },
        ProjectStoreError::WorkerProfileRequired => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "worker_profile_required",
            message: "Select a profile before assigning this adopted worker".to_owned(),
        },
        ProjectStoreError::WorkerProfileAlreadyPinned => ApiError {
            status: StatusCode::CONFLICT,
            code: "worker_profile_already_pinned",
            message: "The worker already has a pinned profile revision".to_owned(),
        },
        ProjectStoreError::HandoffTargetMatchesSource => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "handoff_target_matches_source",
            message: "Select a different target project for the handoff".to_owned(),
        },
        ProjectStoreError::OrchestratorHandoffForbidden => ApiError {
            status: StatusCode::CONFLICT,
            code: "orchestrator_handoff_forbidden",
            message: "Project orchestrators cannot be moved; replace them from the target project"
                .to_owned(),
        },
        ProjectStoreError::HandoffTargetReserved => ApiError {
            status: StatusCode::CONFLICT,
            code: "handoff_target_reserved",
            message: "The target project already has an unfinished orchestrator handoff".to_owned(),
        },
        ProjectStoreError::OrchestratorInterventionInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "orchestrator_intervention_in_progress",
            message: "Wait for the pending orchestrator intervention or replacement".to_owned(),
        },
        error @ (ProjectStoreError::HandoffSourceChanged
        | ProjectStoreError::TargetOrchestratorChanged) => ApiError {
            status: StatusCode::CONFLICT,
            code: "handoff_state_changed",
            message: error.to_string(),
        },
        error @ (ProjectStoreError::RuntimeHandoffClaimMissing
        | ProjectStoreError::RuntimeHandoffClaimConflict) => ApiError {
            status: StatusCode::CONFLICT,
            code: "handoff_runtime_conflict",
            message: error.to_string(),
        },
        ProjectStoreError::IdempotencyConflict => ApiError {
            status: StatusCode::CONFLICT,
            code: "idempotency_conflict",
            message: "Command ID is already associated with different input".to_owned(),
        },
        ProjectStoreError::CommandInProgress => ApiError {
            status: StatusCode::CONFLICT,
            code: "command_in_progress",
            message: "Allocation command is still in progress".to_owned(),
        },
        ProjectStoreError::CommandPreviouslyFailed(message) => ApiError {
            status: StatusCode::CONFLICT,
            code: "command_previously_failed",
            message,
        },
        ProjectStoreError::RuntimeWorkerAlreadyBound => ApiError {
            status: StatusCode::CONFLICT,
            code: "runtime_worker_already_bound",
            message: "The Herdr worker is already bound to a Yard worker".to_owned(),
        },
        ProjectStoreError::RuntimeWorkspaceMismatch => ApiError {
            status: StatusCode::CONFLICT,
            code: "runtime_workspace_mismatch",
            message: "The live worker does not belong to the target project workspace".to_owned(),
        },
        ProjectStoreError::CommandOutcomeAmbiguous(message) => ApiError {
            status: StatusCode::CONFLICT,
            code: "command_outcome_ambiguous",
            message,
        },
        ProjectStoreError::DatabaseBusy => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_busy",
            message: "Yard storage is busy; retry the request".to_owned(),
        },
        _ => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "Yard storage is unavailable".to_owned(),
        },
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response();
        if self.code == "database_busy" {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap},
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use futures_util::{SinkExt, StreamExt, future::join_all};
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tokio::{sync::Notify, time::timeout};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message as TungsteniteMessage, client::IntoClientRequest},
    };
    use tower::ServiceExt;
    use yard_domain::{
        AutomationScope, CanvasPlacement, CoordinationNodeKind, CreateAutomation,
        CreateCoordinationNode, CreateWorkerProfile, DailySchedule, FocusObservation,
        ObservedStatus, ObservedWorker, ProviderSessionRef, ProvisionCoordinationNode,
        RunAutomationNow, RuntimeInventory, RuntimeSession, RuntimeSessions,
        UpdateTokenSpendSettings, WorkerProfileSpec, WorkerRuntimeBinding, WorkspaceObservation,
        WorktreeObservation,
    };
    use yard_herdr::HerdrError;
    use yard_store::{SqliteProjectStore, YardStore};

    use super::{router, test_router_with_shutdown};
    use crate::allocation_service::{
        RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeRetirementError,
        RuntimeRetirementRequest, RuntimeWorkspaceProvisionRequest,
    };
    use crate::artifact_service::ArtifactService;
    use crate::automation_service::AutomationService;
    use crate::coordination_node_service::CoordinationNodeService;
    use crate::intervention_service::{
        InterventionService, RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest,
        RuntimeOutputResult, RuntimePromptRequest, RuntimePromptResult,
    };
    use crate::inventory_service::{InventoryServiceError, InventorySource};
    use crate::reconciliation_service::ReconciliationService;
    use crate::runtime_cleanup_service::RuntimeCleanupService;
    use crate::terminal_service::{
        OpenTerminalRequest, RuntimeTerminal, RuntimeTerminalError, RuntimeTerminalSession,
        TerminalClientMessage, TerminalServerMessage,
    };

    struct FakeInventory;

    #[async_trait]
    impl InventorySource for FakeInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![RuntimeSession {
                    name: "default".to_owned(),
                    is_default: true,
                    running: true,
                }],
            })
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            Ok(RuntimeInventory {
                adapter: "herdr".to_owned(),
                session: session_name.to_owned(),
                runtime_version: "0.8.0".to_owned(),
                protocol: 19,
                observed_at_unix_ms: 1,
                focus: FocusObservation::default(),
                workspaces: vec![WorkspaceObservation {
                    runtime_id: "workspace-1".to_owned(),
                    order: 1,
                    label: "Runtime API".to_owned(),
                    focused: true,
                    active_tab_id: "tab-1".to_owned(),
                    pane_count: 1,
                    tab_count: 1,
                    status: ObservedStatus::Idle,
                    tokens: BTreeMap::new(),
                    worktree: Some(WorktreeObservation {
                        repository_key: "runtime-api".to_owned(),
                        repository_name: "runtime-api".to_owned(),
                        repository_root: "/tmp/runtime-api".to_owned(),
                        checkout_path: "/tmp/runtime-api".to_owned(),
                        is_linked: false,
                    }),
                }],
                tabs: Vec::new(),
                panes: Vec::new(),
                workers: vec![
                    ObservedWorker {
                        runtime_id: "terminal-1".to_owned(),
                        terminal_id: "terminal-1".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                        tab_id: "tab-1".to_owned(),
                        pane_id: "pane-1".to_owned(),
                        name: Some("orchestrator".to_owned()),
                        provider: Some("codex".to_owned()),
                        display_provider: Some("Codex".to_owned()),
                        status: ObservedStatus::Idle,
                        focused: true,
                        launch_pending: false,
                        interactive_ready: true,
                        state_change_sequence: 1,
                        cwd: Some("/tmp/runtime-api".to_owned()),
                        foreground_cwd: Some("/tmp/runtime-api".to_owned()),
                        tokens: BTreeMap::new(),
                        provider_session: Some(provider_session("orchestrator-session")),
                        revision: 1,
                    },
                    ObservedWorker {
                        runtime_id: "terminal-yard-promptallocationcommand".to_owned(),
                        terminal_id: "terminal-yard-promptallocationcommand".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                        tab_id: "tab-yard-promptallocationcommand".to_owned(),
                        pane_id: "pane-yard-promptallocationcommand".to_owned(),
                        name: Some("yard-promptallocationcommand".to_owned()),
                        provider: Some("codex".to_owned()),
                        display_provider: Some("Codex".to_owned()),
                        status: ObservedStatus::Idle,
                        focused: false,
                        launch_pending: false,
                        interactive_ready: true,
                        state_change_sequence: 2,
                        cwd: Some("/tmp/runtime-api".to_owned()),
                        foreground_cwd: Some("/tmp/runtime-api".to_owned()),
                        tokens: BTreeMap::new(),
                        provider_session: Some(provider_session(
                            "yard-promptallocationcommand-session",
                        )),
                        revision: 2,
                    },
                    ObservedWorker {
                        runtime_id: "terminal-yard-allocationcommand1".to_owned(),
                        terminal_id: "terminal-yard-allocationcommand1".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                        tab_id: "tab-yard-allocationcommand1".to_owned(),
                        pane_id: "pane-yard-allocationcommand1".to_owned(),
                        name: Some("yard-allocationcommand1".to_owned()),
                        provider: Some("codex".to_owned()),
                        display_provider: Some("Codex".to_owned()),
                        status: ObservedStatus::Idle,
                        focused: false,
                        launch_pending: false,
                        interactive_ready: true,
                        state_change_sequence: 3,
                        cwd: Some("/tmp/runtime-api".to_owned()),
                        foreground_cwd: Some("/tmp/runtime-api".to_owned()),
                        tokens: BTreeMap::new(),
                        provider_session: Some(provider_session("yard-allocationcommand1-session")),
                        revision: 3,
                    },
                ],
                child_agents: Vec::new(),
            })
        }
    }

    struct FlakyInventory {
        snapshot: RuntimeInventory,
        inventory_calls: AtomicUsize,
    }

    #[async_trait]
    impl InventorySource for FlakyInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            FakeInventory.sessions().await
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            if self.inventory_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(self.snapshot.clone())
            } else {
                Err(InventoryServiceError::Herdr(HerdrError::SocketTimeout))
            }
        }
    }

    struct CountingInventory {
        inventory_calls: AtomicUsize,
    }

    #[async_trait]
    impl InventorySource for CountingInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            FakeInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            self.inventory_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            FakeInventory.inventory(session_name).await
        }
    }

    struct FakeRuntime;

    struct ProjectCreationInventory;

    #[async_trait]
    impl InventorySource for ProjectCreationInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            FakeInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventory = FakeInventory.inventory(session_name).await?;
            inventory.workers.push(ObservedWorker {
                runtime_id: "terminal-yard-profileprojectcommand".to_owned(),
                terminal_id: "terminal-yard-profileprojectcommand".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                tab_id: "tab-yard-profileprojectcommand".to_owned(),
                pane_id: "pane-yard-profileprojectcommand".to_owned(),
                name: Some("yard-profileprojectcommand".to_owned()),
                provider: Some("codex".to_owned()),
                display_provider: Some("Codex".to_owned()),
                status: ObservedStatus::Idle,
                focused: false,
                launch_pending: false,
                interactive_ready: true,
                state_change_sequence: 4,
                cwd: Some("/tmp/runtime-api".to_owned()),
                foreground_cwd: Some("/tmp/runtime-api".to_owned()),
                tokens: BTreeMap::new(),
                provider_session: Some(provider_session("yard-profileprojectcommand-session")),
                revision: 4,
            });
            Ok(inventory)
        }
    }

    struct WorkspaceProjectCreationInventory;

    #[async_trait]
    impl InventorySource for WorkspaceProjectCreationInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            FakeInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventory = FakeInventory.inventory(session_name).await?;
            inventory.workspaces.push(WorkspaceObservation {
                runtime_id: "workspace-created".to_owned(),
                order: 2,
                label: "Workspace API".to_owned(),
                focused: false,
                active_tab_id: "tab-yard-workspaceprojectcommand".to_owned(),
                pane_count: 1,
                tab_count: 1,
                status: ObservedStatus::Idle,
                tokens: BTreeMap::new(),
                worktree: Some(WorktreeObservation {
                    repository_key: "workspace-api".to_owned(),
                    repository_name: "workspace-api".to_owned(),
                    repository_root: "/tmp/workspace-api".to_owned(),
                    checkout_path: "/tmp/workspace-api".to_owned(),
                    is_linked: false,
                }),
            });
            inventory.workers.push(ObservedWorker {
                runtime_id: "terminal-yard-workspaceprojectcommand".to_owned(),
                terminal_id: "terminal-yard-workspaceprojectcommand".to_owned(),
                workspace_id: "workspace-created".to_owned(),
                tab_id: "tab-yard-workspaceprojectcommand".to_owned(),
                pane_id: "pane-yard-workspaceprojectcommand".to_owned(),
                name: Some("yard-workspaceprojectcommand".to_owned()),
                provider: Some("codex".to_owned()),
                display_provider: Some("Codex".to_owned()),
                status: ObservedStatus::Idle,
                focused: false,
                launch_pending: false,
                interactive_ready: true,
                state_change_sequence: 5,
                cwd: Some("/tmp/workspace-api".to_owned()),
                foreground_cwd: Some("/tmp/workspace-api".to_owned()),
                tokens: BTreeMap::new(),
                provider_session: Some(provider_session("yard-workspaceprojectcommand-session")),
                revision: 5,
            });
            Ok(inventory)
        }
    }

    struct YardOrchestratorProvisionInventory;

    #[async_trait]
    impl InventorySource for YardOrchestratorProvisionInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            FakeInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            Ok(RuntimeInventory {
                adapter: "herdr".to_owned(),
                session: session_name.to_owned(),
                runtime_version: "0.8.0".to_owned(),
                protocol: 19,
                observed_at_unix_ms: 10,
                focus: FocusObservation::default(),
                workspaces: vec![WorkspaceObservation {
                    runtime_id: "workspace-yard-orchestrator".to_owned(),
                    order: 1,
                    label: "Yard central coordination".to_owned(),
                    focused: false,
                    active_tab_id: "tab-yard-orchestrator".to_owned(),
                    pane_count: 1,
                    tab_count: 1,
                    status: ObservedStatus::Idle,
                    tokens: BTreeMap::new(),
                    worktree: None,
                }],
                tabs: Vec::new(),
                panes: Vec::new(),
                workers: vec![ObservedWorker {
                    runtime_id: "terminal-yard-orchestrator".to_owned(),
                    terminal_id: "terminal-yard-orchestrator".to_owned(),
                    workspace_id: "workspace-yard-orchestrator".to_owned(),
                    tab_id: "tab-yard-orchestrator".to_owned(),
                    pane_id: "pane-yard-orchestrator".to_owned(),
                    name: Some("yard-orchestrator".to_owned()),
                    provider: Some("codex".to_owned()),
                    display_provider: Some("Codex".to_owned()),
                    status: ObservedStatus::Idle,
                    focused: false,
                    launch_pending: false,
                    interactive_ready: true,
                    state_change_sequence: 1,
                    cwd: Some("/tmp/yard".to_owned()),
                    foreground_cwd: Some("/tmp/yard".to_owned()),
                    tokens: BTreeMap::new(),
                    provider_session: Some(provider_session("yard-orchestrator-session")),
                    revision: 1,
                }],
                child_agents: Vec::new(),
            })
        }
    }

    struct HandoffInventory;

    #[async_trait]
    impl InventorySource for HandoffInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            FakeInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventory = FakeInventory.inventory(session_name).await?;
            inventory.workspaces.push(WorkspaceObservation {
                runtime_id: "workspace-2".to_owned(),
                order: 2,
                label: "Target API".to_owned(),
                focused: false,
                active_tab_id: "tab-target-orchestrator".to_owned(),
                pane_count: 2,
                tab_count: 2,
                status: ObservedStatus::Idle,
                tokens: BTreeMap::new(),
                worktree: Some(WorktreeObservation {
                    repository_key: "target-api".to_owned(),
                    repository_name: "target-api".to_owned(),
                    repository_root: "/tmp/target-api".to_owned(),
                    checkout_path: "/tmp/target-api".to_owned(),
                    is_linked: false,
                }),
            });
            inventory.workers.extend([
                ObservedWorker {
                    runtime_id: "terminal-target-orchestrator".to_owned(),
                    terminal_id: "terminal-target-orchestrator".to_owned(),
                    workspace_id: "workspace-2".to_owned(),
                    tab_id: "tab-target-orchestrator".to_owned(),
                    pane_id: "pane-target-orchestrator".to_owned(),
                    name: Some("target-orchestrator".to_owned()),
                    provider: Some("codex".to_owned()),
                    display_provider: Some("Codex".to_owned()),
                    status: ObservedStatus::Idle,
                    focused: false,
                    launch_pending: false,
                    interactive_ready: true,
                    state_change_sequence: 4,
                    cwd: Some("/tmp/target-api".to_owned()),
                    foreground_cwd: Some("/tmp/target-api".to_owned()),
                    tokens: BTreeMap::new(),
                    provider_session: Some(provider_session("target-orchestrator-session")),
                    revision: 4,
                },
                ObservedWorker {
                    runtime_id: "terminal-yard-handoffcommand1".to_owned(),
                    terminal_id: "terminal-yard-handoffcommand1".to_owned(),
                    workspace_id: "workspace-2".to_owned(),
                    tab_id: "tab-yard-handoffcommand1".to_owned(),
                    pane_id: "pane-yard-handoffcommand1".to_owned(),
                    name: Some("yard-handoffcommand1".to_owned()),
                    provider: Some("codex".to_owned()),
                    display_provider: Some("Codex".to_owned()),
                    status: ObservedStatus::Idle,
                    focused: false,
                    launch_pending: false,
                    interactive_ready: true,
                    state_change_sequence: 5,
                    cwd: Some("/tmp/target-api".to_owned()),
                    foreground_cwd: Some("/tmp/target-api".to_owned()),
                    tokens: BTreeMap::new(),
                    provider_session: Some(provider_session("yard-handoffcommand1-session")),
                    revision: 5,
                },
                ObservedWorker {
                    runtime_id: "terminal-yard-replacecommand1".to_owned(),
                    terminal_id: "terminal-yard-replacecommand1".to_owned(),
                    workspace_id: "workspace-2".to_owned(),
                    tab_id: "tab-yard-replacecommand1".to_owned(),
                    pane_id: "pane-yard-replacecommand1".to_owned(),
                    name: Some("yard-replacecommand1".to_owned()),
                    provider: Some("codex".to_owned()),
                    display_provider: Some("Codex".to_owned()),
                    status: ObservedStatus::Idle,
                    focused: false,
                    launch_pending: false,
                    interactive_ready: true,
                    state_change_sequence: 6,
                    cwd: Some("/tmp/target-api".to_owned()),
                    foreground_cwd: Some("/tmp/target-api".to_owned()),
                    tokens: BTreeMap::new(),
                    provider_session: Some(provider_session("yard-replacecommand1-session")),
                    revision: 6,
                },
            ]);
            Ok(inventory)
        }
    }

    struct ReplacementIdentityMismatchInventory;

    #[async_trait]
    impl InventorySource for ReplacementIdentityMismatchInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            HandoffInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventory = HandoffInventory.inventory(session_name).await?;
            let replacement = inventory
                .workers
                .iter_mut()
                .find(|worker| worker.terminal_id == "terminal-yard-replacecommand1")
                .unwrap();
            replacement.provider_session = Some(provider_session("different-replacement-session"));
            Ok(inventory)
        }
    }

    #[derive(Default)]
    struct ReplacementWorkspaceMissingInventory {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl InventorySource for ReplacementWorkspaceMissingInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            HandoffInventory.sessions().await
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventory = HandoffInventory.inventory(session_name).await?;
            if self.calls.fetch_add(1, Ordering::SeqCst) == 2 {
                inventory
                    .workspaces
                    .retain(|workspace| workspace.runtime_id != "workspace-2");
            }
            Ok(inventory)
        }
    }

    #[derive(Default)]
    struct CountingRuntime {
        bootstrap_calls: AtomicUsize,
        provision_calls: AtomicUsize,
        bootstrap_requests: Mutex<Vec<RuntimeWorkspaceProvisionRequest>>,
        provision_requests: Mutex<Vec<RuntimeProvisionRequest>>,
    }

    #[async_trait]
    impl RuntimeControl for CountingRuntime {
        async fn bootstrap_worker(
            &self,
            request: RuntimeWorkspaceProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.bootstrap_calls.fetch_add(1, Ordering::SeqCst);
            self.bootstrap_requests
                .lock()
                .unwrap()
                .push(request.clone());
            FakeRuntime.bootstrap_worker(request).await
        }

        async fn provision_worker(
            &self,
            request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.provision_calls.fetch_add(1, Ordering::SeqCst);
            self.provision_requests
                .lock()
                .unwrap()
                .push(request.clone());
            FakeRuntime.provision_worker(request).await
        }
    }

    #[derive(Default)]
    struct AmbiguousBootstrapRuntime {
        bootstrap_calls: AtomicUsize,
    }

    #[async_trait]
    impl RuntimeControl for AmbiguousBootstrapRuntime {
        async fn bootstrap_worker(
            &self,
            _request: RuntimeWorkspaceProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.bootstrap_calls.fetch_add(1, Ordering::SeqCst);
            Err(RuntimeProvisionError::AfterPreparation {
                message: "Herdr closed the socket before returning a response".to_owned(),
                ambiguous: true,
                started_runtime: None,
            })
        }

        async fn provision_worker(
            &self,
            request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            FakeRuntime.provision_worker(request).await
        }
    }

    struct ClaimCheckingRuntime {
        database_path: PathBuf,
        claim_seen_before_start: AtomicBool,
        start_calls: AtomicUsize,
        replacement_prompt_failure: AtomicBool,
        retirement_failures: AtomicUsize,
        retirement_calls: Mutex<Vec<RuntimeRetirementRequest>>,
        start_requests: Mutex<Vec<RuntimeProvisionRequest>>,
    }

    #[async_trait]
    impl RuntimeControl for ClaimCheckingRuntime {
        async fn provision_worker(
            &self,
            request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            FakeRuntime.provision_worker(request).await
        }

        async fn prepare_worker(
            &self,
            request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            let mut prepared = FakeRuntime.provision_worker(request).await?;
            prepared.provider_session = None;
            prepared.process_state = yard_domain::RuntimeProcessState::Unknown;
            Ok(prepared)
        }

        async fn start_prepared_worker(
            &self,
            request: RuntimeProvisionRequest,
            mut prepared: WorkerRuntimeBinding,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            self.start_requests.lock().unwrap().push(request.clone());

            // A separate connection proves the claim transaction committed before start.
            let connection = rusqlite::Connection::open(&self.database_path).unwrap();
            let claimed = connection
                .query_row(
                    "SELECT target_runtime_adapter, target_runtime_session,
                            target_runtime_workspace_id, target_terminal_id,
                            target_tab_id, target_pane_id,
                            target_runtime_claimed_at_unix_ms
                       FROM worker_handoff_commands
                      WHERE command_id = ?1",
                    [&request.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, Option<i64>>(6)?,
                        ))
                    },
                )
                .unwrap();
            assert_eq!(claimed.0, prepared.adapter);
            assert_eq!(claimed.1, prepared.session);
            assert_eq!(claimed.2, prepared.workspace_id);
            assert_eq!(claimed.3, prepared.terminal_id);
            assert_eq!(claimed.4, prepared.tab_id);
            assert_eq!(claimed.5, prepared.pane_id);
            assert!(claimed.6.is_some());
            self.claim_seen_before_start.store(true, Ordering::SeqCst);

            prepared.provider_session =
                Some(provider_session(&format!("{}-session", request.agent_name)));
            prepared.process_state = yard_domain::RuntimeProcessState::Running;
            Ok(prepared)
        }

        async fn prepare_replacement_worker(
            &self,
            request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            let mut prepared = FakeRuntime.provision_worker(request).await?;
            prepared.provider_session = None;
            prepared.process_state = yard_domain::RuntimeProcessState::Unknown;
            Ok(prepared)
        }

        async fn start_prepared_replacement_worker(
            &self,
            request: RuntimeProvisionRequest,
            mut prepared: WorkerRuntimeBinding,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            self.start_requests.lock().unwrap().push(request.clone());

            let connection = rusqlite::Connection::open(&self.database_path).unwrap();
            let claimed = connection
                .query_row(
                    "SELECT adapter, runtime_session, runtime_workspace_id,
                            terminal_id, tab_id, pane_id, captured_at_unix_ms
                       FROM orchestrator_replacement_runtime_bindings
                      WHERE command_id = ?1
                        AND binding_role = 'replacement_prepared'",
                    [&request.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .unwrap();
            assert_eq!(claimed.0, prepared.adapter);
            assert_eq!(claimed.1, prepared.session);
            assert_eq!(claimed.2, prepared.workspace_id);
            assert_eq!(claimed.3, prepared.terminal_id);
            assert_eq!(claimed.4, prepared.tab_id);
            assert_eq!(claimed.5, prepared.pane_id);
            assert!(claimed.6 > 0);
            self.claim_seen_before_start.store(true, Ordering::SeqCst);

            prepared.provider_session =
                Some(provider_session(&format!("{}-session", request.agent_name)));
            prepared.process_state = yard_domain::RuntimeProcessState::Running;
            if self.replacement_prompt_failure.load(Ordering::SeqCst) {
                return Err(RuntimeProvisionError::PromptDelivery {
                    runtime: Box::new(prepared),
                    message: "simulated objective delivery ambiguity".to_owned(),
                });
            }
            Ok(prepared)
        }

        async fn retire_runtime(
            &self,
            request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            self.retirement_calls.lock().unwrap().push(request);
            let should_fail = self
                .retirement_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
            if should_fail {
                Err(RuntimeRetirementError::Runtime(
                    "simulated Herdr close failure".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
    }

    struct FakeTerminalSession {
        sent_frame: bool,
    }

    #[derive(Default)]
    struct ReportingRuntime {
        prompts: Mutex<Vec<RuntimePromptRequest>>,
        outputs: Mutex<HashMap<String, String>>,
        prompt_gate: Option<PromptGate>,
    }

    #[derive(Clone)]
    struct PromptGate {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl ReportingRuntime {
        fn blocked() -> (Self, Arc<Notify>, Arc<Notify>) {
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            (
                Self {
                    prompt_gate: Some(PromptGate {
                        entered: Arc::clone(&entered),
                        release: Arc::clone(&release),
                    }),
                    ..Self::default()
                },
                entered,
                release,
            )
        }

        fn set_output(&self, pane_id: &str, text: impl Into<String>) {
            self.outputs
                .lock()
                .unwrap()
                .insert(pane_id.to_owned(), text.into());
        }
    }

    #[async_trait]
    impl RuntimeIntervention for ReportingRuntime {
        async fn prompt(
            &self,
            request: RuntimePromptRequest,
        ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
            self.prompts.lock().unwrap().push(request);
            if let Some(gate) = &self.prompt_gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            Ok(RuntimePromptResult {
                status: "working".to_owned(),
            })
        }

        async fn read_output(
            &self,
            request: RuntimeOutputRequest,
        ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
            let (workspace_id, tab_id) = if request.pane_id == "pane-1" {
                ("workspace-1", "tab-1")
            } else {
                ("workspace-1", "tab-yard-allocationcommand1")
            };
            let text = self
                .outputs
                .lock()
                .unwrap()
                .get(&request.pane_id)
                .cloned()
                .unwrap_or_default();
            Ok(RuntimeOutputResult {
                pane_id: request.pane_id,
                workspace_id: workspace_id.to_owned(),
                tab_id: tab_id.to_owned(),
                source: "recent_unwrapped".to_owned(),
                format: "text".to_owned(),
                text,
                revision: 7,
                truncated: false,
            })
        }
    }

    #[async_trait]
    impl RuntimeTerminalSession for FakeTerminalSession {
        async fn next_message(
            &mut self,
        ) -> Result<Option<TerminalServerMessage>, RuntimeTerminalError> {
            if !self.sent_frame {
                self.sent_frame = true;
                return Ok(Some(TerminalServerMessage::Frame {
                    bytes: "cmVhZHk=".to_owned(),
                    encoding: "ansi".to_owned(),
                    seq: 1,
                    width: 80,
                    height: 24,
                    full: true,
                }));
            }
            std::future::pending().await
        }

        async fn send(
            &mut self,
            _command: TerminalClientMessage,
        ) -> Result<(), RuntimeTerminalError> {
            Ok(())
        }

        async fn release(&mut self) -> Result<(), RuntimeTerminalError> {
            Ok(())
        }
    }

    #[async_trait]
    impl RuntimeControl for FakeRuntime {
        async fn ensure_session(
            &self,
            _request: crate::allocation_service::RuntimeSessionRequest,
        ) -> Result<(), RuntimeProvisionError> {
            Ok(())
        }

        async fn bootstrap_worker(
            &self,
            request: RuntimeWorkspaceProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            Ok(WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: request.session,
                workspace_id: "workspace-created".to_owned(),
                terminal_id: format!("terminal-{}", request.agent_name),
                tab_id: Some(format!("tab-{}", request.agent_name)),
                pane_id: format!("pane-{}", request.agent_name),
                provider_session: Some(provider_session(&format!(
                    "{}-session",
                    request.agent_name
                ))),
                owns_tab: true,
                observation_state: yard_domain::RuntimeObservationState::Observed,
                process_state: yard_domain::RuntimeProcessState::Running,
                status: yard_domain::ObservedStatus::Idle,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 2,
            })
        }

        async fn provision_worker(
            &self,
            request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            Ok(WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: request.session,
                workspace_id: request.workspace_id,
                terminal_id: format!("terminal-{}", request.agent_name),
                tab_id: Some(format!("tab-{}", request.agent_name)),
                pane_id: format!("pane-{}", request.agent_name),
                provider_session: Some(provider_session(&format!(
                    "{}-session",
                    request.agent_name
                ))),
                owns_tab: true,
                observation_state: yard_domain::RuntimeObservationState::Observed,
                process_state: yard_domain::RuntimeProcessState::Running,
                status: yard_domain::ObservedStatus::Idle,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 2,
            })
        }

        async fn retire_runtime(
            &self,
            _request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            Ok(())
        }
    }

    fn provider_session(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    #[async_trait]
    impl RuntimeIntervention for FakeRuntime {
        async fn prompt(
            &self,
            _request: RuntimePromptRequest,
        ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
            Ok(RuntimePromptResult {
                status: "working".to_owned(),
            })
        }

        async fn read_output(
            &self,
            request: RuntimeOutputRequest,
        ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
            let tab_id = if request.pane_id == "pane-1" {
                "tab-1"
            } else {
                "tab-yard-promptallocationcommand"
            };
            Ok(RuntimeOutputResult {
                pane_id: request.pane_id,
                workspace_id: "workspace-1".to_owned(),
                tab_id: tab_id.to_owned(),
                source: "recent_unwrapped".to_owned(),
                format: "text".to_owned(),
                text: "Focused tests are passing.".to_owned(),
                revision: 0,
                truncated: false,
            })
        }
    }

    #[async_trait]
    impl RuntimeTerminal for FakeRuntime {
        async fn open_terminal(
            &self,
            _request: OpenTerminalRequest,
        ) -> Result<Box<dyn RuntimeTerminalSession>, RuntimeTerminalError> {
            Ok(Box::new(FakeTerminalSession { sent_frame: false }))
        }
    }

    async fn test_router_with_source(source: Arc<dyn InventorySource>) -> (Router, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        (
            router(
                source,
                runtime.clone(),
                runtime.clone(),
                runtime,
                store,
                artifacts,
            ),
            temp,
        )
    }

    async fn test_router() -> (Router, TempDir) {
        test_router_with_source(Arc::new(FakeInventory)).await
    }

    async fn shutdown_test_router() -> (
        Router,
        TempDir,
        tokio::sync::watch::Sender<bool>,
        crate::ConnectionTracker,
    ) {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let connections = crate::ConnectionTracker::default();
        let app = test_router_with_shutdown(
            Arc::new(FakeInventory),
            runtime.clone(),
            runtime.clone(),
            runtime,
            store,
            artifacts,
            Some(receiver),
            connections.clone(),
        );
        (app, temp, shutdown, connections)
    }

    #[tokio::test]
    async fn embedded_web_fallback_preserves_server_route_semantics() {
        let (app, _temp) = test_router().await;

        let root = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root.status(), StatusCode::OK);
        assert_eq!(
            root.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );

        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert_eq!(health.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(response_json(health).await["status"], "ok");

        let projects = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(projects.status(), StatusCode::OK);
        assert_eq!(projects.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(projects.headers()[header::CACHE_CONTROL], "no-store");

        let unknown_api = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/not-a-route")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown_api.status(), StatusCode::NOT_FOUND);
        assert!(unknown_api.headers().get(header::CONTENT_TYPE).is_none());

        let health_post = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health_post.status(), StatusCode::METHOD_NOT_ALLOWED);

        let terminal_head = app
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/api/v1/yard/orchestrator/terminal?cols=80&rows=24")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(terminal_head.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    async fn handoff_test_router() -> (Router, TempDir, Arc<ClaimCheckingRuntime>) {
        handoff_test_router_with_retirement_failures(0).await
    }

    async fn handoff_test_router_with_retirement_failures(
        retirement_failures: usize,
    ) -> (Router, TempDir, Arc<ClaimCheckingRuntime>) {
        handoff_test_router_with_source(retirement_failures, Arc::new(HandoffInventory)).await
    }

    async fn handoff_test_router_with_source(
        retirement_failures: usize,
        source: Arc<dyn InventorySource>,
    ) -> (Router, TempDir, Arc<ClaimCheckingRuntime>) {
        let temp = TempDir::new().unwrap();
        let database_path = temp.path().join("yard.sqlite3");
        let store = Arc::new(SqliteProjectStore::open(&database_path).await.unwrap());
        let runtime = Arc::new(ClaimCheckingRuntime {
            database_path,
            claim_seen_before_start: AtomicBool::new(false),
            start_calls: AtomicUsize::new(0),
            replacement_prompt_failure: AtomicBool::new(false),
            retirement_failures: AtomicUsize::new(retirement_failures),
            retirement_calls: Mutex::new(Vec::new()),
            start_requests: Mutex::new(Vec::new()),
        });
        let interactive = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            source,
            runtime.clone(),
            interactive.clone(),
            interactive,
            store,
            artifacts,
        );
        (app, temp, runtime)
    }

    fn runtime_binding_rows(temp: &TempDir) -> Vec<(String, String, String, i64, i64)> {
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT terminal_id, observation_state, process_state, version,
                        last_observed_at_unix_ms
                   FROM worker_runtime_bindings
                  ORDER BY terminal_id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn create_body(orchestrator: &str) -> String {
        serde_json::json!({
            "name": "Runtime API",
            "runtime": {
                "adapter": "herdr",
                "session": "default",
                "workspace_id": "workspace-1"
            },
            "orchestrator_observed_worker_id": orchestrator,
            "placement": {
                "x": 80.0,
                "y": 70.0,
                "width": 322.0,
                "height": 240.0
            }
        })
        .to_string()
    }

    fn profile_body(name: &str) -> String {
        serde_json::json!({
            "name": name,
            "runtime_adapter": "herdr",
            "provider": "codex",
            "model": "gpt-5.4",
            "default_role": "implementer",
            "instructions_ref": null,
            "tools": [],
            "skills": [],
            "mcp_servers": [],
            "sandbox_policy": "runtime_default",
            "worktree_policy": "project_workspace",
            "permission_policy": "runtime_default",
            "completion_contract": "manual_receipt"
        })
        .to_string()
    }

    fn agent_profile_fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../yard-domain/tests/fixtures/agent-profile-v1alpha1.json"
        ))
        .unwrap()
    }

    fn agent_profile_body(manifest: &serde_json::Value) -> String {
        serde_json::json!({ "manifest": manifest }).to_string()
    }

    fn profile_project_body(profile_id: &str) -> String {
        serde_json::json!({
            "command_id": "profile-project-command",
            "actor": "local-user",
            "name": "Runtime API",
            "runtime": {
                "adapter": "herdr",
                "session": "default",
                "workspace_id": "workspace-1"
            },
            "profile_id": profile_id,
            "expected_profile_version": "1",
            "orchestrator_objective": "Coordinate implementation of the Runtime API.",
            "placement": {
                "x": 80.0,
                "y": 70.0,
                "width": 322.0,
                "height": 240.0
            }
        })
        .to_string()
    }

    fn workspace_project_body(profile_id: &str) -> String {
        serde_json::json!({
            "command_id": "workspace-project-command",
            "actor": "local-user",
            "name": "Workspace API",
            "runtime_adapter": "herdr",
            "runtime_session": "default",
            "workspace_label": "Workspace API",
            "cwd": "/tmp/workspace-api",
            "profile_id": profile_id,
            "expected_profile_version": "1",
            "orchestrator_objective": "Coordinate implementation of the Workspace API.",
            "placement": {
                "x": 80.0,
                "y": 70.0,
                "width": 322.0,
                "height": 240.0
            }
        })
        .to_string()
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }

    async fn get_json(app: &Router, uri: &str) -> serde_json::Value {
        response_json(
            app.clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await
    }

    async fn assert_automatic_token_spend_defaults_off(app: &Router) {
        let settings = get_json(app, "/api/v1/token-spend-settings").await;
        assert_eq!(
            settings["superintendent_auto_requests_project_summaries"],
            false
        );
        assert_eq!(
            settings["project_orchestrators_auto_request_worker_summaries"],
            false
        );
        assert_eq!(settings["scheduled_automatic_summaries"], false);
    }

    async fn create_active_assignment(app: &Router) -> (String, String) {
        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Implementer")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let project_id = project["id"].as_str().unwrap().to_owned();
        let allocation = serde_json::json!({
            "command_id": "prompt-allocation-command",
            "actor": "local-user",
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_project_version": project["version"],
            "objective": "Exercise the interactive terminal bridge.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let created = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/api/v1/projects/{project_id}/assignments"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(allocation.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        (
            project_id,
            created["assignment"]["id"].as_str().unwrap().to_owned(),
        )
    }

    async fn create_handoff_request(app: &Router) -> (String, serde_json::Value) {
        let (source_project_id, assignment_id) = create_active_assignment(app).await;
        let source_project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/projects/{source_project_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let assignments = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/projects/{source_project_id}/assignments"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let assignment = assignments["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|assignment| assignment["id"] == assignment_id)
            .unwrap();
        let target_project_body = serde_json::json!({
            "name": "Target API",
            "runtime": {
                "adapter": "herdr",
                "session": "default",
                "workspace_id": "workspace-2"
            },
            "orchestrator_observed_worker_id": "terminal-target-orchestrator",
            "placement": {
                "x": 480.0,
                "y": 70.0,
                "width": 322.0,
                "height": 240.0
            }
        });
        let target_project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(target_project_body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let uri =
            format!("/api/v1/projects/{source_project_id}/assignments/{assignment_id}/handoffs");
        let command = serde_json::json!({
            "command_id": "handoff-command-1",
            "actor": "local-user",
            "worker_id": assignment["worker"]["id"],
            "expected_worker_version": assignment["worker"]["version"],
            "expected_source_project_version": source_project["version"],
            "target_project_id": target_project["id"],
            "expected_target_project_version": target_project["version"],
            "source_attempt_id": assignment["attempt"]["id"],
            "expected_source_assignment_version": assignment["version"],
            "expected_source_attempt_version": assignment["attempt"]["version"],
            "target_role": "member",
            "objective": "Continue implementation in the target project.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        (uri, command)
    }

    async fn create_orchestrator_replacement_request(
        app: &Router,
    ) -> (String, serde_json::Value, String) {
        let target_project_body = serde_json::json!({
            "name": "Target API",
            "runtime": {
                "adapter": "herdr",
                "session": "default",
                "workspace_id": "workspace-2"
            },
            "orchestrator_observed_worker_id": "terminal-target-orchestrator",
            "placement": {
                "x": 480.0,
                "y": 70.0,
                "width": 322.0,
                "height": 240.0
            }
        });
        let target_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(target_project_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(target_response.status(), StatusCode::CREATED);
        let target_project = response_json(target_response).await;
        let profile_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/worker-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(profile_body("Replacement orchestrator")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(profile_response.status(), StatusCode::CREATED);
        let profile = response_json(profile_response).await;
        let project_id = target_project["id"].as_str().unwrap().to_owned();
        let uri = format!("/api/v1/projects/{project_id}/orchestrator/replace");
        let command = serde_json::json!({
            "command_id": "replace-command-1",
            "actor": "local-user",
            "expected_project_version": target_project["version"],
            "expected_orchestrator_worker_id": target_project["orchestrator"]["id"],
            "expected_orchestrator_worker_version":
                target_project["orchestrator"]["version"],
            "expected_orchestrator_runtime":
                target_project["orchestrator"]["runtime"],
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "objective": "Continue orchestration with fresh context.",
            "role": "orchestrator",
            "old_session_disposition": "retire_after_cutover",
            "handoff_artifact_ref": "artifact://orchestrator-handoff"
        });
        (uri, command, project_id)
    }

    async fn create_orchestrator_transfer_request(
        app: &Router,
    ) -> (String, serde_json::Value, String) {
        let target_project_body = serde_json::json!({
            "name": "Transfer API",
            "runtime": {
                "adapter": "herdr",
                "session": "default",
                "workspace_id": "workspace-2"
            },
            "orchestrator_observed_worker_id": "terminal-target-orchestrator",
            "placement": {
                "x": 480.0,
                "y": 70.0,
                "width": 322.0,
                "height": 240.0
            }
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(target_project_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = response_json(response).await;
        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory.status(), StatusCode::OK);
        let project_id = created["id"].as_str().unwrap().to_owned();
        let project = get_json(app, &format!("/api/v1/projects/{project_id}")).await;
        let workers = get_json(app, "/api/v1/workers").await;
        let candidate = workers["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["worker"]["runtime"]["terminal_id"] == "terminal-yard-handoffcommand1"
            })
            .unwrap();
        assert_eq!(candidate["availability"], "unassigned_live");
        let command = serde_json::json!({
            "command_id": "transfer-command-1",
            "actor": "local-user",
            "worker_id": candidate["worker"]["id"],
            "expected_worker_version": candidate["worker"]["version"],
            "expected_worker_runtime": candidate["worker"]["runtime"],
            "expected_project_version": project["version"],
            "expected_orchestrator_worker_id": project["orchestrator"]["id"],
            "expected_orchestrator_worker_version": project["orchestrator"]["version"],
            "expected_orchestrator_runtime": project["orchestrator"]["runtime"]
        });
        (
            format!("/api/v1/projects/{project_id}/orchestrator"),
            command,
            project_id,
        )
    }

    #[test]
    fn maps_pending_prompt_completion_to_conflict() {
        let error = super::allocation_store_error(
            yard_store::ProjectStoreError::AssignmentInterventionInProgress,
        );

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.code, "assignment_intervention_in_progress");
        assert_eq!(
            error.message,
            "Wait for the pending worker prompt before recording completion"
        );
    }

    #[tokio::test]
    async fn replacement_endpoint_claims_starts_verifies_and_cuts_over_before_cleanup() {
        let (app, temp, runtime) = handoff_test_router().await;
        let (uri, command, project_id) = create_orchestrator_replacement_request(&app).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let replacement = response_json(response).await;
        assert_eq!(replacement["command_id"], "replace-command-1");
        assert_eq!(replacement["replayed"], false);
        assert_eq!(replacement["cleanup_pending"], false);
        assert_eq!(
            replacement["project"]["orchestrator"]["runtime"]["terminal_id"],
            "terminal-yard-replacecommand1"
        );
        assert!(
            runtime.claim_seen_before_start.load(Ordering::SeqCst),
            "replacement start was reached before the prepared snapshot committed"
        );
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        {
            let retirements = runtime.retirement_calls.lock().unwrap();
            assert_eq!(retirements.len(), 1);
            assert_eq!(retirements[0].terminal_id, "terminal-target-orchestrator");
            assert_eq!(
                retirements[0].provider_session,
                Some(provider_session("target-orchestrator-session"))
            );
        }

        let project = get_json(&app, &format!("/api/v1/projects/{project_id}")).await;
        assert_eq!(
            project["orchestrator"]["id"],
            replacement["project"]["orchestrator"]["id"]
        );
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let (snapshot_count, receipt_count, old_binding_count): (i64, i64, i64) = connection
            .query_row(
                "SELECT
                    (
                        SELECT COUNT(*)
                          FROM orchestrator_replacement_runtime_bindings
                         WHERE command_id = 'replace-command-1'
                    ),
                    (SELECT COUNT(*) FROM completion_receipts),
                    (
                        SELECT COUNT(*)
                          FROM worker_runtime_bindings
                         WHERE terminal_id = 'terminal-target-orchestrator'
                    )",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(snapshot_count, 3);
        assert_eq!(receipt_count, 0);
        assert_eq!(old_binding_count, 0);
    }

    #[tokio::test]
    async fn replacement_rejects_stale_captured_identity_before_external_mutation() {
        let (app, _temp, runtime) = handoff_test_router().await;
        let (uri, mut command, _project_id) = create_orchestrator_replacement_request(&app).await;
        command["expected_orchestrator_runtime"]["provider_session"]["value"] =
            serde_json::Value::String("different-provider-session".to_owned());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 0);
        assert!(runtime.retirement_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replacement_prompt_ambiguity_retains_started_identity_without_cutover() {
        let (app, temp, runtime) = handoff_test_router().await;
        runtime
            .replacement_prompt_failure
            .store(true, Ordering::SeqCst);
        let (uri, command, project_id) = create_orchestrator_replacement_request(&app).await;
        let displaced_worker_id = command["expected_orchestrator_worker_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = response_json(response).await;
        assert_eq!(error["error"]["code"], "command_outcome_ambiguous");
        let project = get_json(&app, &format!("/api/v1/projects/{project_id}")).await;
        assert_eq!(project["orchestrator"]["id"], displaced_worker_id);
        assert!(runtime.retirement_calls.lock().unwrap().is_empty());

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let (status, started_snapshots, cleanup_jobs): (String, i64, i64) = connection
            .query_row(
                "SELECT command.status,
                        (
                            SELECT COUNT(*)
                              FROM orchestrator_replacement_runtime_bindings snapshot
                             WHERE snapshot.command_id = command.id
                               AND snapshot.binding_role =
                                   'replacement_started'
                        ),
                        (
                            SELECT COUNT(*)
                              FROM runtime_cleanup_jobs cleanup
                             WHERE cleanup.command_id = command.id
                        )
                   FROM command_acknowledgements command
                  WHERE command.id = 'replace-command-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "ambiguous");
        assert_eq!(started_snapshots, 1);
        assert_eq!(cleanup_jobs, 0);
    }

    #[tokio::test]
    async fn replacement_provider_mismatch_blocks_cutover_and_retirement() {
        let (app, _temp, runtime) =
            handoff_test_router_with_source(0, Arc::new(ReplacementIdentityMismatchInventory))
                .await;
        let (uri, command, project_id) = create_orchestrator_replacement_request(&app).await;
        let displaced_worker_id = command["expected_orchestrator_worker_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = response_json(response).await;
        assert_eq!(
            error["error"]["code"],
            "orchestrator_replacement_identity_changed"
        );
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        assert!(runtime.retirement_calls.lock().unwrap().is_empty());
        let project = get_json(&app, &format!("/api/v1/projects/{project_id}")).await;
        assert_eq!(project["orchestrator"]["id"], displaced_worker_id);
    }

    #[tokio::test]
    async fn replacement_missing_fresh_workspace_blocks_cutover_and_retirement() {
        let (app, _temp, runtime) = handoff_test_router_with_source(
            0,
            Arc::new(ReplacementWorkspaceMissingInventory::default()),
        )
        .await;
        let (uri, command, project_id) = create_orchestrator_replacement_request(&app).await;
        let displaced_worker_id = command["expected_orchestrator_worker_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = response_json(response).await;
        assert_eq!(
            error["error"]["code"],
            "orchestrator_replacement_identity_changed"
        );
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        assert!(runtime.retirement_calls.lock().unwrap().is_empty());
        let project = get_json(&app, &format!("/api/v1/projects/{project_id}")).await;
        assert_eq!(project["orchestrator"]["id"], displaced_worker_id);
    }

    #[tokio::test]
    async fn project_orchestrator_transfer_replays_keeps_displaced_live_and_revokes_lease() {
        let (app, _temp, _runtime) = handoff_test_router().await;
        let (uri, command, project_id) = create_orchestrator_transfer_request(&app).await;
        let replaced_worker_id = command["expected_orchestrator_worker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let terminal_url = format!(
            "ws://{address}/api/v1/projects/{project_id}/orchestrator/terminal?cols=80&rows=24"
        );
        let mut terminal_request = terminal_url.into_client_request().unwrap();
        terminal_request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(terminal_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let transferred = response_json(response).await;
        assert_eq!(transferred["replaced_worker_id"], replaced_worker_id);
        assert_eq!(
            transferred["project"]["orchestrator"]["id"],
            command["worker_id"]
        );
        assert_eq!(transferred["replayed"], false);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(response_json(replay).await["replayed"], true);
        let workers = get_json(&app, "/api/v1/workers").await;
        let displaced = workers["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["worker"]["id"] == replaced_worker_id)
            .unwrap();
        assert_eq!(displaced["availability"], "unassigned_live");
        assert!(!displaced["worker"]["runtime"].is_null());

        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("orchestrator terminal lease was not revoked")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "orchestrator_changed");
        server.abort();
    }

    #[tokio::test]
    async fn project_orchestrator_transfer_http_rejects_stale_cross_workspace_and_unavailable() {
        let (app, temp, _runtime) = handoff_test_router().await;
        let (uri, command, _project_id) = create_orchestrator_transfer_request(&app).await;

        let mut stale = command.clone();
        stale["command_id"] = serde_json::json!("stale-transfer");
        stale["expected_project_version"] = serde_json::json!(
            (command["expected_project_version"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                + 1)
            .to_string()
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(stale.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "project_orchestrator_transfer_conflict"
        );

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE worker_runtime_bindings
                    SET runtime_workspace_id = 'workspace-other'
                  WHERE worker_id = ?1",
                [command["worker_id"].as_str().unwrap()],
            )
            .unwrap();
        drop(connection);
        let mut cross_workspace = command.clone();
        cross_workspace["command_id"] = serde_json::json!("cross-workspace-transfer");
        cross_workspace["expected_worker_runtime"]["workspace_id"] =
            serde_json::json!("workspace-other");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(cross_workspace.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "project_orchestrator_identity_changed"
        );

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE worker_runtime_bindings
                    SET process_state = 'exited'
                  WHERE worker_id = ?1",
                [command["worker_id"].as_str().unwrap()],
            )
            .unwrap();
        drop(connection);
        cross_workspace["command_id"] = serde_json::json!("unavailable-transfer");
        cross_workspace["expected_worker_runtime"]["process_state"] = serde_json::json!("exited");
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(cross_workspace.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "project_orchestrator_transfer_conflict"
        );
    }

    #[tokio::test]
    async fn replacement_cutover_revokes_open_orchestrator_terminal_lease() {
        let (app, _temp, _runtime) = handoff_test_router().await;
        let (uri, command, project_id) = create_orchestrator_replacement_request(&app).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let terminal_url = format!(
            "ws://{address}/api/v1/projects/{project_id}/orchestrator/terminal?cols=80&rows=24"
        );
        let mut terminal_request = terminal_url.into_client_request().unwrap();
        terminal_request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(terminal_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let replaced = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replaced.status(), StatusCode::OK);
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("orchestrator terminal lease was not revoked")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "orchestrator_changed");
        server.abort();
    }

    #[tokio::test]
    async fn persists_handoff_runtime_claim_before_starting_agent() {
        let (app, _temp, runtime) = handoff_test_router().await;
        let (uri, command) = create_handoff_request(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(
            runtime.claim_seen_before_start.load(Ordering::SeqCst),
            "start was reached before the target runtime claim was durably visible"
        );
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        let retirements = runtime.retirement_calls.lock().unwrap();
        assert_eq!(retirements.len(), 1);
        assert_eq!(
            retirements[0].terminal_id,
            "terminal-yard-promptallocationcommand"
        );
        assert_eq!(
            retirements[0].tab_id.as_deref(),
            Some("tab-yard-promptallocationcommand")
        );
        assert!(retirements[0].owns_tab);
    }

    #[tokio::test]
    async fn handoff_http_replays_success_and_rejects_conflicting_command_reuse() {
        let (app, _temp, runtime) = handoff_test_router().await;
        let (uri, command) = create_handoff_request(&app).await;

        let created_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created_response.status(), StatusCode::CREATED);
        let created = response_json(created_response).await;
        assert_eq!(created["replayed"], false);

        let replay_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay_response.status(), StatusCode::OK);
        let replayed = response_json(replay_response).await;
        assert_eq!(replayed["replayed"], true);
        assert_eq!(replayed["assignment"]["id"], created["assignment"]["id"]);

        let mut conflicting = command;
        conflicting["objective"] =
            serde_json::Value::String("Conflicting handoff objective.".to_owned());
        let conflict_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(conflicting.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict_response.status(), StatusCode::CONFLICT);
        let conflict = response_json(conflict_response).await;
        assert_eq!(conflict["error"]["code"], "idempotency_conflict");
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn injects_status_contract_only_for_orchestrator_handoff_delivery() {
        let (app, _temp, runtime) = handoff_test_router().await;
        let (uri, mut command) = create_handoff_request(&app).await;
        command["target_role"] = serde_json::Value::String("orchestrator".to_owned());
        command["role"] = serde_json::Value::String("orchestrator".to_owned());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let requests = runtime.start_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].prompt.contains("command \"handoff-command-1\""));
        assert!(requests[0].prompt.contains("There is no completed state"));
        assert!(
            yard_domain::OrchestratorStatusReport::scan_terminal_output(&requests[0].prompt)
                .is_none()
        );
    }

    #[tokio::test]
    async fn cleanup_failure_does_not_rollback_handoff_and_retries_after_restart() {
        let (app, temp, runtime) = handoff_test_router_with_retirement_failures(1).await;
        let (uri, command) = create_handoff_request(&app).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(runtime.retirement_calls.lock().unwrap().len(), 1);

        let database_path = temp.path().join("yard.sqlite3");
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        let (status, attempts, error): (String, i64, Option<String>) = connection
            .query_row(
                "SELECT status, attempts, last_error
                   FROM runtime_cleanup_jobs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(attempts, 1);
        assert_eq!(error.as_deref(), Some("simulated Herdr close failure"));
        connection
            .execute(
                "UPDATE runtime_cleanup_jobs SET next_attempt_at_unix_ms = 0",
                [],
            )
            .unwrap();
        drop(connection);
        drop(app);

        let reopened = Arc::new(SqliteProjectStore::open(&database_path).await.unwrap());
        let cleanup = RuntimeCleanupService::new(runtime.clone(), reopened);
        let report = cleanup.process_pending().await.unwrap();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(runtime.retirement_calls.lock().unwrap().len(), 2);

        let connection = rusqlite::Connection::open(database_path).unwrap();
        let (status, attempts, completed): (String, i64, bool) = connection
            .query_row(
                "SELECT status, attempts, completed_at_unix_ms IS NOT NULL
                   FROM runtime_cleanup_jobs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "succeeded");
        assert_eq!(attempts, 2);
        assert!(completed);
    }

    #[tokio::test]
    async fn handoff_revokes_open_source_terminal_and_output_access() {
        let (app, _temp, _runtime) = handoff_test_router().await;
        let (uri, command) = create_handoff_request(&app).await;
        let segments = uri.split('/').collect::<Vec<_>>();
        let project_id = segments[4].to_owned();
        let assignment_id = segments[6].to_owned();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let terminal_url = format!(
            "ws://{address}/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal?cols=80&rows=24"
        );
        let mut terminal_request = terminal_url.into_client_request().unwrap();
        terminal_request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(terminal_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let handoff = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(handoff.status(), StatusCode::CREATED);

        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("source terminal lease was not revoked after handoff")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "assignment_changed");

        let output = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal-output"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(output.status(), StatusCode::CONFLICT);
        let error = response_json(output).await;
        assert_eq!(error["error"]["code"], "assignment_not_active");

        server.abort();
    }

    #[tokio::test]
    async fn orchestrator_replacement_revokes_open_project_terminal() {
        let (app, _temp, _runtime) = handoff_test_router().await;
        let (uri, mut command) = create_handoff_request(&app).await;
        command["target_role"] = serde_json::Value::String("orchestrator".to_owned());
        let target_project_id = command["target_project_id"].as_str().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let terminal_url = format!(
            "ws://{address}/api/v1/projects/{target_project_id}/orchestrator/terminal?cols=80&rows=24"
        );
        let mut terminal_request = terminal_url.into_client_request().unwrap();
        terminal_request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(terminal_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let handoff = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(handoff.status(), StatusCode::CREATED);

        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("orchestrator terminal lease was not revoked")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "orchestrator_changed");

        server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn yard_orchestrator_replacement_revokes_open_singleton_terminal() {
        let (app, _temp) = test_router().await;
        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory.status(), StatusCode::OK);
        let initial = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/yard/orchestrator")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let candidates = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/workers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let candidates = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|candidate| candidate["availability"] == "unassigned_live")
            .take(2)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(candidates.len(), 2);

        let first_configure = serde_json::json!({
            "command_id": "configure-yard-terminal-first",
            "actor": "local-user",
            "worker_id": candidates[0]["worker"]["id"],
            "expected_worker_version": candidates[0]["worker"]["version"],
            "expected_orchestrator_version": initial["version"],
        });
        let configured = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::PUT)
                        .uri("/api/v1/yard/orchestrator")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(first_configure.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let terminal_url =
            format!("ws://{address}/api/v1/yard/orchestrator/terminal?cols=80&rows=24");
        let mut terminal_request = terminal_url.into_client_request().unwrap();
        terminal_request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(terminal_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let replacement = serde_json::json!({
            "command_id": "configure-yard-terminal-replacement",
            "actor": "local-user",
            "worker_id": candidates[1]["worker"]["id"],
            "expected_worker_version": candidates[1]["worker"]["version"],
            "expected_orchestrator_version": configured["orchestrator"]["version"],
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/yard/orchestrator")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(replacement.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("Yard orchestrator terminal lease was not revoked")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "yard_orchestrator_changed");

        server.abort();
    }

    #[tokio::test]
    async fn sessions_are_not_cached() {
        let (app, _temp) = test_router().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let json = response_json(response).await;
        assert_eq!(json["sessions"][0]["name"], "default");
    }

    #[tokio::test]
    async fn relays_interactive_terminal_frames_commands_and_release() {
        let (app, _temp) = test_router().await;
        let (project_id, assignment_id) = create_active_assignment(&app).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app).into_future());
        let url = format!(
            "ws://{address}/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal?cols=80&rows=24"
        );
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

        let frame = socket.next().await.unwrap().unwrap();
        let TungsteniteMessage::Text(frame) = frame else {
            panic!("expected a terminal frame");
        };
        let frame: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["type"], "terminal.frame");
        assert_eq!(frame["seq"], 1);

        for message in [
            serde_json::json!({"type": "terminal.input", "text": "echo ready\r"}),
            serde_json::json!({"type": "terminal.resize", "cols": 90, "rows": 30}),
            serde_json::json!({"type": "terminal.release"}),
        ] {
            socket
                .send(TungsteniteMessage::Text(message.to_string().into()))
                .await
                .unwrap();
        }
        let closed = socket.next().await.unwrap().unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "released");

        server.abort();
    }

    #[tokio::test]
    async fn shutdown_closes_active_terminal_websocket_and_drains_tracker() {
        let (app, _temp, shutdown, connections) = shutdown_test_router().await;
        let (project_id, assignment_id) = create_active_assignment(&app).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut shutdown_receiver = shutdown.subscribe();
        let server = tokio::spawn(
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_receiver.wait_for(|requested| *requested).await;
                })
                .into_future(),
        );
        let url = format!(
            "ws://{address}/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal?cols=80&rows=24"
        );
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        connections.close();
        shutdown.send(true).unwrap();
        let closed = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("terminal did not close during shutdown")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "server_shutdown");
        timeout(Duration::from_secs(2), connections.wait_for_idle())
            .await
            .expect("connection tracker did not drain");
        timeout(Duration::from_secs(2), server)
            .await
            .expect("server did not stop")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn revokes_open_terminal_and_output_after_assignment_completion() {
        let (app, _temp) = test_router().await;
        let (project_id, assignment_id) = create_active_assignment(&app).await;
        let assignments = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/projects/{project_id}/assignments"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let assignment = assignments["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|assignment| assignment["id"] == assignment_id)
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let url = format!(
            "ws://{address}/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal?cols=80&rows=24"
        );
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let completion = serde_json::json!({
            "command_id": "completion-revokes-terminal",
            "actor": "local-user",
            "attempt_id": assignment["attempt"]["id"],
            "expected_assignment_version": assignment["version"],
            "expected_attempt_version": assignment["attempt"]["version"],
            "outcome": "completed",
            "summary": "Verified terminal lease revocation.",
            "artifact_refs": [],
            "evidence_refs": ["test://terminal-lease-revocation"],
            "unresolved_blockers": []
        });
        let completion_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/projects/{project_id}/assignments/{assignment_id}/completion-receipts"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(completion.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completion_response.status(), StatusCode::CREATED);

        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("stale terminal lease was not revoked")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "assignment_changed");

        let output_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal-output"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(output_response.status(), StatusCode::CONFLICT);
        let output_error = response_json(output_response).await;
        assert_eq!(output_error["error"]["code"], "assignment_not_active");

        server.abort();
    }

    #[tokio::test]
    async fn selected_session_is_forwarded() {
        let (app, _temp) = test_router().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = response_json(response).await;
        assert_eq!(json["session"], "default");
        assert_eq!(json["protocol"], 19);
    }

    #[tokio::test]
    async fn inventory_get_reconciles_observed_workers_durably() {
        let (app, temp) = test_router().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();
        let adoption_event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE event_type = 'runtime_worker_adopted'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(worker_count, 3);
        assert_eq!(adoption_event_count, 3);
    }

    #[tokio::test]
    async fn concurrent_inventory_gets_share_one_snapshot_and_reconciliation() {
        let source = Arc::new(CountingInventory {
            inventory_calls: AtomicUsize::new(0),
        });
        let (app, _temp) = test_router_with_source(source.clone()).await;
        let uri = "/api/v1/runtimes/herdr/sessions/default/inventory";
        let responses = join_all((0..16).map(|_| {
            app.clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        }))
        .await;

        for response in responses {
            assert_eq!(response.unwrap().status(), StatusCode::OK);
        }
        assert_eq!(source.inventory_calls.load(Ordering::SeqCst), 1);

        tokio::time::sleep(crate::reconciliation_service::RECONCILIATION_INTERVAL).await;
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(source.inventory_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn transient_inventory_failure_preserves_durable_projection() {
        let snapshot = FakeInventory.inventory("default").await.unwrap();
        let source = Arc::new(FlakyInventory {
            snapshot,
            inventory_calls: AtomicUsize::new(0),
        });
        let (app, temp) = test_router_with_source(source).await;
        let uri = "/api/v1/runtimes/herdr/sessions/default/inventory";

        let first = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let before = runtime_binding_rows(&temp);
        assert_eq!(before.len(), 3);

        tokio::time::sleep(crate::reconciliation_service::RECONCILIATION_INTERVAL).await;
        let failed = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
        let error = response_json(failed).await;
        assert_eq!(error["error"]["code"], "herdr_snapshot_failed");
        assert_eq!(runtime_binding_rows(&temp), before);
    }

    #[tokio::test]
    async fn creates_and_lists_runtime_validated_project() {
        let (app, _temp) = test_router().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body("terminal-1")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let created = response_json(response).await;
        assert_eq!(created["version"], "1");
        assert_eq!(created["placement"]["version"], "1");
        assert_eq!(
            created["orchestrator"]["runtime"]["terminal_id"],
            "terminal-1"
        );
        assert_ne!(created["id"], created["orchestrator"]["id"]);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(location)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let fetched = response_json(response).await;
        assert_eq!(fetched["id"], created["id"]);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let projects = response_json(response).await;

        assert_eq!(projects["projects"][0]["id"], created["id"]);
    }

    #[tokio::test]
    async fn creates_profile_backed_project_once_and_replays_without_reprovisioning() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(CountingRuntime::default());
        let interactive = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            Arc::new(ProjectCreationInventory),
            runtime.clone(),
            interactive.clone(),
            interactive,
            store,
            artifacts,
        );
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Orchestrator")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let body = profile_project_body(profile["id"].as_str().unwrap());
        let created_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects/from-profile")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(created_response.status(), StatusCode::CREATED);
        assert!(
            created_response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .starts_with("/api/v1/projects/")
        );
        let created = response_json(created_response).await;
        assert!(!created["replayed"].as_bool().unwrap());
        assert_eq!(
            created["project"]["orchestrator"]["profile_id"],
            profile["id"]
        );
        assert_eq!(
            created["project"]["orchestrator"]["runtime"]["terminal_id"],
            "terminal-yard-profileprojectcommand"
        );
        assert_eq!(runtime.provision_calls.load(Ordering::SeqCst), 1);
        {
            let requests = runtime.provision_requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(
                requests[0]
                    .prompt
                    .contains("command \"profile-project-command\"")
            );
            assert!(requests[0].prompt.contains("There is no completed state"));
            assert!(
                yard_domain::OrchestratorStatusReport::scan_terminal_output(&requests[0].prompt)
                    .is_none()
            );
        }

        let replay_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects/from-profile")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(replay_response.status(), StatusCode::OK);
        let replayed = response_json(replay_response).await;
        assert!(replayed["replayed"].as_bool().unwrap());
        assert_eq!(replayed["project"]["id"], created["project"]["id"]);
        assert_eq!(runtime.provision_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn creates_workspace_backed_project_once_and_replays_without_reprovisioning() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(CountingRuntime::default());
        let interactive = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            Arc::new(WorkspaceProjectCreationInventory),
            runtime.clone(),
            interactive.clone(),
            interactive,
            store,
            artifacts,
        );
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Orchestrator")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let body = workspace_project_body(profile["id"].as_str().unwrap());

        let created_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects/from-profile/workspace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(created_response.status(), StatusCode::CREATED);
        let created = response_json(created_response).await;
        assert!(!created["replayed"].as_bool().unwrap());
        assert_eq!(
            created["project"]["runtime"]["workspace_id"],
            "workspace-created"
        );
        assert_eq!(
            created["project"]["orchestrator"]["runtime"]["terminal_id"],
            "terminal-yard-workspaceprojectcommand"
        );
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        {
            let requests = runtime.bootstrap_requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(
                requests[0]
                    .prompt
                    .contains("command \"workspace-project-command\"")
            );
            assert!(requests[0].prompt.contains("There is no completed state"));
            assert!(
                yard_domain::OrchestratorStatusReport::scan_terminal_output(&requests[0].prompt)
                    .is_none()
            );
        }

        let replay_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects/from-profile/workspace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(replay_response.status(), StatusCode::OK);
        let replayed = response_json(replay_response).await;
        assert!(replayed["replayed"].as_bool().unwrap());
        assert_eq!(replayed["project"]["id"], created["project"]["id"]);
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ambiguous_workspace_creation_is_never_retried() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(AmbiguousBootstrapRuntime::default());
        let interactive = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            Arc::new(FakeInventory),
            runtime.clone(),
            interactive.clone(),
            interactive,
            store,
            artifacts,
        );
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Orchestrator")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let body = workspace_project_body(profile["id"].as_str().unwrap());

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects/from-profile/workspace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(first).await["error"]["code"],
            "command_outcome_ambiguous"
        );

        let replay = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects/from-profile/workspace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(replay.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(replay).await["error"]["code"],
            "command_outcome_ambiguous"
        );
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn creates_updates_and_lists_worker_profiles() {
        let (app, _temp) = test_router().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/worker-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(profile_body("Implementer")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .starts_with("/api/v1/worker-profiles/")
        );
        let created = response_json(response).await;
        let profile_id = created["id"].as_str().unwrap();
        assert_eq!(created["version"], "1");
        assert_eq!(created["provider"], "codex");
        assert!(created.get("manifest").is_none());

        let mut update =
            serde_json::from_str::<serde_json::Value>(&profile_body("Reviewer")).unwrap();
        update["expected_version"] = serde_json::Value::String("1".to_owned());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/worker-profiles/{profile_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let updated = response_json(response).await;
        assert_eq!(updated["name"], "Reviewer");
        assert_eq!(updated["version"], "2");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/worker-profiles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let profiles = response_json(response).await;
        assert_eq!(profiles["profiles"][0]["id"], profile_id);
        assert_eq!(profiles["profiles"][0]["version"], "2");

        let portable = get_json(
            &app,
            &format!("/api/v1/agent-profiles/{profile_id}/revisions/2"),
        )
        .await;
        assert_eq!(portable["id"], profile_id);
        assert_eq!(portable["version"], "2");
        assert_eq!(portable["manifest"]["metadata"]["name"], "Reviewer");
    }

    #[tokio::test]
    async fn imports_updates_and_exports_agent_profile_revisions() {
        let (app, _temp) = test_router().await;
        let manifest = agent_profile_fixture();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agent-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(agent_profile_body(&manifest)))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let created = response_json(response).await;
        let profile_id = created["id"].as_str().unwrap();
        assert_eq!(location, format!("/api/v1/agent-profiles/{profile_id}"));
        assert_eq!(created["version"], "1");
        assert_eq!(created["manifest"], manifest);
        assert_eq!(created["validation"]["results"][3]["status"], "unsupported");

        let mut replacement = agent_profile_fixture();
        replacement["metadata"]["name"] = "Portable reviewer".into();
        replacement["spec"]["extensions"]["io.example.provider"]["futureSetting"] = 23.into();
        let update = serde_json::json!({
            "expected_version": "1",
            "manifest": replacement,
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/agent-profiles/{profile_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let updated = response_json(response).await;
        assert_eq!(updated["version"], "2");
        assert_eq!(updated["manifest"]["metadata"]["name"], "Portable reviewer");
        assert_eq!(
            updated["manifest"]["spec"]["extensions"]["io.example.provider"]["futureSetting"],
            23
        );

        let original = get_json(
            &app,
            &format!("/api/v1/agent-profiles/{profile_id}/revisions/1"),
        )
        .await;
        let worker = get_json(&app, &format!("/api/v1/worker-profiles/{profile_id}")).await;
        let profiles = get_json(&app, "/api/v1/agent-profiles").await;

        assert_eq!(original["manifest"], manifest);
        assert_eq!(worker["id"], profile_id);
        assert_eq!(worker["name"], "Portable reviewer");
        assert_eq!(worker["version"], "2");
        assert_eq!(profiles["profiles"][0]["id"], profile_id);
        assert_eq!(profiles["profiles"][0]["version"], "2");

        let mut incompatible_legacy_update =
            serde_json::from_str::<serde_json::Value>(&profile_body("Portable reviewer")).unwrap();
        incompatible_legacy_update["runtime_adapter"] = "other-runtime".into();
        incompatible_legacy_update["expected_version"] = "2".into();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/worker-profiles/{profile_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(incompatible_legacy_update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "invalid_worker_profile"
        );
        assert_eq!(
            get_json(&app, &format!("/api/v1/agent-profiles/{profile_id}")).await["version"],
            "2"
        );
    }

    #[tokio::test]
    async fn gets_updates_resets_and_conflicts_orchestrator_workflow_profile() {
        let (app, _temp) = test_router().await;
        let factory = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/orchestrator-workflow-profile")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(factory["version"], "1");
        assert_eq!(factory["monitor_interval_ms"], "600000");
        assert!(
            factory["instructions_markdown"]
                .as_str()
                .unwrap()
                .contains("one Yard/Herdr worker per lane")
        );

        let edited = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::PUT)
                        .uri("/api/v1/orchestrator-workflow-profile")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "actor": "local-user",
                                "expected_version": factory["version"],
                                "instructions_markdown": "# Custom workflow\n\nUse two lanes.",
                                "monitor_interval_ms": "900000"
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(edited["version"], "2");
        assert_eq!(edited["source"], "user");

        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/orchestrator-workflow-profile/reset")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "actor": "local-user",
                            "expected_version": factory["version"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(stale).await["error"]["code"],
            "orchestrator_workflow_profile_version_conflict"
        );

        let reset = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/orchestrator-workflow-profile/reset")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "actor": "local-user",
                                "expected_version": edited["version"]
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(reset["version"], "3");
        assert_eq!(reset["source"], "reset");
        assert_eq!(reset["monitor_interval_ms"], "600000");
        assert!(
            reset["instructions_markdown"]
                .as_str()
                .unwrap()
                .contains("Automatic token-spending behavior remains opt-in")
        );
        assert_automatic_token_spend_defaults_off(&app).await;
    }

    #[tokio::test]
    async fn agent_profile_import_fails_closed_for_unsupported_required_capability() {
        let (app, _temp) = test_router().await;
        let mut manifest = agent_profile_fixture();
        manifest["spec"]["capabilities"]["required"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": "agent.skills",
                "version": 1
            }));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agent-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(agent_profile_body(&manifest)))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "invalid_agent_profile"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/agent-profiles/profile-1/revisions/not-a-number")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "invalid_agent_profile_revision"
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn ends_an_unallocated_worker_session_and_prevents_re_adoption() {
        let (app, _temp) = test_router().await;
        let inventory_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory_response.status(), StatusCode::OK);

        let project = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body("terminal-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(project.status(), StatusCode::CREATED);

        let candidates = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/workers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let orchestrator = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["availability"] == "orchestrator")
            .unwrap();
        let orchestrator_id = orchestrator["worker"]["id"].as_str().unwrap();
        let forbidden = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/workers/{orchestrator_id}/end-session"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "command_id": "end-orchestrator-session",
                            "actor": "local-user",
                            "expected_worker_version": orchestrator["worker"]["version"],
                            "expected_runtime_version":
                                orchestrator["worker"]["runtime"]["version"],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(forbidden).await["error"]["code"],
            "orchestrator_replacement_required"
        );

        let candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["worker"]["runtime"]["terminal_id"]
                    == "terminal-yard-promptallocationcommand"
            })
            .unwrap();
        let worker_id = candidate["worker"]["id"].as_str().unwrap();
        let command = serde_json::json!({
            "command_id": "end-worker-session-1",
            "actor": "local-user",
            "expected_worker_version": candidate["worker"]["version"],
            "expected_runtime_version": candidate["worker"]["runtime"]["version"],
        });

        let ended = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/workers/{worker_id}/end-session"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ended.status(), StatusCode::OK);
        let ended = response_json(ended).await;
        assert_eq!(ended["worker"]["desired_state"], "ended");
        assert!(ended["worker"]["runtime"].is_null());
        assert_eq!(ended["cleanup_pending"], false);
        assert_eq!(ended["replayed"], false);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/workers/{worker_id}/end-session"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(response_json(replay).await["replayed"], true);

        let mut conflicting_actor = command;
        conflicting_actor["actor"] = serde_json::Value::String("other-user".to_owned());
        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/workers/{worker_id}/end-session"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(conflicting_actor.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(conflict).await["error"]["code"],
            "idempotency_conflict"
        );

        let inventory_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory_response.status(), StatusCode::OK);
        let candidates = response_json(
            app.oneshot(
                Request::builder()
                    .uri("/api/v1/workers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        let ended_candidates = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|candidate| candidate["worker"]["id"] == worker_id)
            .collect::<Vec<_>>();
        assert_eq!(ended_candidates.len(), 1);
        assert_eq!(ended_candidates[0]["availability"], "ended");
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn lists_and_assigns_an_existing_live_worker() {
        let (app, _temp) = test_router().await;
        let inventory_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory_response.status(), StatusCode::OK);

        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Implementer")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let candidates_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/workers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(candidates_response.status(), StatusCode::OK);
        let candidates = response_json(candidates_response).await;
        let candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["worker"]["runtime"]["terminal_id"]
                    == "terminal-yard-promptallocationcommand"
            })
            .unwrap();
        assert_eq!(candidate["availability"], "unassigned_live");
        let command = serde_json::json!({
            "command_id": "assign-existing-live",
            "actor": "local-user",
            "worker_id": candidate["worker"]["id"],
            "expected_worker_version": candidate["worker"]["version"],
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_project_version": project["version"],
            "objective": "Continue in the existing Herdr worker.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/projects/{}/assignments",
                        project["id"].as_str().unwrap()
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let assigned = response_json(response).await;
        assert_eq!(
            assigned["assignment"]["worker"]["id"],
            candidate["worker"]["id"]
        );
        assert_eq!(assigned["allocation"]["mode"], "adopt_existing");
        assert_eq!(assigned["assignment"]["lifecycle"], "active");

        let candidates = response_json(
            app.oneshot(
                Request::builder()
                    .uri("/api/v1/workers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        let assigned_candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate_after| candidate_after["worker"]["id"] == candidate["worker"]["id"])
            .unwrap();
        assert_eq!(assigned_candidate["availability"], "assigned");
        assert_eq!(
            assigned_candidate["assignment_id"],
            assigned["assignment"]["id"]
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn resumes_worker_with_replacement_runtime_and_stable_identity() {
        let (app, temp) = test_router().await;
        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let project_id = project["id"].as_str().unwrap();
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Implementer")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let first_command = serde_json::json!({
            "command_id": "allocation-command-1",
            "actor": "local-user",
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_project_version": project["version"],
            "objective": "Complete the first assignment.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let assignment_uri = format!("/api/v1/projects/{project_id}/assignments");
        let first = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(&assignment_uri)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(first_command.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let worker_id = first["assignment"]["worker"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let assignment_id = first["assignment"]["id"].as_str().unwrap();
        let completion = serde_json::json!({
            "command_id": "complete-before-http-resume",
            "actor": "local-user",
            "attempt_id": first["assignment"]["attempt"]["id"],
            "expected_assignment_version": first["assignment"]["version"],
            "expected_attempt_version": first["assignment"]["attempt"]["version"],
            "outcome": "completed",
            "summary": "The first assignment is complete.",
            "artifact_refs": [],
            "evidence_refs": ["test://http-resume"],
            "unresolved_blockers": []
        });
        let completion_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/projects/{project_id}/assignments/{assignment_id}/completion-receipts"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(completion.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completion_response.status(), StatusCode::CREATED);

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE worker_runtime_bindings
                    SET process_state = 'exited', observed_status = 'unknown'
                  WHERE worker_id = ?1",
                [&worker_id],
            )
            .unwrap();
        drop(connection);
        let candidates = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/workers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["worker"]["id"] == worker_id)
            .unwrap();
        assert_eq!(candidate["availability"], "resumable");
        let current_project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/projects/{project_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let resume = serde_json::json!({
            "command_id": "prompt-allocation-command",
            "actor": "local-user",
            "worker_id": worker_id,
            "expected_worker_version": candidate["worker"]["version"],
            "expected_project_version": current_project["version"],
            "objective": "Continue with the resumed worker.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(assignment_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(resume.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let resumed = response_json(response).await;
        assert_eq!(resumed["assignment"]["worker"]["id"], worker_id);
        assert_eq!(
            resumed["assignment"]["worker"]["runtime"]["terminal_id"],
            "terminal-yard-promptallocationcommand"
        );
        assert_eq!(resumed["assignment"]["lifecycle"], "active");
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn confirms_allocation_and_replays_evidence_backed_completion() {
        let (app, _temp) = test_router().await;
        let project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body("terminal-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let project = response_json(project_response).await;
        let project_id = project["id"].as_str().unwrap();
        let profile_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/worker-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(profile_body("Implementer")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let profile = response_json(profile_response).await;
        let command = serde_json::json!({
            "command_id": "allocation-command-1",
            "actor": "local-user",
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_project_version": project["version"],
            "objective": "Implement the allocation endpoint.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let uri = format!("/api/v1/projects/{project_id}/assignments");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = response_json(response).await;
        assert_eq!(created["assignment"]["lifecycle"], "active");
        assert_eq!(created["assignment"]["attempt"]["lifecycle"], "active");
        assert_eq!(created["assignment"]["worker"]["runtime"]["owns_tab"], true);
        assert_eq!(
            created["assignment"]["completion_receipt"],
            serde_json::Value::Null
        );
        assert_eq!(created["replayed"], false);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replayed = response_json(replay).await;
        assert_eq!(replayed["assignment"]["id"], created["assignment"]["id"]);
        assert_eq!(replayed["replayed"], true);

        let assignment_id = created["assignment"]["id"].as_str().unwrap();
        let completion_uri = format!(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/completion-receipts"
        );
        let completion_command = serde_json::json!({
            "command_id": "completion-command-1",
            "actor": "local-user",
            "attempt_id": created["assignment"]["attempt"]["id"],
            "expected_assignment_version": created["assignment"]["version"],
            "expected_attempt_version": created["assignment"]["attempt"]["version"],
            "outcome": "completed",
            "summary": "Implemented and verified the endpoint.",
            "artifact_refs": ["yard://artifacts/change-set"],
            "evidence_refs": ["test://cargo-test"],
            "unresolved_blockers": []
        });
        let invalid_completion = serde_json::json!({
            "command_id": "invalid-completion-command",
            "actor": "local-user",
            "attempt_id": created["assignment"]["attempt"]["id"],
            "expected_assignment_version": created["assignment"]["version"],
            "expected_attempt_version": created["assignment"]["attempt"]["version"],
            "outcome": "completed",
            "summary": "No evidence was supplied.",
            "artifact_refs": [],
            "evidence_refs": [],
            "unresolved_blockers": []
        });
        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&completion_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(invalid_completion.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let invalid = response_json(invalid).await;
        assert_eq!(invalid["error"]["code"], "invalid_completion_receipt");
        let completion = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&completion_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(completion_command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completion.status(), StatusCode::CREATED);
        let completed = response_json(completion).await;
        assert_eq!(completed["assignment"]["lifecycle"], "completed");
        assert_eq!(completed["assignment"]["attempt"]["lifecycle"], "completed");
        assert_eq!(completed["receipt"]["outcome"], "completed");
        assert_eq!(
            completed["receipt"]["evidence_refs"][0],
            "test://cargo-test"
        );
        assert_eq!(completed["replayed"], false);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&completion_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(completion_command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replayed = response_json(replay).await;
        assert_eq!(replayed["receipt"]["id"], completed["receipt"]["id"]);
        assert_eq!(replayed["replayed"], true);

        let conflicting_command = serde_json::json!({
            "command_id": "completion-command-1",
            "actor": "local-user",
            "attempt_id": created["assignment"]["attempt"]["id"],
            "expected_assignment_version": created["assignment"]["version"],
            "expected_attempt_version": created["assignment"]["attempt"]["version"],
            "outcome": "completed",
            "summary": "Different input.",
            "artifact_refs": ["yard://artifacts/change-set"],
            "evidence_refs": ["test://cargo-test"],
            "unresolved_blockers": []
        });
        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&completion_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(conflicting_command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let conflict = response_json(conflict).await;
        assert_eq!(conflict["error"]["code"], "idempotency_conflict");

        let assignments = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let assignments = response_json(assignments).await;
        assert_eq!(assignments["assignments"].as_array().unwrap().len(), 1);
        assert_eq!(
            assignments["assignments"][0]["worker"]["id"],
            created["assignment"]["worker"]["id"]
        );
        assert_eq!(
            assignments["assignments"][0]["completion_receipt"]["id"],
            completed["receipt"]["id"]
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn uploads_inspects_and_links_a_typed_artifact() {
        let (app, temp) = test_router().await;
        let (project_id, assignment_id) = create_active_assignment(&app).await;
        let assignments = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/projects/{project_id}/assignments"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let assignment = &assignments["assignments"][0];
        let artifact_id = "019ff1a2-0000-7000-8000-000000000001";
        let artifact_uri = format!(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/artifacts/{artifact_id}"
        );
        let content = "<h1>Release report</h1><script>parent.pwned = true</script>";
        let upload = serde_json::json!({
            "actor": "local-user",
            "attempt_id": assignment["attempt"]["id"],
            "expected_assignment_version": assignment["version"],
            "expected_attempt_version": assignment["attempt"]["version"],
            "kind": "html",
            "display_name": "release-report.html",
            "content": content
        });

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&artifact_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(upload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        assert_eq!(created["id"], artifact_id);
        assert_eq!(created["kind"], "html");
        assert_eq!(created["media_type"], "text/html");
        assert_eq!(created["display_name"], "release-report.html");
        assert_eq!(created["assignment_id"], assignment_id);
        assert_eq!(created["attempt_id"], assignment["attempt"]["id"]);
        assert_eq!(created["sha256"].as_str().unwrap().len(), 64);
        assert!(temp.path().join("artifacts/01").join(artifact_id).is_file());

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&artifact_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(upload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);

        let mut conflicting_upload = upload.clone();
        conflicting_upload["content"] = serde_json::json!("<h1>Different</h1>");
        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&artifact_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(conflicting_upload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(conflict).await["error"]["code"],
            "artifact_id_conflict"
        );

        let content_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{artifact_uri}/content"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(content_response.status(), StatusCode::OK);
        assert_eq!(
            content_response.headers()[header::CACHE_CONTROL],
            "no-store"
        );
        assert_eq!(
            content_response.headers()["x-content-type-options"],
            "nosniff"
        );
        assert!(
            content_response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("default-src 'none'")
        );
        let inspected = response_json(content_response).await;
        assert_eq!(inspected["content"], content);

        let completion = serde_json::json!({
            "command_id": "typed-artifact-completion",
            "actor": "local-user",
            "attempt_id": assignment["attempt"]["id"],
            "expected_assignment_version": assignment["version"],
            "expected_attempt_version": assignment["attempt"]["version"],
            "outcome": "completed",
            "summary": "Published and inspected the release report.",
            "artifact_refs": [],
            "artifact_ids": [artifact_id],
            "evidence_refs": [],
            "unresolved_blockers": []
        });
        let completion_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/projects/{project_id}/assignments/{assignment_id}/completion-receipts"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(completion.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completion_response.status(), StatusCode::CREATED);
        let completed = response_json(completion_response).await;
        assert_eq!(completed["receipt"]["artifact_refs"], serde_json::json!([]));
        assert_eq!(completed["receipt"]["artifacts"][0]["id"], artifact_id);
        assert_eq!(
            completed["receipt"]["artifacts"][0]["display_name"],
            "release-report.html"
        );
    }

    #[tokio::test]
    async fn rejects_orchestrator_missing_from_runtime_snapshot() {
        let (app, _temp) = test_router().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body("missing-pane")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = response_json(response).await;
        assert_eq!(error["error"]["code"], "runtime_binding_stale");
    }

    #[tokio::test]
    async fn rejects_duplicate_workspace_adoption() {
        let (app, _temp) = test_router().await;
        for expected in [StatusCode::CREATED, StatusCode::CONFLICT] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::CONFLICT {
                let error = response_json(response).await;
                assert_eq!(error["error"]["code"], "runtime_workspace_already_bound");
            }
        }
    }

    #[tokio::test]
    async fn unknown_project_returns_not_found() {
        let (app, _temp) = test_router().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error = response_json(response).await;
        assert_eq!(error["error"]["code"], "project_not_found");
    }

    #[tokio::test]
    async fn placement_update_rejects_stale_version() {
        let (app, _temp) = test_router().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body("terminal-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let created = response_json(response).await;
        let project_id = created["id"].as_str().unwrap();
        let placement = serde_json::json!({
            "placement": {
                "x": 420.0,
                "y": 110.0,
                "width": 520.0,
                "height": 360.0
            },
            "expected_version": "1"
        });
        let uri = format!("/api/v1/projects/{project_id}/placement");

        let updated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(placement.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);

        let conflict = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(placement.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let error = response_json(conflict).await;
        assert_eq!(error["error"]["code"], "project_version_conflict");
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn prompts_active_assignment_idempotently_and_reads_recent_output() {
        let (app, temp) = test_router().await;
        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Implementer")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let project_id = project["id"].as_str().unwrap();
        let allocation = serde_json::json!({
            "command_id": "prompt-allocation-command",
            "actor": "local-user",
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_project_version": project["version"],
            "objective": "Implement direct intervention.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let created = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/api/v1/projects/{project_id}/assignments"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(allocation.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let assignment = &created["assignment"];
        let assignment_id = assignment["id"].as_str().unwrap();
        let output_uri = format!(
            "/api/v1/projects/{project_id}/assignments/{assignment_id}/terminal-output?lines=120"
        );
        let output = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&output_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(output.status(), StatusCode::OK);
        assert_eq!(
            response_json(output).await["text"],
            "Focused tests are passing."
        );

        let prompt_uri =
            format!("/api/v1/projects/{project_id}/assignments/{assignment_id}/prompts");
        let prompt = serde_json::json!({
            "command_id": "direct-prompt-command",
            "actor": "local-user",
            "attempt_id": assignment["attempt"]["id"],
            "expected_assignment_version": assignment["version"],
            "expected_attempt_version": assignment["attempt"]["version"],
            "text": "Run the focused test and report the result."
        });
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&prompt_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let acknowledged = response_json(first).await;
        assert_eq!(acknowledged["runtime_status"], "working");

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&prompt_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replayed = response_json(replay).await;
        assert_eq!(
            replayed["submitted_at_unix_ms"],
            acknowledged["submitted_at_unix_ms"]
        );

        let mut conflict = prompt;
        conflict["text"] = serde_json::Value::String("Different input.".to_owned());
        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&prompt_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(conflict.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(conflict).await["error"]["code"],
            "idempotency_conflict"
        );

        let invalid_output = app
            .oneshot(
                Request::builder()
                    .uri(output_uri.replace("lines=120", "lines=0"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_output.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let prompt_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM assignment_prompt_commands",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type = 'assignment_prompt_submitted'",
                [assignment_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(prompt_count, 1);
        assert_eq!(event_count, 1);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn configures_and_prompts_the_yard_orchestrator_idempotently() {
        let (app, _temp) = test_router().await;
        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory.status(), StatusCode::OK);
        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;

        let initial = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/yard/orchestrator")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(initial["worker"].is_null());
        let candidates = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/workers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["worker"]["runtime"]["terminal_id"]
                    == "terminal-yard-promptallocationcommand"
            })
            .unwrap();
        let configure = serde_json::json!({
            "command_id": "configure-yard-orchestrator",
            "actor": "local-user",
            "worker_id": candidate["worker"]["id"],
            "expected_worker_version": candidate["worker"]["version"],
            "expected_orchestrator_version": initial["version"],
        });
        let configured = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/yard/orchestrator")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(configure.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(configured.status(), StatusCode::OK);
        let configured = response_json(configured).await;
        assert_eq!(
            configured["orchestrator"]["worker"]["id"],
            candidate["worker"]["id"]
        );

        let output = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/yard/orchestrator/terminal-output?lines=120")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(output.status(), StatusCode::OK);
        assert_eq!(
            response_json(output).await["text"],
            "Focused tests are passing."
        );

        let prompt = serde_json::json!({
            "command_id": "yard-orchestrator-prompt",
            "actor": "local-user",
            "expected_orchestrator_version": configured["orchestrator"]["version"],
            "orchestrator_worker_id": configured["orchestrator"]["worker"]["id"],
            "text": "Review every project and prioritize attention."
        });
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/yard/orchestrator/prompts")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let acknowledged = response_json(first).await;
        let replayed = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/yard/orchestrator/prompts")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(prompt.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            replayed["submitted_at_unix_ms"],
            acknowledged["submitted_at_unix_ms"]
        );

        let route = serde_json::json!({
            "command_id": "route-to-project-orchestrator",
            "actor": "local-user",
            "expected_orchestrator_version": configured["orchestrator"]["version"],
            "orchestrator_worker_id": configured["orchestrator"]["worker"]["id"],
            "target_project_id": project["id"],
            "expected_project_version": project["version"],
            "target_orchestrator_worker_id": project["orchestrator"]["id"],
            "text": "Pick up the highest-priority blocked work."
        });
        let submitted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/yard/orchestrator/routes")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(route.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(submitted.status(), StatusCode::OK);
        let submitted = response_json(submitted).await;
        assert_eq!(submitted["status"], "submitted");
        assert_eq!(submitted["target_project_id"], project["id"]);

        let listed = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/yard/orchestrator/routes?limit=20")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(listed["routes"].as_array().unwrap().len(), 1);
        assert_eq!(listed["routes"][0]["command_id"], submitted["command_id"]);

        let candidates = response_json(
            app.oneshot(
                Request::builder()
                    .uri("/api/v1/workers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(
            candidates["workers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|worker| worker["worker"]["id"] == candidate["worker"]["id"]
                    && worker["availability"] == "yard_orchestrator")
        );
    }

    #[tokio::test]
    async fn provisions_the_dedicated_yard_orchestrator_over_post() {
        let (app, _temp) =
            test_router_with_source(Arc::new(YardOrchestratorProvisionInventory)).await;
        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Central coordinator")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let command = serde_json::json!({
            "command_id": "provision-yard-over-http",
            "actor": "local-user",
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_orchestrator_version": "1",
        });

        let configured = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/yard/orchestrator")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(configured.status(), StatusCode::OK);
        let configured = response_json(configured).await;
        assert_eq!(configured["command_id"], "provision-yard-over-http");
        assert_eq!(
            configured["orchestrator"]["worker"]["profile_id"],
            profile["id"]
        );
        assert_eq!(
            configured["orchestrator"]["worker"]["runtime"]["session"],
            "yard-orchestrator"
        );
        assert_eq!(
            configured["orchestrator"]["worker"]["runtime"]["terminal_id"],
            "terminal-yard-orchestrator"
        );

        let replayed = response_json(
            app.oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/yard/orchestrator")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(command.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(replayed["orchestrator"], configured["orchestrator"]);
        assert_eq!(replayed["replayed"], true);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn delivers_runtime_only_contract_and_exposes_observational_status_reports() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let control = Arc::new(FakeRuntime);
        let reporting = Arc::new(ReportingRuntime::default());
        let terminal = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            Arc::new(FakeInventory),
            control,
            reporting.clone(),
            terminal,
            store,
            artifacts,
        );

        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let project_id = project["id"].as_str().unwrap();

        let profile = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/worker-profiles")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(profile_body("Implementer")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let allocation = serde_json::json!({
            "command_id": "prompt-allocation-command",
            "actor": "local-user",
            "profile_id": profile["id"],
            "expected_profile_version": profile["version"],
            "expected_project_version": project["version"],
            "objective": "Remain active while orchestrator status is observed.",
            "role": "implementer",
            "isolation_policy": "project_workspace"
        });
        let allocated_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/projects/{project_id}/assignments"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(allocation.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let allocated_status = allocated_response.status();
        let allocated = response_json(allocated_response).await;
        assert_eq!(
            allocated_status,
            StatusCode::CREATED,
            "allocation failed: {allocated}"
        );
        let assignment_id = allocated["assignment"]["id"].as_str().unwrap();

        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory.status(), StatusCode::OK);
        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/projects/{project_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let initial_yard = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/yard/orchestrator")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let candidates = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/workers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let yard_candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["worker"]["runtime"]["terminal_id"] == "terminal-yard-allocationcommand1"
            })
            .unwrap();
        let configured = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::PUT)
                        .uri("/api/v1/yard/orchestrator")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "command_id": "configure-status-yard",
                                "actor": "local-user",
                                "worker_id": yard_candidate["worker"]["id"],
                                "expected_worker_version": yard_candidate["worker"]["version"],
                                "expected_orchestrator_version": initial_yard["version"]
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let workflow = get_json(&app, "/api/v1/orchestrator-workflow-profile").await;
        let updated_workflow = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/orchestrator-workflow-profile")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "actor": "local-user",
                            "expected_version": workflow["version"],
                            "instructions_markdown": "# New-current workflow\n\nDo not use the pinned factory text.",
                            "monitor_interval_ms": "900000"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated_workflow.status(), StatusCode::OK);

        let project_prompt_text = "Raw project prompt.";
        let yard_prompt_text = "Raw Yard prompt.";
        let route_prompt_text = "Raw routed prompt.";
        let project_output_uri =
            format!("/api/v1/projects/{project_id}/orchestrator/terminal-output?lines=120");
        let status_line = |command_id: &str, state: &str| {
            serde_json::json!({
                "version": 1,
                "command_id": command_id,
                "state": state,
                "last": "checked current work",
                "next": "continue",
                "blockers": []
            })
            .to_string()
        };
        let assert_null_status_report = |output: &serde_json::Value| {
            assert_eq!(
                output.get("status_report"),
                Some(&serde_json::Value::Null),
                "status_report must be present and null"
            );
        };

        reporting.set_output("pane-1", status_line("forged-before-delivery", "idle"));
        assert_null_status_report(&get_json(&app, &project_output_uri).await);

        let project_prompt = serde_json::json!({
            "command_id": "runtime-project-prompt",
            "actor": "local-user",
            "expected_project_version": project["version"],
            "orchestrator_worker_id": project["orchestrator"]["id"],
            "text": project_prompt_text
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/projects/{project_id}/orchestrator/prompts"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(project_prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let delivered_project_prompt = reporting.prompts.lock().unwrap()[1].text.clone();
        reporting.set_output("pane-1", delivered_project_prompt);
        assert_null_status_report(&get_json(&app, &project_output_uri).await);
        reporting.set_output("pane-1", status_line("forged-project-command", "idle"));
        assert_null_status_report(&get_json(&app, &project_output_uri).await);
        reporting.set_output("pane-1", status_line("runtime-project-prompt", "working"));
        let direct_output = get_json(&app, &project_output_uri).await;
        assert_eq!(
            direct_output["status_report"]["command_id"],
            "runtime-project-prompt"
        );
        assert_eq!(direct_output["status_report"]["state"], "working");

        let yard_prompt = serde_json::json!({
            "command_id": "runtime-yard-prompt",
            "actor": "local-user",
            "expected_orchestrator_version": configured["orchestrator"]["version"],
            "orchestrator_worker_id": configured["orchestrator"]["worker"]["id"],
            "text": yard_prompt_text
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/yard/orchestrator/prompts")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(yard_prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        reporting.set_output(
            "pane-yard-allocationcommand1",
            status_line("forged-yard-command", "idle"),
        );
        assert_null_status_report(
            &get_json(&app, "/api/v1/yard/orchestrator/terminal-output?lines=120").await,
        );
        reporting.set_output(
            "pane-yard-allocationcommand1",
            status_line("runtime-yard-prompt", "needs_attention"),
        );
        let central_output =
            get_json(&app, "/api/v1/yard/orchestrator/terminal-output?lines=120").await;
        assert_eq!(
            central_output["status_report"]["command_id"],
            "runtime-yard-prompt"
        );
        assert_eq!(central_output["status_report"]["state"], "needs_attention");

        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let route = serde_json::json!({
            "command_id": "runtime-yard-route",
            "actor": "local-user",
            "expected_orchestrator_version": configured["orchestrator"]["version"],
            "orchestrator_worker_id": configured["orchestrator"]["worker"]["id"],
            "target_project_id": project["id"],
            "expected_project_version": project["version"],
            "target_orchestrator_worker_id": project["orchestrator"]["id"],
            "text": route_prompt_text
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/yard/orchestrator/routes")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(route.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        reporting.set_output("pane-1", status_line("runtime-project-prompt", "working"));
        assert_null_status_report(&get_json(&app, &project_output_uri).await);
        reporting.set_output("pane-1", status_line("runtime-yard-route", "idle"));
        let routed_output = get_json(&app, &project_output_uri).await;
        assert_eq!(
            routed_output["status_report"]["command_id"],
            "runtime-yard-route"
        );
        assert_eq!(routed_output["status_report"]["state"], "idle");

        {
            let prompts = reporting.prompts.lock().unwrap();
            assert_eq!(prompts.len(), 4);
            assert_eq!(prompts[0].command_id, "configure-status-yard");
            assert!(
                prompts[0]
                    .text
                    .contains("Assume ownership of central Yard orchestration")
            );
            assert!(prompts[0].text.contains("one Yard/Herdr worker per lane"));
            for (request, command_id, raw) in [
                (&prompts[1], "runtime-project-prompt", project_prompt_text),
                (&prompts[3], "runtime-yard-route", route_prompt_text),
            ] {
                assert_eq!(request.command_id, command_id);
                assert!(request.text.starts_with(raw));
                assert!(request.text.contains(&format!("command \"{command_id}\"")));
                assert!(request.text.contains("There is no completed state"));
                assert!(
                    yard_domain::OrchestratorStatusReport::scan_terminal_output(&request.text)
                        .is_none()
                );
            }
            let yard_prompt = &prompts[2];
            assert_eq!(yard_prompt.command_id, "runtime-yard-prompt");
            assert!(
                yard_prompt
                    .text
                    .starts_with("Yard orchestrator workflow profile revision 1")
            );
            assert!(yard_prompt.text.contains(yard_prompt_text));
            assert!(yard_prompt.text.contains("one Yard/Herdr worker per lane"));
            assert!(!yard_prompt.text.contains("# New-current workflow"));
            assert!(yard_prompt.text.contains("command \"runtime-yard-prompt\""));
            assert!(yard_prompt.text.contains("There is no completed state"));
        }

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        for (table, command_id, expected) in [
            (
                "orchestrator_prompt_commands",
                "runtime-project-prompt",
                project_prompt_text,
            ),
            (
                "yard_orchestrator_prompt_commands",
                "runtime-yard-prompt",
                yard_prompt_text,
            ),
            (
                "yard_orchestrator_route_commands",
                "runtime-yard-route",
                route_prompt_text,
            ),
        ] {
            let persisted: String = connection
                .query_row(
                    &format!("SELECT prompt_text FROM {table} WHERE command_id = ?1"),
                    [command_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(persisted, expected);
        }
        drop(connection);

        let assignments = response_json(
            app.oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/{project_id}/assignments"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        let assignment = assignments["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|assignment| assignment["id"] == assignment_id)
            .unwrap();
        assert_eq!(assignment["lifecycle"], "active");
        assert_eq!(assignment["attempt"]["lifecycle"], "active");
        assert!(assignment["completion_receipt"].is_null());
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn prompts_project_orchestrator_idempotently_and_reads_recent_output() {
        let (app, temp) = test_router().await;
        let project = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(create_body("terminal-1")))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let project_id = project["id"].as_str().unwrap();
        let output_uri =
            format!("/api/v1/projects/{project_id}/orchestrator/terminal-output?lines=120");
        let output = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&output_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(output.status(), StatusCode::OK);
        let output = response_json(output).await;
        assert_eq!(output["project_id"], project["id"]);
        assert_eq!(output["worker_id"], project["orchestrator"]["id"]);
        assert_eq!(output["text"], "Focused tests are passing.");

        let prompt_uri = format!("/api/v1/projects/{project_id}/orchestrator/prompts");
        let prompt = serde_json::json!({
            "command_id": "orchestrator-prompt-command",
            "actor": "local-user",
            "expected_project_version": project["version"],
            "orchestrator_worker_id": project["orchestrator"]["id"],
            "text": "Rebalance the active workers."
        });
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&prompt_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let acknowledged = response_json(first).await;
        assert_eq!(acknowledged["runtime_status"], "working");
        assert_eq!(acknowledged["worker_id"], project["orchestrator"]["id"]);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&prompt_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(
            response_json(replay).await["submitted_at_unix_ms"],
            acknowledged["submitted_at_unix_ms"]
        );

        let mut stale = prompt;
        stale["command_id"] = serde_json::Value::String("stale-orchestrator".to_owned());
        stale["orchestrator_worker_id"] = serde_json::Value::String("worker-replaced".to_owned());
        let stale = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(prompt_uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(stale.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(stale).await["error"]["code"],
            "orchestrator_changed"
        );

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let prompt_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM orchestrator_prompt_commands",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(prompt_count, 1);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn coordination_node_http_workflow_is_idempotent_versioned_and_receipt_neutral() {
        let (app, temp) = test_router().await;
        let project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body("terminal-1")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(project_response.status(), StatusCode::CREATED);
        let project = response_json(project_response).await;
        let project_id = project["id"].as_str().unwrap();
        let create = serde_json::json!({
            "command_id": "create-http-knowledge-node",
            "actor": "local-user",
            "name": "Shared knowledge",
            "kind": "knowledge_store",
            "placement": {
                "x": 160.0,
                "y": 120.0,
                "width": 116.0,
                "height": 116.0
            },
            "attached_project_ids": [project_id]
        });
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/coordination-nodes")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);
        let created = response_json(create_response).await;
        let node_id = created["node"]["id"].as_str().unwrap().to_owned();
        assert_eq!(created["node"]["kind"], "knowledge_store");
        assert!(
            created["node"]["folder_path"]
                .as_str()
                .unwrap()
                .contains(&node_id)
        );
        assert!(created["node"]["cwd"].is_null());

        let replay_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/coordination-nodes")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let replay = response_json(replay_response).await;
        assert_eq!(replay["node"]["id"], node_id);
        assert_eq!(replay["replayed"], true);

        let listed = get_json(&app, "/api/v1/coordination-nodes").await;
        assert_eq!(listed["nodes"].as_array().unwrap().len(), 1);
        let fetched = get_json(&app, &format!("/api/v1/coordination-nodes/{node_id}")).await;
        assert_eq!(fetched["id"], node_id);

        let update = serde_json::json!({
            "command_id": "update-http-knowledge-node",
            "actor": "local-user",
            "expected_version": created["node"]["version"],
            "name": "Shared architecture knowledge",
            "attached_project_ids": [project_id]
        });
        let update_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/coordination-nodes/{node_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update_response.status(), StatusCode::OK);
        let updated = response_json(update_response).await;
        assert_eq!(updated["node"]["name"], "Shared architecture knowledge");

        let move_node = serde_json::json!({
            "command_id": "move-http-knowledge-node",
            "actor": "local-user",
            "expected_version": updated["node"]["placement"]["version"],
            "placement": {
                "x": 420.0,
                "y": 180.0,
                "width": 360.0,
                "height": 220.0
            }
        });
        let move_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/coordination-nodes/{node_id}/placement"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(move_node.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(move_response.status(), StatusCode::OK);
        let moved = response_json(move_response).await;
        assert_eq!(moved["node"]["placement"]["geometry"]["x"], 420.0);
        assert_eq!(moved["node"]["version"], updated["node"]["version"]);

        let snapshot_request = serde_json::json!({
            "command_id": "snapshot-http-knowledge-node",
            "actor": "local-user",
            "expected_node_version": updated["node"]["version"]
        });
        let snapshot_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/coordination-nodes/{node_id}/snapshots"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(snapshot_request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot_response.status(), StatusCode::OK);
        let snapshot = response_json(snapshot_response).await;
        assert_eq!(snapshot["progress"]["completed"], 0);
        assert_eq!(snapshot["progress"]["total"], 1);
        assert_eq!(snapshot["projects"][0]["delivery_status"], "submitted");
        let snapshot_id = snapshot["id"].as_str().unwrap();
        let fetched_snapshot = get_json(
            &app,
            &format!("/api/v1/coordination-nodes/{node_id}/snapshots/{snapshot_id}"),
        )
        .await;
        assert_eq!(fetched_snapshot["id"], snapshot_id);

        let invalid_id = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/coordination-nodes/NOT-A-UUID")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_id.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let receipt_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM completion_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(receipt_count, 0);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn coordination_node_terminal_upgrades_and_revokes_on_node_change() {
        use yard_store::YardStore;

        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let node_id = uuid::Uuid::now_v7().to_string();
        let cwd = temp.path().join("coordination").join(&node_id);
        std::fs::create_dir_all(&cwd).unwrap();
        let node = store
            .create_coordination_node(
                &node_id,
                Some(cwd.to_string_lossy().into_owned()),
                None,
                CreateCoordinationNode {
                    command_id: "create-terminal-node".to_owned(),
                    actor: "local-user".to_owned(),
                    name: "Terminal coordination".to_owned(),
                    kind: CoordinationNodeKind::Workstream,
                    placement: CanvasPlacement {
                        x: 80.0,
                        y: 70.0,
                        width: 322.0,
                        height: 240.0,
                    },
                    attached_project_ids: Vec::new(),
                },
            )
            .await
            .unwrap()
            .node;
        let source = Arc::new(FakeInventory);
        store
            .reconcile_runtime_inventory(source.inventory("yard-coordination").await.unwrap())
            .await
            .unwrap();
        let candidate = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .into_iter()
            .find(|candidate| {
                candidate
                    .worker
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-1")
            })
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: WorkerProfileSpec {
                    name: "Terminal coordinator".to_owned(),
                    runtime_adapter: "herdr".to_owned(),
                    provider: "codex".to_owned(),
                    model: Some("gpt-5.4".to_owned()),
                    default_role: "coordinator".to_owned(),
                    instructions_ref: None,
                    tools: Vec::new(),
                    skills: Vec::new(),
                    mcp_servers: Vec::new(),
                    sandbox_policy: "runtime_default".to_owned(),
                    worktree_policy: "project_workspace".to_owned(),
                    permission_policy: "runtime_default".to_owned(),
                    completion_contract: "manual_receipt".to_owned(),
                },
            })
            .await
            .unwrap();
        let worker = store
            .pin_worker_profile(&candidate.worker.id, &profile.id, profile.version)
            .await
            .unwrap();
        let provisioned = store
            .configure_coordination_node(
                &node_id,
                ProvisionCoordinationNode {
                    command_id: "provision-terminal-node".to_owned(),
                    actor: "local-user".to_owned(),
                    profile_id: profile.id,
                    expected_profile_version: profile.version,
                    expected_node_version: node.version,
                },
                &worker.id,
                worker.version,
            )
            .await
            .unwrap()
            .node;
        let runtime = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            source,
            runtime.clone(),
            runtime.clone(),
            runtime,
            store,
            artifacts,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let terminal_url =
            format!("ws://{address}/api/v1/coordination-nodes/{node_id}/terminal?cols=80&rows=24");
        let mut terminal_request = terminal_url.into_client_request().unwrap();
        terminal_request.headers_mut().insert(
            header::ORIGIN,
            axum::http::HeaderValue::from_static("http://127.0.0.1:5173"),
        );
        let (mut socket, response) = connect_async(terminal_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            TungsteniteMessage::Text(_)
        ));

        let update = serde_json::json!({
            "command_id": "update-terminal-node",
            "actor": "local-user",
            "expected_version": provisioned.version.to_string(),
            "name": "Changed terminal coordination",
            "attached_project_ids": []
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/coordination-nodes/{node_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("coordination node terminal lease was not revoked")
            .unwrap()
            .unwrap();
        let TungsteniteMessage::Text(closed) = closed else {
            panic!("expected terminal.closed");
        };
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["type"], "terminal.closed");
        assert_eq!(closed["reason"], "coordination_node_changed");
        server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn manages_and_runs_a_paused_superintendent_automation() {
        let (app, _temp) = test_router().await;
        let create = serde_json::json!({
            "command_id": "create-daily-briefing",
            "actor": "local-user",
            "name": "Daily briefing",
            "scope": { "kind": "yard_orchestrator" },
            "placement": {
                "x": 220.0,
                "y": 180.0,
                "width": 168.0,
                "height": 58.0
            },
            "schedule": {
                "hour": 9,
                "minute": 0,
                "timezone": "America/Los_Angeles"
            },
            "selected_project_ids": [],
            "prompt_template": "Summarize active work and owner actions."
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/automations")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL].to_str().unwrap(),
            "no-store"
        );
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let created = response_json(response).await;
        let automation_id = created["automation"]["id"].as_str().unwrap();
        assert_eq!(location, format!("/api/v1/automations/{automation_id}"));
        assert_eq!(created["automation"]["state"], "active");
        assert!(created["automation"]["next_run_at_unix_ms"].is_string());

        let pause = serde_json::json!({
            "command_id": "pause-daily-briefing",
            "actor": "local-user",
            "expected_version": created["automation"]["version"],
            "paused": true
        });
        let paused = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::PUT)
                        .uri(format!("/api/v1/automations/{automation_id}/state"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(pause.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(paused["automation"]["state"], "paused");
        assert!(paused["automation"]["next_run_at_unix_ms"].is_null());

        let run = serde_json::json!({
            "command_id": "run-daily-briefing",
            "actor": "local-user",
            "expected_version": paused["automation"]["version"]
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/automations/{automation_id}/runs"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(run.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let run = response_json(response).await;
        assert_eq!(run["run"]["status"], "failed");
        assert!(
            run["run"]["error_message"]
                .as_str()
                .unwrap()
                .contains("Superintendent")
        );

        let runs = get_json(
            &app,
            &format!("/api/v1/automations/{automation_id}/runs?limit=20"),
        )
        .await;
        assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
        assert_eq!(runs["runs"][0]["id"], run["run"]["id"]);
    }

    #[tokio::test]
    async fn token_spend_settings_api_defaults_off_and_updates_independently() {
        let (app, _temp) = test_router().await;
        let defaults = get_json(&app, "/api/v1/token-spend-settings").await;
        assert_eq!(
            defaults["superintendent_auto_requests_project_summaries"],
            false
        );
        assert_eq!(
            defaults["project_orchestrators_auto_request_worker_summaries"],
            false
        );
        assert_eq!(defaults["scheduled_automatic_summaries"], false);

        let update = serde_json::json!({
            "actor": "local-user",
            "expected_version": defaults["version"],
            "superintendent_auto_requests_project_summaries": true,
            "project_orchestrators_auto_request_worker_summaries": false,
            "scheduled_automatic_summaries": false
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/token-spend-settings")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL].to_str().unwrap(),
            "no-store"
        );
        let updated = response_json(response).await;
        assert_eq!(
            updated["superintendent_auto_requests_project_summaries"],
            true
        );
        assert_eq!(
            updated["project_orchestrators_auto_request_worker_summaries"],
            false
        );
        assert_eq!(updated["scheduled_automatic_summaries"], false);

        let conflict = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/token-spend-settings")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let conflict = response_json(conflict).await;
        assert_eq!(conflict["error"]["code"], "token_spend_settings_conflict");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn scheduler_enforces_independent_automatic_layers_and_preserves_manual_dispatch() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let source = Arc::new(FakeInventory);
        let control = Arc::new(FakeRuntime);
        let reporting = Arc::new(ReportingRuntime::default());
        let terminal = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            source.clone(),
            control.clone(),
            reporting.clone(),
            terminal,
            store.clone(),
            artifacts,
        );
        let (project_id, assignment_id) = create_active_assignment(&app).await;

        let inventory_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtimes/herdr/sessions/default/inventory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory_response.status(), StatusCode::OK);
        let candidates = get_json(&app, "/api/v1/workers").await;
        let yard_candidate = candidates["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| {
                candidate["worker"]["runtime"]["terminal_id"] == "terminal-yard-allocationcommand1"
            })
            .unwrap();
        let initial_yard = get_json(&app, "/api/v1/yard/orchestrator").await;
        let configure = serde_json::json!({
            "command_id": "configure-automatic-summary-yard",
            "actor": "local-user",
            "worker_id": yard_candidate["worker"]["id"],
            "expected_worker_version": yard_candidate["worker"]["version"],
            "expected_orchestrator_version": initial_yard["version"]
        });
        let configured_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/yard/orchestrator")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(configure.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(configured_response.status(), StatusCode::OK);
        {
            let mut prompts = reporting.prompts.lock().unwrap();
            assert_eq!(prompts.len(), 1);
            assert_eq!(prompts[0].command_id, "configure-automatic-summary-yard");
            assert!(
                prompts[0]
                    .text
                    .contains("Assume ownership of central Yard orchestration")
            );
            assert!(prompts[0].text.contains("one Yard/Herdr worker per lane"));
            prompts.clear();
        }

        let reconciliation = ReconciliationService::new(source.clone(), store.clone());
        let coordination_nodes = CoordinationNodeService::new(
            source.clone(),
            control,
            reporting.clone(),
            store.clone(),
            reconciliation,
            temp.path().join("coordination"),
            temp.path().join("knowledge"),
        );
        let interventions = InterventionService::new(source, reporting.clone(), store.clone());
        let scheduler = AutomationService::new(store.clone(), interventions, coordination_nodes);

        let automation_id = uuid::Uuid::now_v7().to_string();
        let automation = store
            .create_automation(
                CreateAutomation {
                    command_id: "create-due-summary-automation".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    name: "Due project summary".to_owned(),
                    scope: AutomationScope::ProjectOrchestrator {
                        project_id: project_id.clone(),
                    },
                    placement: CanvasPlacement {
                        x: 220.0,
                        y: 180.0,
                        width: 168.0,
                        height: 58.0,
                    },
                    schedule: DailySchedule {
                        hour: 9,
                        minute: 0,
                        timezone: "UTC".to_owned(),
                    },
                    selected_project_ids: vec![project_id.clone()],
                    prompt_template: "Summarize current project status.".to_owned(),
                },
                1,
            )
            .await
            .unwrap()
            .automation;

        scheduler.run_due_once().await.unwrap();
        assert!(reporting.prompts.lock().unwrap().is_empty());
        let count_rows = |table: &str| {
            let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        };
        assert_eq!(count_rows("yard_orchestrator_route_commands"), 0);
        assert_eq!(count_rows("assignment_prompt_commands"), 0);
        assert_eq!(count_rows("automation_runs"), 0);

        let assignment = store
            .list_project_assignments(&project_id)
            .await
            .unwrap()
            .assignments
            .into_iter()
            .find(|assignment| assignment.id == assignment_id)
            .unwrap();
        let manual_prompt = serde_json::json!({
            "command_id": "manual-prompt-with-automatic-settings-off",
            "actor": "local-user",
            "attempt_id": assignment.attempt.id,
            "expected_assignment_version": assignment.version.to_string(),
            "expected_attempt_version": assignment.attempt.version.to_string(),
            "text": "Manual status request."
        });
        let manual_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/projects/{project_id}/assignments/{assignment_id}/prompts"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(manual_prompt.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(manual_response.status(), StatusCode::OK);
        assert_eq!(reporting.prompts.lock().unwrap().len(), 1);

        let manual_run = scheduler
            .run_now(RunAutomationNow {
                command_id: "manual-run-with-schedule-off".to_owned(),
                actor: "local-user".to_owned(),
                automation_id: automation_id.clone(),
                expected_version: automation.version,
            })
            .await
            .unwrap();
        assert_eq!(
            manual_run.run.trigger,
            yard_domain::AutomationRunTrigger::Manual
        );
        assert_eq!(
            manual_run.run.status,
            yard_domain::AutomationRunStatus::Submitted
        );
        assert_eq!(reporting.prompts.lock().unwrap().len(), 2);

        let defaults = store.get_token_spend_settings().await.unwrap();
        let superintendent_only = store
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: defaults.version,
                superintendent_auto_requests_project_summaries: true,
                project_orchestrators_auto_request_worker_summaries: false,
                scheduled_automatic_summaries: false,
            })
            .await
            .unwrap();
        scheduler.run_due_once().await.unwrap();
        assert_eq!(reporting.prompts.lock().unwrap().len(), 3);
        assert_eq!(count_rows("yard_orchestrator_route_commands"), 1);
        assert_eq!(count_rows("assignment_prompt_commands"), 1);
        assert_eq!(count_rows("automation_runs"), 1);

        let workers_only = store
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: superintendent_only.version,
                superintendent_auto_requests_project_summaries: false,
                project_orchestrators_auto_request_worker_summaries: true,
                scheduled_automatic_summaries: false,
            })
            .await
            .unwrap();
        scheduler.run_due_once().await.unwrap();
        assert_eq!(reporting.prompts.lock().unwrap().len(), 4);
        assert_eq!(count_rows("yard_orchestrator_route_commands"), 1);
        assert_eq!(count_rows("assignment_prompt_commands"), 2);
        assert_eq!(count_rows("automation_runs"), 1);

        store
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: workers_only.version,
                superintendent_auto_requests_project_summaries: false,
                project_orchestrators_auto_request_worker_summaries: false,
                scheduled_automatic_summaries: true,
            })
            .await
            .unwrap();
        scheduler.run_due_once().await.unwrap();
        assert_eq!(reporting.prompts.lock().unwrap().len(), 5);
        assert_eq!(count_rows("yard_orchestrator_route_commands"), 1);
        assert_eq!(count_rows("assignment_prompt_commands"), 2);
        assert_eq!(count_rows("automation_runs"), 2);
        let runs = store
            .list_automation_runs(&automation_id, 10)
            .await
            .unwrap();
        assert_eq!(
            runs.runs
                .iter()
                .filter(|run| { run.trigger == yard_domain::AutomationRunTrigger::Scheduled })
                .count(),
            1
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn disabling_automatic_spend_waits_for_in_flight_dispatch_and_blocks_later_ticks() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let source = Arc::new(FakeInventory);
        let control = Arc::new(FakeRuntime);
        let (reporting, prompt_entered, release_prompt) = ReportingRuntime::blocked();
        let reporting = Arc::new(reporting);
        let terminal = Arc::new(FakeRuntime);
        let artifacts = ArtifactService::new(temp.path().join("artifacts"), store.clone());
        let app = router(
            source.clone(),
            control.clone(),
            reporting.clone(),
            terminal,
            store.clone(),
            artifacts,
        );
        let (project_id, _) = create_active_assignment(&app).await;
        let reconciliation = ReconciliationService::new(source.clone(), store.clone());
        let coordination_nodes = CoordinationNodeService::new(
            source.clone(),
            control,
            reporting.clone(),
            store.clone(),
            reconciliation,
            temp.path().join("coordination"),
            temp.path().join("knowledge"),
        );
        let interventions = InterventionService::new(source, reporting.clone(), store.clone());
        let scheduler = AutomationService::new(store.clone(), interventions, coordination_nodes);

        let automation_id = uuid::Uuid::now_v7().to_string();
        store
            .create_automation(
                CreateAutomation {
                    command_id: "create-blocked-scheduled-summary".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    name: "Blocked scheduled summary".to_owned(),
                    scope: AutomationScope::ProjectOrchestrator {
                        project_id: project_id.clone(),
                    },
                    placement: CanvasPlacement {
                        x: 220.0,
                        y: 180.0,
                        width: 168.0,
                        height: 58.0,
                    },
                    schedule: DailySchedule {
                        hour: 9,
                        minute: 0,
                        timezone: "UTC".to_owned(),
                    },
                    selected_project_ids: vec![project_id.clone()],
                    prompt_template: "Summarize current project status.".to_owned(),
                },
                1,
            )
            .await
            .unwrap();
        let defaults = scheduler.token_spend_settings().await.unwrap();
        let enabled = scheduler
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: defaults.version,
                superintendent_auto_requests_project_summaries: false,
                project_orchestrators_auto_request_worker_summaries: false,
                scheduled_automatic_summaries: true,
            })
            .await
            .unwrap();

        let running_scheduler = scheduler.clone();
        let tick = tokio::spawn(async move { running_scheduler.run_due_once().await });
        prompt_entered.notified().await;

        let disabling_scheduler = scheduler.clone();
        let mut disable = tokio::spawn(async move {
            disabling_scheduler
                .update_token_spend_settings(UpdateTokenSpendSettings {
                    actor: "local-user".to_owned(),
                    expected_version: enabled.version,
                    superintendent_auto_requests_project_summaries: false,
                    project_orchestrators_auto_request_worker_summaries: false,
                    scheduled_automatic_summaries: false,
                })
                .await
        });
        assert!(
            timeout(Duration::from_millis(50), &mut disable)
                .await
                .is_err()
        );

        release_prompt.notify_one();
        tick.await.unwrap().unwrap();
        let disabled = disable.await.unwrap().unwrap();
        assert!(!disabled.scheduled_automatic_summaries);
        assert_eq!(reporting.prompts.lock().unwrap().len(), 1);

        let later_automation_id = uuid::Uuid::now_v7().to_string();
        store
            .create_automation(
                CreateAutomation {
                    command_id: "create-later-disabled-summary".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: later_automation_id.clone(),
                    name: "Later disabled summary".to_owned(),
                    scope: AutomationScope::ProjectOrchestrator {
                        project_id: project_id.clone(),
                    },
                    placement: CanvasPlacement {
                        x: 420.0,
                        y: 180.0,
                        width: 168.0,
                        height: 58.0,
                    },
                    schedule: DailySchedule {
                        hour: 9,
                        minute: 0,
                        timezone: "UTC".to_owned(),
                    },
                    selected_project_ids: vec![project_id],
                    prompt_template: "This must remain disabled.".to_owned(),
                },
                1,
            )
            .await
            .unwrap();
        scheduler.run_due_once().await.unwrap();
        assert_eq!(reporting.prompts.lock().unwrap().len(), 1);
        assert!(
            store
                .list_automation_runs(&later_automation_id, 10)
                .await
                .unwrap()
                .runs
                .is_empty()
        );
    }
}

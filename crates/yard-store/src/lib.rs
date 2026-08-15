use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs::{File, OpenOptions},
    num::TryFromIntError,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

use async_trait::async_trait;
use fs2::FileExt;
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Row, Transaction, TransactionBehavior, params,
    types::Type,
};
use thiserror::Error;
use tokio::task;
use uuid::Uuid;
use yard_domain::{
    AllocationContext, AllocationMode, Artifact, ArtifactKind, ArtifactRegistration,
    ArtifactSource, Assignment, AssignmentAttempt, AssignmentLifecycle, Assignments,
    AttemptLifecycle, Automation, AutomationCommandResult, AutomationRun,
    AutomationRunCommandResult, AutomationRuns, Automations, CanvasPlacement, CompletionOutcome,
    CompletionReceipt, ConfigureYardOrchestrator, ConfiguredYardOrchestrator,
    ConfirmProfileAllocation, ConfirmWorkerAllocation, ConfirmWorkerHandoff, ConfirmedAllocation,
    ConfirmedProjectCreation, ConfirmedWorkerHandoff, CoordinationCommandStatus, CoordinationNode,
    CoordinationNodeCommandResult, CoordinationNodePromptAcknowledgement, CoordinationNodeRoute,
    CoordinationNodeRoutes, CoordinationNodes, CoordinationSnapshot, CoordinationSnapshots,
    CreateAutomation, CreateCoordinationNode, CreateProject, CreateProjectFromProfile,
    CreateProjectRelationship, CreateWorkerProfile, CreateWorkspaceProjectFromProfile,
    CreatedProjectRelationship, DeleteProjectRelationship, DeletedProjectRelationship,
    EndWorkerSession, EndedWorkerSession, HandoffTargetRole, IsolationPolicy, ObservedStatus,
    ObservedWorker, OrchestratorPromptAcknowledgement, PaneObservation, Project, ProjectPlacement,
    ProjectRelationship, ProjectRelationshipKind, ProjectRelationships, ProjectRuntimeBinding,
    Projects, ProviderSessionRef, ProvisionCoordinationNode, RecordCompletionReceipt,
    RecordedCompletionReceipt, RequestCoordinationSnapshot, RunAutomationNow, RuntimeInventory,
    RuntimeObservationState, RuntimeProcessState, RuntimeReconciliation, SendAssignmentPrompt,
    SendCoordinationNodePrompt, SendCoordinationNodeRoute, SendOrchestratorPrompt,
    SendYardOrchestratorPrompt, SendYardOrchestratorRoute, SetAutomationPaused, UpdateAutomation,
    UpdateAutomationPlacement, UpdateCoordinationNode, UpdateCoordinationNodePlacement,
    UpdateProjectPlacement, UpdateWorkerProfile, Worker, WorkerAllocation, WorkerAvailability,
    WorkerCandidate, WorkerCandidates, WorkerDesiredState, WorkerProfile, WorkerProfileSpec,
    WorkerProfiles, WorkerRuntimeBinding, YardOrchestrator, YardOrchestratorPromptAcknowledgement,
    YardOrchestratorRoute, YardOrchestratorRoutes,
};

mod automation_store;
mod coordination_node_store;

const SCHEMA_VERSION: i64 = 18;
const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_projects.sql");
const PROFILE_ASSIGNMENT_MIGRATION: &str =
    include_str!("../migrations/0002_profiles_assignments.sql");
const COMPLETION_RECEIPT_MIGRATION: &str =
    include_str!("../migrations/0003_completion_receipts.sql");
const ASSIGNMENT_PROMPT_MIGRATION: &str = include_str!("../migrations/0004_assignment_prompts.sql");
const RUNTIME_RECONCILIATION_MIGRATION: &str =
    include_str!("../migrations/0005_runtime_reconciliation.sql");
const RECONCILIATION_WATERMARK_MIGRATION: &str =
    include_str!("../migrations/0006_reconciliation_watermarks.sql");
const WORKER_ALLOCATION_MIGRATION: &str = include_str!("../migrations/0007_worker_allocations.sql");
const PROFILE_PROJECT_CREATION_MIGRATION: &str =
    include_str!("../migrations/0008_profile_project_creation.sql");
const WORKER_HANDOFF_MIGRATION: &str = include_str!("../migrations/0009_worker_handoffs.sql");
const RUNTIME_CLEANUP_MIGRATION: &str = include_str!("../migrations/0010_runtime_cleanup.sql");
const ARTIFACT_MIGRATION: &str = include_str!("../migrations/0011_artifacts.sql");
const WORKSPACE_PROJECT_CREATION_MIGRATION: &str =
    include_str!("../migrations/0012_workspace_project_creation.sql");
const ORCHESTRATOR_PROMPT_MIGRATION: &str =
    include_str!("../migrations/0013_orchestrator_prompts.sql");
const WORKER_SESSION_END_MIGRATION: &str =
    include_str!("../migrations/0014_worker_session_end.sql");
const WORKER_SESSION_END_ACK_REPAIR_MIGRATION: &str =
    include_str!("../migrations/0014_worker_session_end_ack_repair.sql");
const YARD_ORCHESTRATOR_MIGRATION: &str = include_str!("../migrations/0015_yard_orchestrator.sql");
const COORDINATION_MIGRATION: &str = include_str!("../migrations/0016_coordination.sql");
const COORDINATION_NODES_MIGRATION: &str =
    include_str!("../migrations/0017_coordination_nodes.sql");
const AUTOMATIONS_MIGRATION: &str = include_str!("../migrations/0018_automations.sql");
const BLANK_WORKER_PROFILE_ID: &str = "yard:managed-blank-profile";
const BLANK_WORKER_PROFILE_NAME: &str = "Blank profile";
const MAX_ROUTE_LIST_LIMIT: usize = 500;

#[async_trait]
pub trait YardStore: Send + Sync {
    async fn list_automations(&self) -> Result<Automations, ProjectStoreError>;
    async fn get_automation(&self, automation_id: &str) -> Result<Automation, ProjectStoreError>;
    async fn create_automation(
        &self,
        command: CreateAutomation,
        next_run_at_unix_ms: u64,
    ) -> Result<AutomationCommandResult, ProjectStoreError>;
    async fn update_automation(
        &self,
        command: UpdateAutomation,
        next_run_at_unix_ms: Option<u64>,
    ) -> Result<AutomationCommandResult, ProjectStoreError>;
    async fn update_automation_placement(
        &self,
        command: UpdateAutomationPlacement,
    ) -> Result<AutomationCommandResult, ProjectStoreError>;
    async fn set_automation_paused(
        &self,
        command: SetAutomationPaused,
        next_run_at_unix_ms: Option<u64>,
    ) -> Result<AutomationCommandResult, ProjectStoreError>;
    async fn create_manual_automation_run(
        &self,
        command: RunAutomationNow,
        run_id: &str,
        dispatch_command_id: &str,
    ) -> Result<AutomationRunCommandResult, ProjectStoreError>;
    async fn list_due_automations(
        &self,
        now_unix_ms: u64,
        limit: usize,
    ) -> Result<Automations, ProjectStoreError>;
    #[allow(clippy::too_many_arguments)]
    async fn claim_scheduled_automation_run(
        &self,
        automation_id: &str,
        expected_due_unix_ms: u64,
        next_due_unix_ms: u64,
        run_id: &str,
        dispatch_command_id: &str,
        requested_by: &str,
    ) -> Result<AutomationRun, ProjectStoreError>;
    async fn list_pending_automation_runs(
        &self,
        limit: usize,
    ) -> Result<AutomationRuns, ProjectStoreError>;
    async fn mark_automation_run_submitted(
        &self,
        run_id: &str,
        runtime_status: &str,
        submitted_at_unix_ms: u64,
    ) -> Result<AutomationRun, ProjectStoreError>;
    async fn mark_automation_run_failed(
        &self,
        run_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<AutomationRun, ProjectStoreError>;
    async fn list_automation_runs(
        &self,
        automation_id: &str,
        limit: usize,
    ) -> Result<AutomationRuns, ProjectStoreError>;
    async fn get_automation_run(
        &self,
        automation_id: &str,
        run_id: &str,
    ) -> Result<AutomationRun, ProjectStoreError>;
    async fn list_coordination_nodes(&self) -> Result<CoordinationNodes, ProjectStoreError>;
    async fn get_coordination_node(
        &self,
        node_id: &str,
    ) -> Result<CoordinationNode, ProjectStoreError>;
    async fn create_coordination_node(
        &self,
        node_id: &str,
        workstream_cwd: Option<String>,
        knowledge_path: Option<String>,
        command: CreateCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError>;
    async fn update_coordination_node(
        &self,
        node_id: &str,
        command: UpdateCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError>;
    async fn update_coordination_node_placement(
        &self,
        node_id: &str,
        command: UpdateCoordinationNodePlacement,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError>;
    async fn configure_coordination_node(
        &self,
        node_id: &str,
        command: ProvisionCoordinationNode,
        worker_id: &str,
        expected_worker_version: u64,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError>;
    async fn begin_coordination_node_prompt(
        &self,
        node_id: &str,
        command: SendCoordinationNodePrompt,
    ) -> Result<BeginCoordinationNodePrompt, ProjectStoreError>;
    async fn succeed_coordination_node_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<CoordinationNodePromptAcknowledgement, ProjectStoreError>;
    async fn fail_coordination_node_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn latest_delivered_coordination_node_command_id(
        &self,
        node_id: &str,
    ) -> Result<Option<String>, ProjectStoreError>;
    async fn list_coordination_node_routes(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<CoordinationNodeRoutes, ProjectStoreError>;
    async fn begin_coordination_node_route(
        &self,
        node_id: &str,
        command: SendCoordinationNodeRoute,
    ) -> Result<BeginCoordinationNodeRoute, ProjectStoreError>;
    async fn succeed_coordination_node_route(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<CoordinationNodeRoute, ProjectStoreError>;
    async fn fail_coordination_node_route(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn create_coordination_snapshot(
        &self,
        node_id: &str,
        snapshot_id: &str,
        folder_path: String,
        project_folders: Vec<SnapshotProjectFolder>,
        command: RequestCoordinationSnapshot,
    ) -> Result<CoordinationSnapshot, ProjectStoreError>;
    async fn list_coordination_snapshots(
        &self,
        node_id: &str,
    ) -> Result<CoordinationSnapshots, ProjectStoreError>;
    async fn get_coordination_snapshot(
        &self,
        node_id: &str,
        snapshot_id: &str,
    ) -> Result<CoordinationSnapshot, ProjectStoreError>;
    async fn record_snapshot_project_delivery(
        &self,
        snapshot_id: &str,
        project_id: &str,
        result: SnapshotDeliveryResult,
    ) -> Result<(), ProjectStoreError>;
    async fn record_snapshot_project_collected(
        &self,
        snapshot_id: &str,
        project_id: &str,
    ) -> Result<(), ProjectStoreError>;
    async fn get_yard_orchestrator(&self) -> Result<YardOrchestrator, ProjectStoreError>;
    async fn configure_yard_orchestrator(
        &self,
        command: ConfigureYardOrchestrator,
    ) -> Result<ConfiguredYardOrchestrator, ProjectStoreError>;
    async fn list_projects(&self) -> Result<Projects, ProjectStoreError>;
    async fn list_project_relationships(&self) -> Result<ProjectRelationships, ProjectStoreError>;
    async fn create_project_relationship(
        &self,
        command: CreateProjectRelationship,
    ) -> Result<CreatedProjectRelationship, ProjectStoreError>;
    async fn delete_project_relationship(
        &self,
        relationship_id: &str,
        command: DeleteProjectRelationship,
    ) -> Result<DeletedProjectRelationship, ProjectStoreError>;
    async fn get_project(&self, project_id: &str) -> Result<Project, ProjectStoreError>;
    async fn create_project(
        &self,
        project: CreateProject,
        orchestrator_runtime: WorkerRuntimeBinding,
    ) -> Result<Project, ProjectStoreError>;
    async fn begin_profile_project_creation(
        &self,
        command: CreateProjectFromProfile,
    ) -> Result<BeginProfileProjectCreation, ProjectStoreError>;
    async fn finalize_profile_project_creation(
        &self,
        command_id: &str,
        orchestrator_runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedProjectCreation, ProjectStoreError>;
    async fn fail_profile_project_creation(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn begin_workspace_project_creation(
        &self,
        command: CreateWorkspaceProjectFromProfile,
    ) -> Result<BeginWorkspaceProjectCreation, ProjectStoreError>;
    async fn finalize_workspace_project_creation(
        &self,
        command_id: &str,
        orchestrator_runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedProjectCreation, ProjectStoreError>;
    async fn fail_workspace_project_creation(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn update_project_placement(
        &self,
        project_id: &str,
        update: UpdateProjectPlacement,
    ) -> Result<Project, ProjectStoreError>;
    async fn list_worker_profiles(&self) -> Result<WorkerProfiles, ProjectStoreError>;
    async fn get_worker_profile(
        &self,
        profile_id: &str,
    ) -> Result<WorkerProfile, ProjectStoreError>;
    async fn pin_worker_profile(
        &self,
        worker_id: &str,
        profile_id: &str,
        expected_profile_version: u64,
    ) -> Result<Worker, ProjectStoreError>;
    async fn create_worker_profile(
        &self,
        profile: CreateWorkerProfile,
    ) -> Result<WorkerProfile, ProjectStoreError>;
    async fn update_worker_profile(
        &self,
        profile_id: &str,
        update: UpdateWorkerProfile,
    ) -> Result<WorkerProfile, ProjectStoreError>;
    async fn list_worker_candidates(&self) -> Result<WorkerCandidates, ProjectStoreError>;
    async fn begin_profile_allocation(
        &self,
        project_id: &str,
        command: ConfirmProfileAllocation,
    ) -> Result<BeginProfileAllocation, ProjectStoreError>;
    async fn persist_runtime_allocation(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedAllocation, ProjectStoreError>;
    async fn activate_profile_allocation(
        &self,
        command_id: &str,
    ) -> Result<ConfirmedAllocation, ProjectStoreError>;
    async fn fail_profile_allocation(
        &self,
        command_id: &str,
        message: &str,
    ) -> Result<(), ProjectStoreError>;
    async fn begin_worker_allocation(
        &self,
        project_id: &str,
        command: ConfirmWorkerAllocation,
    ) -> Result<BeginWorkerAllocation, ProjectStoreError>;
    async fn replace_worker_runtime(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedAllocation, ProjectStoreError>;
    async fn activate_worker_allocation(
        &self,
        command_id: &str,
    ) -> Result<ConfirmedAllocation, ProjectStoreError>;
    async fn fail_worker_allocation(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn begin_worker_handoff(
        &self,
        source_project_id: &str,
        source_assignment_id: &str,
        command: ConfirmWorkerHandoff,
    ) -> Result<BeginWorkerHandoff, ProjectStoreError>;
    async fn claim_worker_handoff_runtime(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<(), ProjectStoreError>;
    async fn finalize_worker_handoff(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedWorkerHandoff, ProjectStoreError>;
    async fn fail_worker_handoff(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn end_worker_session(
        &self,
        worker_id: &str,
        command: EndWorkerSession,
    ) -> Result<EndedWorkerSession, ProjectStoreError>;
    async fn list_pending_runtime_cleanups(
        &self,
        command_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PendingRuntimeCleanup>, ProjectStoreError>;
    async fn succeed_runtime_cleanup(&self, cleanup_id: &str) -> Result<(), ProjectStoreError>;
    async fn fail_runtime_cleanup(
        &self,
        cleanup_id: &str,
        message: &str,
    ) -> Result<(), ProjectStoreError>;
    async fn list_project_assignments(
        &self,
        project_id: &str,
    ) -> Result<Assignments, ProjectStoreError>;
    async fn register_artifact(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
        registration: ArtifactRegistration,
    ) -> Result<Artifact, ProjectStoreError>;
    async fn get_artifact(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
    ) -> Result<Artifact, ProjectStoreError>;
    async fn record_completion_receipt(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: RecordCompletionReceipt,
    ) -> Result<RecordedCompletionReceipt, ProjectStoreError>;
    async fn begin_assignment_prompt(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: SendAssignmentPrompt,
    ) -> Result<BeginAssignmentPrompt, ProjectStoreError>;
    async fn succeed_assignment_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<yard_domain::PromptAcknowledgement, ProjectStoreError>;
    async fn fail_assignment_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn begin_orchestrator_prompt(
        &self,
        project_id: &str,
        command: SendOrchestratorPrompt,
    ) -> Result<BeginOrchestratorPrompt, ProjectStoreError>;
    async fn succeed_orchestrator_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<OrchestratorPromptAcknowledgement, ProjectStoreError>;
    async fn fail_orchestrator_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn latest_delivered_project_orchestrator_command_id(
        &self,
        project_id: &str,
    ) -> Result<Option<String>, ProjectStoreError>;
    async fn begin_yard_orchestrator_prompt(
        &self,
        command: SendYardOrchestratorPrompt,
    ) -> Result<BeginYardOrchestratorPrompt, ProjectStoreError>;
    async fn succeed_yard_orchestrator_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<YardOrchestratorPromptAcknowledgement, ProjectStoreError>;
    async fn fail_yard_orchestrator_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn latest_delivered_yard_orchestrator_command_id(
        &self,
    ) -> Result<Option<String>, ProjectStoreError>;
    async fn list_yard_orchestrator_routes(
        &self,
        limit: usize,
    ) -> Result<YardOrchestratorRoutes, ProjectStoreError>;
    async fn begin_yard_orchestrator_route(
        &self,
        command: SendYardOrchestratorRoute,
    ) -> Result<BeginYardOrchestratorRoute, ProjectStoreError>;
    async fn succeed_yard_orchestrator_route(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<YardOrchestratorRoute, ProjectStoreError>;
    async fn fail_yard_orchestrator_route(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError>;
    async fn reconcile_runtime_inventory(
        &self,
        inventory: RuntimeInventory,
    ) -> Result<RuntimeReconciliation, ProjectStoreError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginProfileAllocation {
    Started(Box<AllocationContext>),
    Replayed(Box<ConfirmedAllocation>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginProfileProjectCreation {
    Started(Box<ProfileProjectCreationContext>),
    Replayed(Box<ConfirmedProjectCreation>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileProjectCreationContext {
    pub command: CreateProjectFromProfile,
    pub profile: WorkerProfile,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginWorkspaceProjectCreation {
    Started(Box<WorkspaceProjectCreationContext>),
    Replayed(Box<ConfirmedProjectCreation>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceProjectCreationContext {
    pub command: CreateWorkspaceProjectFromProfile,
    pub profile: WorkerProfile,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginWorkerAllocation {
    Started(Box<WorkerAllocationContext>),
    Replayed(Box<ConfirmedAllocation>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerAllocationContext {
    pub command: ConfirmWorkerAllocation,
    pub project: Project,
    pub worker: Worker,
    pub profile: WorkerProfile,
    pub replace_runtime: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginWorkerHandoff {
    Started(Box<WorkerHandoffContext>),
    Replayed(Box<ConfirmedWorkerHandoff>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerHandoffContext {
    pub command: ConfirmWorkerHandoff,
    pub source_project: Project,
    pub target_project: Project,
    pub source_assignment: Assignment,
    pub profile: WorkerProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRuntimeCleanup {
    pub id: String,
    pub command_id: String,
    pub worker_id: String,
    pub reason: String,
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub provider_session: Option<ProviderSessionRef>,
    pub owns_tab: bool,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginAssignmentPrompt {
    Started {
        command: SendAssignmentPrompt,
        assignment: Box<Assignment>,
    },
    Replayed(yard_domain::PromptAcknowledgement),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginOrchestratorPrompt {
    Started {
        command: SendOrchestratorPrompt,
        project: Box<Project>,
    },
    Replayed(OrchestratorPromptAcknowledgement),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginYardOrchestratorPrompt {
    Started {
        command: SendYardOrchestratorPrompt,
        orchestrator: Box<YardOrchestrator>,
    },
    Replayed(YardOrchestratorPromptAcknowledgement),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginYardOrchestratorRoute {
    Started {
        command: SendYardOrchestratorRoute,
        orchestrator: Box<YardOrchestrator>,
        target_project: Box<Project>,
    },
    Replayed(YardOrchestratorRoute),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginCoordinationNodePrompt {
    Started {
        command: SendCoordinationNodePrompt,
        node: Box<CoordinationNode>,
    },
    Replayed(CoordinationNodePromptAcknowledgement),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginCoordinationNodeRoute {
    Started {
        command: SendCoordinationNodeRoute,
        node: Box<CoordinationNode>,
        target_project: Box<Project>,
    },
    Replayed(CoordinationNodeRoute),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProjectFolder {
    pub project_id: String,
    pub folder_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotDeliveryResult {
    Submitted { runtime_status: String },
    Failed { message: String, ambiguous: bool },
}

#[derive(Debug, Clone)]
pub struct SqliteProjectStore {
    _lock: Arc<File>,
    connection: Arc<std::sync::Mutex<Connection>>,
}

impl SqliteProjectStore {
    /// Open or create an exclusively owned Yard `SQLite` database, apply
    /// supported migrations, and recover prompts interrupted by restart.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when the parent directory cannot be
    /// created, another Yard process owns the database, `SQLite` cannot be
    /// opened, or the schema is newer than this version of Yard.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, ProjectStoreError> {
        let path = path.into();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let (lock, connection) = task::spawn_blocking(move || {
            let lock = acquire_database_lock(&path)?;
            let mut connection = open_connection(&path)?;
            migrate(&mut connection)?;
            recover_interrupted_prompt_commands(&mut connection)?;
            Ok::<_, ProjectStoreError>((lock, connection))
        })
        .await??;
        Ok(Self {
            _lock: Arc::new(lock),
            connection: Arc::new(std::sync::Mutex::new(connection)),
        })
    }

    async fn run<T, F>(&self, operation: F) -> Result<T, ProjectStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, ProjectStoreError> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        task::spawn_blocking(move || {
            let mut connection = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            operation(&mut connection)
        })
        .await?
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl YardStore for SqliteProjectStore {
    async fn list_automations(&self) -> Result<Automations, ProjectStoreError> {
        automation_store::list_automations(self).await
    }

    async fn get_automation(&self, automation_id: &str) -> Result<Automation, ProjectStoreError> {
        automation_store::get_automation(self, automation_id).await
    }

    async fn create_automation(
        &self,
        command: CreateAutomation,
        next_run_at_unix_ms: u64,
    ) -> Result<AutomationCommandResult, ProjectStoreError> {
        automation_store::create_automation(self, command, next_run_at_unix_ms).await
    }

    async fn update_automation(
        &self,
        command: UpdateAutomation,
        next_run_at_unix_ms: Option<u64>,
    ) -> Result<AutomationCommandResult, ProjectStoreError> {
        automation_store::update_automation(self, command, next_run_at_unix_ms).await
    }

    async fn update_automation_placement(
        &self,
        command: UpdateAutomationPlacement,
    ) -> Result<AutomationCommandResult, ProjectStoreError> {
        automation_store::update_automation_placement(self, command).await
    }

    async fn set_automation_paused(
        &self,
        command: SetAutomationPaused,
        next_run_at_unix_ms: Option<u64>,
    ) -> Result<AutomationCommandResult, ProjectStoreError> {
        automation_store::set_automation_paused(self, command, next_run_at_unix_ms).await
    }

    async fn create_manual_automation_run(
        &self,
        command: RunAutomationNow,
        run_id: &str,
        dispatch_command_id: &str,
    ) -> Result<AutomationRunCommandResult, ProjectStoreError> {
        automation_store::create_manual_automation_run(self, command, run_id, dispatch_command_id)
            .await
    }

    async fn list_due_automations(
        &self,
        now_unix_ms: u64,
        limit: usize,
    ) -> Result<Automations, ProjectStoreError> {
        automation_store::list_due_automations(self, now_unix_ms, limit).await
    }

    async fn claim_scheduled_automation_run(
        &self,
        automation_id: &str,
        expected_due_unix_ms: u64,
        next_due_unix_ms: u64,
        run_id: &str,
        dispatch_command_id: &str,
        requested_by: &str,
    ) -> Result<AutomationRun, ProjectStoreError> {
        automation_store::claim_scheduled_automation_run(
            self,
            automation_id,
            expected_due_unix_ms,
            next_due_unix_ms,
            run_id,
            dispatch_command_id,
            requested_by,
        )
        .await
    }

    async fn list_pending_automation_runs(
        &self,
        limit: usize,
    ) -> Result<AutomationRuns, ProjectStoreError> {
        automation_store::list_pending_automation_runs(self, limit).await
    }

    async fn mark_automation_run_submitted(
        &self,
        run_id: &str,
        runtime_status: &str,
        submitted_at_unix_ms: u64,
    ) -> Result<AutomationRun, ProjectStoreError> {
        automation_store::mark_automation_run_submitted(
            self,
            run_id,
            runtime_status,
            submitted_at_unix_ms,
        )
        .await
    }

    async fn mark_automation_run_failed(
        &self,
        run_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<AutomationRun, ProjectStoreError> {
        automation_store::mark_automation_run_failed(self, run_id, message, ambiguous).await
    }

    async fn list_automation_runs(
        &self,
        automation_id: &str,
        limit: usize,
    ) -> Result<AutomationRuns, ProjectStoreError> {
        automation_store::list_automation_runs(self, automation_id, limit).await
    }

    async fn get_automation_run(
        &self,
        automation_id: &str,
        run_id: &str,
    ) -> Result<AutomationRun, ProjectStoreError> {
        automation_store::get_automation_run(self, automation_id, run_id).await
    }

    async fn list_coordination_nodes(&self) -> Result<CoordinationNodes, ProjectStoreError> {
        coordination_node_store::list_nodes(self).await
    }

    async fn get_coordination_node(
        &self,
        node_id: &str,
    ) -> Result<CoordinationNode, ProjectStoreError> {
        coordination_node_store::get_node(self, node_id).await
    }

    async fn create_coordination_node(
        &self,
        node_id: &str,
        workstream_cwd: Option<String>,
        knowledge_path: Option<String>,
        command: CreateCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
        coordination_node_store::create_node(self, node_id, workstream_cwd, knowledge_path, command)
            .await
    }

    async fn update_coordination_node(
        &self,
        node_id: &str,
        command: UpdateCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
        coordination_node_store::update_node(self, node_id, command).await
    }

    async fn update_coordination_node_placement(
        &self,
        node_id: &str,
        command: UpdateCoordinationNodePlacement,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
        coordination_node_store::update_placement(self, node_id, command).await
    }

    async fn configure_coordination_node(
        &self,
        node_id: &str,
        command: ProvisionCoordinationNode,
        worker_id: &str,
        expected_worker_version: u64,
    ) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
        coordination_node_store::configure_node(
            self,
            node_id,
            command,
            worker_id,
            expected_worker_version,
        )
        .await
    }

    async fn begin_coordination_node_prompt(
        &self,
        node_id: &str,
        command: SendCoordinationNodePrompt,
    ) -> Result<BeginCoordinationNodePrompt, ProjectStoreError> {
        coordination_node_store::begin_prompt(self, node_id, command).await
    }

    async fn succeed_coordination_node_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<CoordinationNodePromptAcknowledgement, ProjectStoreError> {
        coordination_node_store::succeed_prompt(self, command_id, runtime_status).await
    }

    async fn fail_coordination_node_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        coordination_node_store::fail_prompt(self, command_id, message, ambiguous).await
    }

    async fn latest_delivered_coordination_node_command_id(
        &self,
        node_id: &str,
    ) -> Result<Option<String>, ProjectStoreError> {
        coordination_node_store::latest_delivered_command_id(self, node_id).await
    }

    async fn list_coordination_node_routes(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<CoordinationNodeRoutes, ProjectStoreError> {
        coordination_node_store::list_routes(self, node_id, limit).await
    }

    async fn begin_coordination_node_route(
        &self,
        node_id: &str,
        command: SendCoordinationNodeRoute,
    ) -> Result<BeginCoordinationNodeRoute, ProjectStoreError> {
        coordination_node_store::begin_route(self, node_id, command).await
    }

    async fn succeed_coordination_node_route(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<CoordinationNodeRoute, ProjectStoreError> {
        coordination_node_store::succeed_route(self, command_id, runtime_status).await
    }

    async fn fail_coordination_node_route(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        coordination_node_store::fail_route(self, command_id, message, ambiguous).await
    }

    async fn create_coordination_snapshot(
        &self,
        node_id: &str,
        snapshot_id: &str,
        folder_path: String,
        project_folders: Vec<SnapshotProjectFolder>,
        command: RequestCoordinationSnapshot,
    ) -> Result<CoordinationSnapshot, ProjectStoreError> {
        coordination_node_store::create_snapshot(
            self,
            node_id,
            snapshot_id,
            folder_path,
            project_folders,
            command,
        )
        .await
    }

    async fn list_coordination_snapshots(
        &self,
        node_id: &str,
    ) -> Result<CoordinationSnapshots, ProjectStoreError> {
        coordination_node_store::list_snapshots(self, node_id).await
    }

    async fn get_coordination_snapshot(
        &self,
        node_id: &str,
        snapshot_id: &str,
    ) -> Result<CoordinationSnapshot, ProjectStoreError> {
        coordination_node_store::get_snapshot(self, node_id, snapshot_id).await
    }

    async fn record_snapshot_project_delivery(
        &self,
        snapshot_id: &str,
        project_id: &str,
        result: SnapshotDeliveryResult,
    ) -> Result<(), ProjectStoreError> {
        coordination_node_store::record_snapshot_delivery(self, snapshot_id, project_id, result)
            .await
    }

    async fn record_snapshot_project_collected(
        &self,
        snapshot_id: &str,
        project_id: &str,
    ) -> Result<(), ProjectStoreError> {
        coordination_node_store::record_snapshot_collected(self, snapshot_id, project_id).await
    }

    async fn get_yard_orchestrator(&self) -> Result<YardOrchestrator, ProjectStoreError> {
        self.run(|connection| select_yard_orchestrator(connection))
            .await
    }

    async fn configure_yard_orchestrator(
        &self,
        command: ConfigureYardOrchestrator,
    ) -> Result<ConfiguredYardOrchestrator, ProjectStoreError> {
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT yocc.worker_id, yocc.expected_worker_version,
                            yocc.expected_orchestrator_version, ca.actor
                       FROM yard_orchestrator_configure_commands yocc
                       JOIN command_acknowledgements ca
                         ON ca.id = yocc.command_id
                      WHERE yocc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row_u64(row, 1)?,
                            row_u64(row, 2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((
                worker_id,
                expected_worker_version,
                expected_orchestrator_version,
                actor,
            )) = existing
            {
                let expected_worker_version_matches = expected_worker_version
                    == command.expected_worker_version
                    || select_worker_candidate(&transaction, &worker_id)?.is_some_and(
                        |candidate| candidate.worker.version == command.expected_worker_version,
                    );
                if worker_id != command.worker_id
                    || !expected_worker_version_matches
                    || expected_orchestrator_version != command.expected_orchestrator_version
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return select_configured_yard_orchestrator(
                    &transaction,
                    &command.command_id,
                    true,
                );
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let current = select_yard_orchestrator(&transaction)?;
            if current.version != command.expected_orchestrator_version {
                return Err(ProjectStoreError::YardOrchestratorVersionConflict {
                    current_version: current.version,
                });
            }
            let candidate = select_worker_candidate(&transaction, &command.worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            if candidate.worker.version != command.expected_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: candidate.worker.version,
                });
            }
            let is_current = current
                .worker
                .as_ref()
                .is_some_and(|worker| worker.id == candidate.worker.id);
            if !is_current && candidate.availability != WorkerAvailability::UnassignedLive {
                return Err(ProjectStoreError::WorkerNotAvailable {
                    availability: candidate.availability,
                });
            }
            if candidate.worker.runtime.is_none() {
                return Err(ProjectStoreError::RuntimeBindingMissing);
            }
            let prompt_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM yard_orchestrator_prompt_commands yopc
                      JOIN command_acknowledgements ca
                        ON ca.id = yopc.command_id
                     WHERE ca.status = 'pending'
                    UNION ALL
                    SELECT 1
                      FROM yard_orchestrator_route_commands yorc
                      JOIN command_acknowledgements ca
                        ON ca.id = yorc.command_id
                     WHERE ca.status = 'pending'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if prompt_pending {
                return Err(ProjectStoreError::YardOrchestratorInterventionInProgress);
            }

            let now = unix_time_ms()?;
            let replaced_worker_id = current
                .worker
                .as_ref()
                .and_then(|worker| (worker.id != candidate.worker.id).then(|| worker.id.clone()));
            let result_version = if is_current {
                current.version
            } else {
                let next_version = current
                    .version
                    .checked_add(1)
                    .ok_or(ProjectStoreError::VersionOverflow)?;
                if let Some(replaced_worker_id) = replaced_worker_id.as_deref() {
                    transaction.execute(
                        "UPDATE workers
                            SET version = version + 1, updated_at_unix_ms = ?1
                          WHERE id = ?2",
                        params![to_i64(now)?, replaced_worker_id],
                    )?;
                }
                transaction.execute(
                    "UPDATE workers
                        SET version = version + 1, updated_at_unix_ms = ?1
                      WHERE id = ?2 AND version = ?3",
                    params![
                        to_i64(now)?,
                        command.worker_id,
                        to_i64(command.expected_worker_version)?,
                    ],
                )?;
                transaction.execute(
                    "UPDATE yard_orchestrator
                        SET worker_id = ?1, version = ?2,
                            updated_at_unix_ms = ?3
                      WHERE singleton_id = 1 AND version = ?4",
                    params![
                        command.worker_id,
                        to_i64(next_version)?,
                        to_i64(now)?,
                        to_i64(current.version)?,
                    ],
                )?;
                insert_lifecycle_event(
                    &transaction,
                    "yard_orchestrator",
                    "yard",
                    next_version,
                    if replaced_worker_id.is_some() {
                        "orchestrator_replaced"
                    } else {
                        "orchestrator_configured"
                    },
                    &command.actor,
                    now,
                )?;
                next_version
            };

            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'yard_orchestrator_configure', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO yard_orchestrator_configure_commands (
                    command_id, worker_id, expected_worker_version,
                    expected_orchestrator_version, replaced_worker_id,
                    result_orchestrator_version, finished_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    command.command_id,
                    command.worker_id,
                    to_i64(command.expected_worker_version)?,
                    to_i64(command.expected_orchestrator_version)?,
                    replaced_worker_id,
                    to_i64(result_version)?,
                    to_i64(now)?,
                ],
            )?;
            let configured =
                select_configured_yard_orchestrator(&transaction, &command.command_id, false)?;
            transaction.commit()?;
            Ok(configured)
        })
        .await
    }

    async fn list_projects(&self) -> Result<Projects, ProjectStoreError> {
        self.run(|connection| {
            let mut statement = connection.prepare(&format!(
                "{PROJECT_SELECT} ORDER BY p.created_at_unix_ms, p.id"
            ))?;
            let projects = statement
                .query_map([], project_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Projects { projects })
        })
        .await
    }

    async fn list_project_relationships(&self) -> Result<ProjectRelationships, ProjectStoreError> {
        self.run(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, source_project_id, target_project_id, kind,
                        version, created_by, created_at_unix_ms,
                        updated_at_unix_ms
                   FROM project_relationships
                  ORDER BY created_at_unix_ms, id",
            )?;
            let relationships = statement
                .query_map([], project_relationship_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ProjectRelationships { relationships })
        })
        .await
    }

    async fn create_project_relationship(
        &self,
        command: CreateProjectRelationship,
    ) -> Result<CreatedProjectRelationship, ProjectStoreError> {
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_project_relationship_create_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(existing.created(true));
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            if !project_exists(&transaction, &command.source_project_id)? {
                return Err(ProjectStoreError::RelationshipSourceProjectNotFound);
            }
            if !project_exists(&transaction, &command.target_project_id)? {
                return Err(ProjectStoreError::RelationshipTargetProjectNotFound);
            }
            let relationship_id_used = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM project_relationships WHERE id = ?1
                    UNION ALL
                    SELECT 1 FROM project_relationship_create_commands
                     WHERE relationship_id = ?1
                 )",
                [&command.relationship_id],
                |row| row.get::<_, bool>(0),
            )?;
            if relationship_id_used {
                return Err(ProjectStoreError::ProjectRelationshipIdConflict);
            }
            let semantic_duplicate = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM project_relationships
                     WHERE source_project_id = ?1
                       AND target_project_id = ?2
                       AND kind = ?3
                 )",
                params![
                    command.source_project_id,
                    command.target_project_id,
                    project_relationship_kind_value(command.kind),
                ],
                |row| row.get::<_, bool>(0),
            )?;
            if semantic_duplicate {
                return Err(ProjectStoreError::ProjectRelationshipAlreadyExists);
            }

            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'project_relationship_create', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO project_relationships (
                    id, source_project_id, target_project_id, kind, version,
                    created_by, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?6)",
                params![
                    command.relationship_id,
                    command.source_project_id,
                    command.target_project_id,
                    project_relationship_kind_value(command.kind),
                    command.actor,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO project_relationship_create_commands (
                    command_id, relationship_id, source_project_id,
                    target_project_id, kind, result_version,
                    result_created_by, result_created_at_unix_ms,
                    result_updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)",
                params![
                    command.command_id,
                    command.relationship_id,
                    command.source_project_id,
                    command.target_project_id,
                    project_relationship_kind_value(command.kind),
                    command.actor,
                    to_i64(now)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project_relationship",
                &command.relationship_id,
                1,
                "project_relationship_created",
                &command.actor,
                now,
            )?;
            let created =
                select_project_relationship_create_command(&transaction, &command.command_id)?
                    .ok_or(ProjectStoreError::CommandNotFound)?
                    .created(false);
            transaction.commit()?;
            Ok(created)
        })
        .await
    }

    async fn delete_project_relationship(
        &self,
        relationship_id: &str,
        command: DeleteProjectRelationship,
    ) -> Result<DeletedProjectRelationship, ProjectStoreError> {
        let relationship_id = required_relationship_id(relationship_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_project_relationship_delete_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&relationship_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(existing.deleted(true));
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let relationship = select_project_relationship(&transaction, &relationship_id)?
                .ok_or(ProjectStoreError::ProjectRelationshipNotFound)?;
            if relationship.version != command.expected_version {
                return Err(ProjectStoreError::ProjectRelationshipVersionConflict {
                    current_version: relationship.version,
                });
            }

            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'project_relationship_delete', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO project_relationship_delete_commands (
                    command_id, relationship_id, expected_version,
                    deleted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    command.command_id,
                    relationship_id,
                    to_i64(command.expected_version)?,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "DELETE FROM project_relationships
                  WHERE id = ?1 AND version = ?2",
                params![relationship_id, to_i64(command.expected_version)?],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project_relationship",
                &relationship_id,
                relationship.version,
                "project_relationship_deleted",
                &command.actor,
                now,
            )?;
            transaction.commit()?;
            Ok(DeletedProjectRelationship {
                command_id: command.command_id,
                relationship_id,
                deleted_at_unix_ms: now,
                replayed: false,
            })
        })
        .await
    }

    async fn get_project(&self, project_id: &str) -> Result<Project, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        self.run(move |connection| select_project(connection, &project_id))
            .await
    }

    async fn create_project(
        &self,
        project: CreateProject,
        orchestrator_runtime: WorkerRuntimeBinding,
    ) -> Result<Project, ProjectStoreError> {
        let project = project.normalize()?;
        let orchestrator_runtime = orchestrator_runtime.normalize()?;
        if project.runtime.adapter != orchestrator_runtime.adapter
            || project.runtime.session != orchestrator_runtime.session
            || project.runtime.workspace_id != orchestrator_runtime.workspace_id
        {
            return Err(ProjectStoreError::InvalidProject(
                yard_domain::ProjectValidationError::RuntimeBindingMismatch,
            ));
        }

        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_project_workspace_available(&transaction, &project.runtime)?;
            ensure_runtime_snapshot_current(&transaction, &orchestrator_runtime)?;

            let project_id = Uuid::now_v7().to_string();
            let reusable_worker_id =
                select_reusable_runtime_worker(&transaction, &orchestrator_runtime)?;
            let create_worker = reusable_worker_id.is_none();
            let worker_id = reusable_worker_id.unwrap_or_else(|| Uuid::now_v7().to_string());
            let now = unix_time_ms()?;
            insert_project_aggregate(
                &transaction,
                &project_id,
                &worker_id,
                &project,
                &orchestrator_runtime,
                create_worker,
                None,
                "adopt_existing",
                None,
                now,
            )
            .map_err(map_project_insert_error)?;
            let saved = select_project(&transaction, &project_id)?;
            transaction.commit()?;
            Ok(saved)
        })
        .await
    }

    async fn begin_profile_project_creation(
        &self,
        command: CreateProjectFromProfile,
    ) -> Result<BeginProfileProjectCreation, ProjectStoreError> {
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_profile_project_creation_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => {
                        select_confirmed_project_creation(&transaction, &command.command_id, true)
                            .map(Box::new)
                            .map(BeginProfileProjectCreation::Replayed)
                    }
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "profile-backed project creation failed".to_owned()),
                    )),
                    "pending" | "ambiguous" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "project creation may have reached Herdr; it was not retried".to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            ensure_project_workspace_available(&transaction, &command.runtime)?;
            let current_profile_version =
                select_current_profile_version(&transaction, &command.profile_id)?;
            if current_profile_version != command.expected_profile_version {
                return Err(ProjectStoreError::ProfileVersionConflict {
                    current_version: current_profile_version,
                });
            }
            let profile = select_worker_profile_revision(
                &transaction,
                &command.profile_id,
                command.expected_profile_version,
            )?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'profile_project_creation', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO profile_project_creation_commands (
                    command_id, project_name, runtime_adapter, runtime_session,
                    runtime_workspace_id, profile_id, profile_version,
                    orchestrator_objective, canvas_x, canvas_y, canvas_width,
                    canvas_height, result_project_id, result_worker_id,
                    finished_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    NULL, NULL, NULL
                 )",
                params![
                    command.command_id,
                    command.name,
                    command.runtime.adapter,
                    command.runtime.session,
                    command.runtime.workspace_id,
                    command.profile_id,
                    to_i64(command.expected_profile_version)?,
                    command.orchestrator_objective,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginProfileProjectCreation::Started(Box::new(
                ProfileProjectCreationContext { command, profile },
            )))
        })
        .await
    }

    async fn finalize_profile_project_creation(
        &self,
        command_id: &str,
        orchestrator_runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedProjectCreation, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let orchestrator_runtime = orchestrator_runtime.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_profile_project_creation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status == "succeeded" {
                return select_confirmed_project_creation(&transaction, &command_id, true);
            }
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            if orchestrator_runtime.adapter != command.runtime.adapter
                || orchestrator_runtime.session != command.runtime.session
                || orchestrator_runtime.workspace_id != command.runtime.workspace_id
            {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }

            ensure_project_workspace_unbound(&transaction, &command.runtime)?;
            ensure_runtime_snapshot_current(&transaction, &orchestrator_runtime)?;
            let project_id = Uuid::now_v7().to_string();
            let reusable_worker_id =
                select_reusable_runtime_worker(&transaction, &orchestrator_runtime)?;
            let create_worker = reusable_worker_id.is_none();
            let worker_id = reusable_worker_id.unwrap_or_else(|| Uuid::now_v7().to_string());
            let project = CreateProject {
                name: command.project_name,
                runtime: command.runtime,
                orchestrator_observed_worker_id: orchestrator_runtime.terminal_id.clone(),
                placement: command.placement,
            };
            let now = unix_time_ms()?;
            insert_project_aggregate(
                &transaction,
                &project_id,
                &worker_id,
                &project,
                &orchestrator_runtime,
                create_worker,
                Some((&command.profile_id, command.profile_version)),
                "create_new",
                Some(&command_id),
                now,
            )
            .map_err(map_project_insert_error)?;
            transaction.execute(
                "UPDATE profile_project_creation_commands
                    SET result_project_id = ?1, result_worker_id = ?2,
                        finished_at_unix_ms = ?3
                  WHERE command_id = ?4 AND finished_at_unix_ms IS NULL",
                params![project_id, worker_id, to_i64(now)?, command_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project",
                &project_id,
                1,
                "profile_backed_project_created",
                "user",
                now,
            )?;
            let created = select_project(&transaction, &project_id)?;
            transaction.commit()?;
            Ok(ConfirmedProjectCreation {
                command_id,
                project: created,
                replayed: false,
            })
        })
        .await
    }

    async fn fail_profile_project_creation(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_profile_project_creation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Ok(());
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    message,
                    to_i64(now)?,
                    command_id,
                ],
            )?;
            if !ambiguous {
                transaction.execute(
                    "UPDATE profile_project_creation_commands
                        SET finished_at_unix_ms = ?1
                      WHERE command_id = ?2 AND finished_at_unix_ms IS NULL",
                    params![to_i64(now)?, command_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn begin_workspace_project_creation(
        &self,
        command: CreateWorkspaceProjectFromProfile,
    ) -> Result<BeginWorkspaceProjectCreation, ProjectStoreError> {
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_workspace_project_creation_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => select_confirmed_workspace_project_creation(
                        &transaction,
                        &command.command_id,
                        true,
                    )
                    .map(Box::new)
                    .map(BeginWorkspaceProjectCreation::Replayed),
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing.error_message.unwrap_or_else(|| {
                            "workspace-backed project creation failed".to_owned()
                        }),
                    )),
                    "pending" | "ambiguous" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "workspace creation may have reached the runtime; it was not retried"
                                .to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            ensure_workspace_project_creation_available(&transaction, &command)?;
            let current_profile_version =
                select_current_profile_version(&transaction, &command.profile_id)?;
            if current_profile_version != command.expected_profile_version {
                return Err(ProjectStoreError::ProfileVersionConflict {
                    current_version: current_profile_version,
                });
            }
            let profile = select_worker_profile_revision(
                &transaction,
                &command.profile_id,
                command.expected_profile_version,
            )?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'profile_project_creation', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO workspace_project_creation_commands (
                    command_id, project_name, runtime_adapter, runtime_session,
                    workspace_label, cwd, profile_id, profile_version,
                    orchestrator_objective, canvas_x, canvas_y, canvas_width,
                    canvas_height, result_runtime_workspace_id,
                    result_project_id, result_worker_id, finished_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, NULL, NULL, NULL, NULL
                 )",
                params![
                    command.command_id,
                    command.name,
                    command.runtime_adapter,
                    command.runtime_session,
                    command.workspace_label,
                    command.cwd,
                    command.profile_id,
                    to_i64(command.expected_profile_version)?,
                    command.orchestrator_objective,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginWorkspaceProjectCreation::Started(Box::new(
                WorkspaceProjectCreationContext { command, profile },
            )))
        })
        .await
    }

    async fn finalize_workspace_project_creation(
        &self,
        command_id: &str,
        orchestrator_runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedProjectCreation, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let orchestrator_runtime = orchestrator_runtime.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_workspace_project_creation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status == "succeeded" {
                return select_confirmed_workspace_project_creation(
                    &transaction,
                    &command_id,
                    true,
                );
            }
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            if orchestrator_runtime.adapter != command.runtime_adapter
                || orchestrator_runtime.session != command.runtime_session
            {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }

            let project_runtime = ProjectRuntimeBinding {
                adapter: command.runtime_adapter,
                session: command.runtime_session,
                workspace_id: orchestrator_runtime.workspace_id.clone(),
            };
            ensure_project_workspace_unbound(&transaction, &project_runtime)?;
            ensure_runtime_snapshot_current(&transaction, &orchestrator_runtime)?;
            let project_id = Uuid::now_v7().to_string();
            let reusable_worker_id =
                select_reusable_runtime_worker(&transaction, &orchestrator_runtime)?;
            let create_worker = reusable_worker_id.is_none();
            let worker_id = reusable_worker_id.unwrap_or_else(|| Uuid::now_v7().to_string());
            let project = CreateProject {
                name: command.project_name,
                runtime: project_runtime,
                orchestrator_observed_worker_id: orchestrator_runtime.terminal_id.clone(),
                placement: command.placement,
            };
            let now = unix_time_ms()?;
            insert_project_aggregate(
                &transaction,
                &project_id,
                &worker_id,
                &project,
                &orchestrator_runtime,
                create_worker,
                Some((&command.profile_id, command.profile_version)),
                "create_new",
                Some(&command_id),
                now,
            )
            .map_err(map_project_insert_error)?;
            transaction.execute(
                "UPDATE workspace_project_creation_commands
                    SET result_runtime_workspace_id = ?1,
                        result_project_id = ?2, result_worker_id = ?3,
                        finished_at_unix_ms = ?4
                  WHERE command_id = ?5 AND finished_at_unix_ms IS NULL",
                params![
                    project.runtime.workspace_id,
                    project_id,
                    worker_id,
                    to_i64(now)?,
                    command_id,
                ],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project",
                &project_id,
                1,
                "workspace_backed_project_created",
                "user",
                now,
            )?;
            let created = select_project(&transaction, &project_id)?;
            transaction.commit()?;
            Ok(ConfirmedProjectCreation {
                command_id,
                project: created,
                replayed: false,
            })
        })
        .await
    }

    async fn fail_workspace_project_creation(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_workspace_project_creation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Ok(());
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    message,
                    to_i64(now)?,
                    command_id,
                ],
            )?;
            if !ambiguous {
                transaction.execute(
                    "UPDATE workspace_project_creation_commands
                        SET finished_at_unix_ms = ?1
                      WHERE command_id = ?2 AND finished_at_unix_ms IS NULL",
                    params![to_i64(now)?, command_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn update_project_placement(
        &self,
        project_id: &str,
        update: UpdateProjectPlacement,
    ) -> Result<Project, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        update.validate()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_version = transaction
                .query_row(
                    "SELECT version FROM project_placements WHERE project_id = ?1",
                    [&project_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .ok_or(ProjectStoreError::ProjectNotFound)?;
            let current_version = to_u64(current_version)?;
            if current_version != update.expected_version {
                return Err(ProjectStoreError::VersionConflict { current_version });
            }
            let next_version = current_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE project_placements
                    SET canvas_x = ?1,
                        canvas_y = ?2,
                        canvas_width = ?3,
                        canvas_height = ?4,
                        version = ?5,
                        updated_at_unix_ms = ?6
                  WHERE project_id = ?7",
                params![
                    update.placement.x,
                    update.placement.y,
                    update.placement.width,
                    update.placement.height,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    project_id,
                ],
            )?;
            let project = select_project(&transaction, &project_id)?;
            transaction.commit()?;
            Ok(project)
        })
        .await
    }

    async fn list_worker_profiles(&self) -> Result<WorkerProfiles, ProjectStoreError> {
        self.run(|connection| {
            let mut statement = connection.prepare(&format!(
                "{PROFILE_SELECT} ORDER BY wp.name COLLATE NOCASE, wp.id"
            ))?;
            let keys = statement
                .query_map([], profile_key_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            let profiles = keys
                .into_iter()
                .filter(|(profile_id, _)| profile_id != BLANK_WORKER_PROFILE_ID)
                .map(|(profile_id, version)| {
                    select_worker_profile_revision(connection, &profile_id, version)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(WorkerProfiles { profiles })
        })
        .await
    }

    async fn get_worker_profile(
        &self,
        profile_id: &str,
    ) -> Result<WorkerProfile, ProjectStoreError> {
        let profile_id = required_profile_id(profile_id)?;
        self.run(move |connection| select_worker_profile(connection, &profile_id))
            .await
    }

    async fn pin_worker_profile(
        &self,
        worker_id: &str,
        profile_id: &str,
        expected_profile_version: u64,
    ) -> Result<Worker, ProjectStoreError> {
        let worker_id = worker_id.trim();
        if worker_id.is_empty() {
            return Err(ProjectStoreError::WorkerNotFound);
        }
        let worker_id = worker_id.to_owned();
        let profile_id = required_profile_id(profile_id)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_profile_version =
                select_current_profile_version(&transaction, &profile_id)?;
            if current_profile_version != expected_profile_version {
                return Err(ProjectStoreError::ProfileVersionConflict {
                    current_version: current_profile_version,
                });
            }
            select_worker_profile_revision(&transaction, &profile_id, expected_profile_version)?;
            let candidate = select_worker_candidate(&transaction, &worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            match (
                candidate.worker.profile_id.as_deref(),
                candidate.worker.profile_version,
            ) {
                (Some(current_id), Some(current_version))
                    if current_id == profile_id && current_version == expected_profile_version =>
                {
                    transaction.commit()?;
                    return Ok(candidate.worker);
                }
                (Some(_), Some(_)) => {
                    return Err(ProjectStoreError::WorkerProfileAlreadyPinned);
                }
                (None, None) => {}
                _ => return Err(ProjectStoreError::InvalidWorkerProfileBinding),
            }
            if !matches!(
                candidate.availability,
                WorkerAvailability::UnassignedLive | WorkerAvailability::YardOrchestrator
            ) {
                return Err(ProjectStoreError::WorkerNotAvailable {
                    availability: candidate.availability,
                });
            }
            let next_version = candidate
                .worker
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            let rows = transaction.execute(
                "UPDATE workers
                    SET profile_id = ?1, profile_version = ?2, version = ?3,
                        updated_at_unix_ms = ?4
                  WHERE id = ?5 AND version = ?6
                    AND profile_id IS NULL AND profile_version IS NULL",
                params![
                    profile_id,
                    to_i64(expected_profile_version)?,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    worker_id,
                    to_i64(candidate.worker.version)?,
                ],
            )?;
            if rows != 1 {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: select_worker_candidate(&transaction, &worker_id)?
                        .map_or(candidate.worker.version, |current| current.worker.version),
                });
            }
            insert_lifecycle_event(
                &transaction,
                "worker",
                &worker_id,
                next_version,
                "worker_profile_pinned",
                "yard",
                now,
            )?;
            let worker = select_worker_candidate(&transaction, &worker_id)?
                .map(|candidate| candidate.worker)
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            transaction.commit()?;
            Ok(worker)
        })
        .await
    }

    async fn create_worker_profile(
        &self,
        profile: CreateWorkerProfile,
    ) -> Result<WorkerProfile, ProjectStoreError> {
        let profile = profile.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let profile_id = Uuid::now_v7().to_string();
            let now = unix_time_ms()?;
            let result = transaction.execute(
                "INSERT INTO worker_profiles (
                    id, name, current_version, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, 1, ?3, ?3)",
                params![profile_id, profile.spec.name, to_i64(now)?],
            );
            if let Err(error) = result {
                return if is_unique_constraint(&error) {
                    Err(ProjectStoreError::ProfileNameAlreadyExists)
                } else {
                    Err(error.into())
                };
            }
            insert_profile_revision(&transaction, &profile_id, 1, &profile.spec, now)?;
            let saved = select_worker_profile(&transaction, &profile_id)?;
            transaction.commit()?;
            Ok(saved)
        })
        .await
    }

    async fn update_worker_profile(
        &self,
        profile_id: &str,
        update: UpdateWorkerProfile,
    ) -> Result<WorkerProfile, ProjectStoreError> {
        let profile_id = required_profile_id(profile_id)?;
        let update = update.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_version = transaction
                .query_row(
                    "SELECT current_version FROM worker_profiles WHERE id = ?1",
                    [&profile_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .ok_or(ProjectStoreError::ProfileNotFound)?;
            let current_version = to_u64(current_version)?;
            if current_version != update.expected_version {
                return Err(ProjectStoreError::ProfileVersionConflict { current_version });
            }
            let next_version = current_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            let result = transaction.execute(
                "UPDATE worker_profiles
                    SET name = ?1, current_version = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4",
                params![
                    update.spec.name,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    profile_id,
                ],
            );
            if let Err(error) = result {
                return if is_unique_constraint(&error) {
                    Err(ProjectStoreError::ProfileNameAlreadyExists)
                } else {
                    Err(error.into())
                };
            }
            insert_profile_revision(&transaction, &profile_id, next_version, &update.spec, now)?;
            let saved = select_worker_profile(&transaction, &profile_id)?;
            transaction.commit()?;
            Ok(saved)
        })
        .await
    }

    async fn begin_profile_allocation(
        &self,
        project_id: &str,
        command: ConfirmProfileAllocation,
    ) -> Result<BeginProfileAllocation, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_allocation_command(&transaction, &command.command_id)? {
                if !existing.matches(&project_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => {
                        select_confirmed_allocation(&transaction, &command.command_id, true)
                            .map(Box::new)
                            .map(BeginProfileAllocation::Replayed)
                    }
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "allocation command failed".to_owned()),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let project = select_project(&transaction, &project_id)?;
            if project.version != command.expected_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: project.version,
                });
            }
            let current_profile_version =
                select_current_profile_version(&transaction, &command.profile_id)?;
            if current_profile_version != command.expected_profile_version {
                return Err(ProjectStoreError::ProfileVersionConflict {
                    current_version: current_profile_version,
                });
            }
            let profile = select_worker_profile_revision(
                &transaction,
                &command.profile_id,
                command.expected_profile_version,
            )?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, 'profile_allocation', ?2, 'pending', NULL, ?3, ?3)",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO profile_allocation_commands (
                    command_id, project_id, profile_id, profile_version,
                    expected_project_version, objective, role, isolation_policy
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'project_workspace')",
                params![
                    command.command_id,
                    project_id,
                    command.profile_id,
                    to_i64(command.expected_profile_version)?,
                    to_i64(command.expected_project_version)?,
                    command.objective,
                    command.role,
                ],
            )?;
            let next_project_version = project
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            transaction.execute(
                "UPDATE projects
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3",
                params![to_i64(next_project_version)?, to_i64(now)?, project_id,],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project",
                &project_id,
                next_project_version,
                "profile_allocation_confirmed",
                "user",
                now,
            )?;
            transaction.commit()?;
            Ok(BeginProfileAllocation::Started(Box::new(
                AllocationContext {
                    command,
                    project,
                    profile,
                },
            )))
        })
        .await
    }

    async fn persist_runtime_allocation(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedAllocation, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime = runtime.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_allocation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            if command.result_assignment_id.is_some() {
                return select_confirmed_allocation(&transaction, &command_id, true);
            }
            let project = select_project(&transaction, &command.project_id)?;
            if runtime.adapter != project.runtime.adapter
                || runtime.session != project.runtime.session
                || runtime.workspace_id != project.runtime.workspace_id
            {
                return Err(ProjectStoreError::InvalidProject(
                    yard_domain::ProjectValidationError::RuntimeBindingMismatch,
                ));
            }
            ensure_worker_binding_available(&transaction, &runtime)?;

            let worker_id = Uuid::now_v7().to_string();
            let allocation_id = Uuid::now_v7().to_string();
            let assignment_id = Uuid::now_v7().to_string();
            let attempt_id = Uuid::now_v7().to_string();
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO workers (
                    id, profile_id, profile_version, desired_state, version,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, 'running', 1, ?4, ?4)",
                params![
                    worker_id,
                    command.profile_id,
                    to_i64(command.profile_version)?,
                    to_i64(now)?,
                ],
            )?;
            insert_worker_runtime_binding(&transaction, &worker_id, &runtime, now)?;
            transaction.execute(
                "INSERT INTO worker_allocations (
                    id, project_id, worker_id, mode, started_by_command_id,
                    started_at_unix_ms, ended_at_unix_ms
                 ) VALUES (?1, ?2, ?3, 'create_new', ?4, ?5, NULL)",
                params![
                    allocation_id,
                    command.project_id,
                    worker_id,
                    command_id,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO assignments (
                    id, project_id, allocation_id, worker_id,
                    profile_id, profile_version, objective, role,
                    isolation_policy, lifecycle, version,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    'project_workspace', 'allocating', 1, ?9, ?9
                 )",
                params![
                    assignment_id,
                    command.project_id,
                    allocation_id,
                    worker_id,
                    command.profile_id,
                    to_i64(command.profile_version)?,
                    command.objective,
                    command.role,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO assignment_attempts (
                    id, assignment_id, ordinal, lifecycle, error_message,
                    version, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, 1, 'starting', NULL, 1, ?3, ?3)",
                params![attempt_id, assignment_id, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE profile_allocation_commands
                    SET result_worker_id = ?1,
                        result_allocation_id = ?2,
                        result_assignment_id = ?3
                  WHERE command_id = ?4",
                params![worker_id, allocation_id, assignment_id, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                1,
                "runtime_worker_created",
                "herdr",
                now,
            )?;
            let allocation = select_confirmed_allocation(&transaction, &command_id, false)?;
            transaction.commit()?;
            Ok(allocation)
        })
        .await
    }

    async fn activate_profile_allocation(
        &self,
        command_id: &str,
    ) -> Result<ConfirmedAllocation, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_allocation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status == "succeeded" {
                return select_confirmed_allocation(&transaction, &command_id, true);
            }
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let assignment_id = command
                .result_assignment_id
                .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE assignments
                    SET lifecycle = 'active', version = version + 1,
                        updated_at_unix_ms = ?1
                  WHERE id = ?2 AND lifecycle = 'allocating'",
                params![to_i64(now)?, assignment_id],
            )?;
            transaction.execute(
                "UPDATE assignment_attempts
                    SET lifecycle = 'active', version = version + 1,
                        updated_at_unix_ms = ?1
                  WHERE assignment_id = ?2 AND lifecycle = 'starting'",
                params![to_i64(now)?, assignment_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                2,
                "objective_delivered",
                "herdr",
                now,
            )?;
            let allocation = select_confirmed_allocation(&transaction, &command_id, false)?;
            transaction.commit()?;
            Ok(allocation)
        })
        .await
    }

    async fn fail_profile_allocation(
        &self,
        command_id: &str,
        message: &str,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_allocation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Ok(());
            }
            let now = unix_time_ms()?;
            if let Some(assignment_id) = command.result_assignment_id {
                transaction.execute(
                    "UPDATE assignments
                        SET lifecycle = 'failed', version = version + 1,
                            updated_at_unix_ms = ?1
                      WHERE id = ?2 AND lifecycle = 'allocating'",
                    params![to_i64(now)?, assignment_id],
                )?;
                transaction.execute(
                    "UPDATE assignment_attempts
                        SET lifecycle = 'failed', error_message = ?1,
                            version = version + 1, updated_at_unix_ms = ?2
                      WHERE assignment_id = ?3 AND lifecycle = 'starting'",
                    params![message, to_i64(now)?, assignment_id],
                )?;
                transaction.execute(
                    "UPDATE worker_allocations
                        SET ended_at_unix_ms = ?1
                      WHERE id = (
                          SELECT allocation_id
                            FROM assignments
                           WHERE id = ?2
                      )
                        AND ended_at_unix_ms IS NULL",
                    params![to_i64(now)?, assignment_id],
                )?;
                insert_lifecycle_event(
                    &transaction,
                    "assignment",
                    &assignment_id,
                    2,
                    "objective_delivery_failed",
                    "herdr",
                    now,
                )?;
            }
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'failed', error_message = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND status = 'pending'",
                params![message, to_i64(now)?, command_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn list_worker_candidates(&self) -> Result<WorkerCandidates, ProjectStoreError> {
        self.run(|connection| {
            let mut statement = connection.prepare(&format!(
                "{WORKER_CANDIDATE_SELECT}
                 ORDER BY w.created_at_unix_ms, w.id"
            ))?;
            let workers = statement
                .query_map([], worker_candidate_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(WorkerCandidates { workers })
        })
        .await
    }

    async fn begin_worker_allocation(
        &self,
        project_id: &str,
        command: ConfirmWorkerAllocation,
    ) -> Result<BeginWorkerAllocation, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_worker_allocation_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&project_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => {
                        select_confirmed_worker_allocation(&transaction, &command.command_id, true)
                            .map(Box::new)
                            .map(BeginWorkerAllocation::Replayed)
                    }
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "worker allocation command failed".to_owned()),
                    )),
                    "ambiguous" | "pending" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "worker assignment may have reached Herdr; it was not retried"
                                .to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let project = select_project(&transaction, &project_id)?;
            if project.version != command.expected_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: project.version,
                });
            }
            let candidate = select_worker_candidate(&transaction, &command.worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            if candidate.worker.version != command.expected_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: candidate.worker.version,
                });
            }
            if !matches!(
                candidate.availability,
                WorkerAvailability::UnassignedLive | WorkerAvailability::Resumable
            ) {
                return Err(ProjectStoreError::WorkerNotAvailable {
                    availability: candidate.availability,
                });
            }

            let replace_runtime = candidate.availability == WorkerAvailability::Resumable;
            if !replace_runtime {
                let runtime = candidate
                    .worker
                    .runtime
                    .as_ref()
                    .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
                if runtime.adapter != project.runtime.adapter
                    || runtime.session != project.runtime.session
                    || runtime.workspace_id != project.runtime.workspace_id
                {
                    return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
                }
            }

            let (profile_id, profile_version) =
                resolve_worker_allocation_profile(&transaction, &candidate.worker, &command)?;
            let profile =
                select_worker_profile_revision(&transaction, &profile_id, profile_version)?;
            let next_worker_version = candidate
                .worker
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_project_version = project
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let allocation_id = Uuid::now_v7().to_string();
            let assignment_id = Uuid::now_v7().to_string();
            let attempt_id = Uuid::now_v7().to_string();
            let now = unix_time_ms()?;

            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, 'worker_allocation', ?2, 'pending', NULL, ?3, ?3)",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO worker_allocation_commands (
                    command_id, project_id, worker_id, expected_worker_version,
                    requested_profile_id, requested_profile_version,
                    expected_project_version, objective, role, isolation_policy,
                    replace_runtime, result_allocation_id, result_assignment_id
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    'project_workspace', ?10, NULL, NULL
                 )",
                params![
                    command.command_id,
                    project_id,
                    command.worker_id,
                    to_i64(command.expected_worker_version)?,
                    command.profile_id,
                    command.expected_profile_version.map(to_i64).transpose()?,
                    to_i64(command.expected_project_version)?,
                    command.objective,
                    command.role,
                    replace_runtime,
                ],
            )?;
            transaction.execute(
                "UPDATE workers
                    SET profile_id = ?1, profile_version = ?2,
                        version = ?3, updated_at_unix_ms = ?4
                  WHERE id = ?5 AND version = ?6",
                params![
                    profile_id,
                    to_i64(profile_version)?,
                    to_i64(next_worker_version)?,
                    to_i64(now)?,
                    command.worker_id,
                    to_i64(command.expected_worker_version)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO worker_allocations (
                    id, project_id, worker_id, mode, started_by_command_id,
                    started_at_unix_ms, ended_at_unix_ms
                 ) VALUES (?1, ?2, ?3, 'adopt_existing', ?4, ?5, NULL)",
                params![
                    allocation_id,
                    project_id,
                    command.worker_id,
                    command.command_id,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO assignments (
                    id, project_id, allocation_id, worker_id,
                    profile_id, profile_version, objective, role,
                    isolation_policy, lifecycle, version,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    'project_workspace', 'allocating', 1, ?9, ?9
                 )",
                params![
                    assignment_id,
                    project_id,
                    allocation_id,
                    command.worker_id,
                    profile_id,
                    to_i64(profile_version)?,
                    command.objective,
                    command.role,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO assignment_attempts (
                    id, assignment_id, ordinal, lifecycle, error_message,
                    version, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, 1, 'starting', NULL, 1, ?3, ?3)",
                params![attempt_id, assignment_id, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE worker_allocation_commands
                    SET result_allocation_id = ?1, result_assignment_id = ?2
                  WHERE command_id = ?3",
                params![allocation_id, assignment_id, command.command_id],
            )?;
            transaction.execute(
                "UPDATE projects
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(next_project_version)?,
                    to_i64(now)?,
                    project_id,
                    to_i64(command.expected_project_version)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project",
                &project_id,
                next_project_version,
                "worker_allocation_confirmed",
                "user",
                now,
            )?;
            insert_lifecycle_event(
                &transaction,
                "worker",
                &command.worker_id,
                next_worker_version,
                if replace_runtime {
                    "runtime_replacement_requested"
                } else {
                    "worker_assignment_reserved"
                },
                "user",
                now,
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                1,
                "worker_allocation_confirmed",
                "user",
                now,
            )?;

            let mut worker = candidate.worker;
            worker.profile_id = Some(profile_id);
            worker.profile_version = Some(profile_version);
            worker.version = next_worker_version;
            worker.updated_at_unix_ms = now;
            transaction.commit()?;
            Ok(BeginWorkerAllocation::Started(Box::new(
                WorkerAllocationContext {
                    command,
                    project,
                    worker,
                    profile,
                    replace_runtime,
                },
            )))
        })
        .await
    }

    async fn replace_worker_runtime(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedAllocation, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime = runtime.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_worker_allocation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            if !command.replace_runtime {
                return Err(ProjectStoreError::RuntimeReplacementNotRequired);
            }
            let project = select_project(&transaction, &command.project_id)?;
            if runtime.adapter != project.runtime.adapter
                || runtime.session != project.runtime.session
                || runtime.workspace_id != project.runtime.workspace_id
            {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }
            ensure_runtime_snapshot_current(&transaction, &runtime)?;
            ensure_worker_binding_available_for(&transaction, &command.worker_id, &runtime)?;
            let worker_version = transaction.query_row(
                "SELECT version FROM workers WHERE id = ?1",
                [&command.worker_id],
                |row| row.get::<_, i64>(0),
            )?;
            let worker_version = to_u64(worker_version)?;
            let next_worker_version = worker_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            let binding_exists = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM worker_runtime_bindings WHERE worker_id = ?1
                 )",
                [&command.worker_id],
                |row| row.get::<_, bool>(0),
            )?;
            if binding_exists {
                replace_worker_runtime_binding(&transaction, &command.worker_id, &runtime, now)?;
            } else {
                insert_worker_runtime_binding(&transaction, &command.worker_id, &runtime, now)?;
            }
            transaction.execute(
                "UPDATE workers
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(next_worker_version)?,
                    to_i64(now)?,
                    command.worker_id,
                    to_i64(worker_version)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "worker",
                &command.worker_id,
                next_worker_version,
                "runtime_binding_replaced",
                "herdr",
                now,
            )?;
            let allocation = select_confirmed_worker_allocation(&transaction, &command_id, false)?;
            transaction.commit()?;
            Ok(allocation)
        })
        .await
    }

    async fn activate_worker_allocation(
        &self,
        command_id: &str,
    ) -> Result<ConfirmedAllocation, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_worker_allocation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status == "succeeded" {
                return select_confirmed_worker_allocation(&transaction, &command_id, true);
            }
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let assignment_id = command
                .result_assignment_id
                .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
            let now = unix_time_ms()?;
            let assignment_rows = transaction.execute(
                "UPDATE assignments
                    SET lifecycle = 'active', version = version + 1,
                        updated_at_unix_ms = ?1
                  WHERE id = ?2 AND lifecycle = 'allocating'",
                params![to_i64(now)?, assignment_id],
            )?;
            let attempt_rows = transaction.execute(
                "UPDATE assignment_attempts
                    SET lifecycle = 'active', version = version + 1,
                        updated_at_unix_ms = ?1
                  WHERE assignment_id = ?2 AND lifecycle = 'starting'",
                params![to_i64(now)?, assignment_id],
            )?;
            if assignment_rows != 1 || attempt_rows != 1 {
                return Err(ProjectStoreError::CommandNotPending);
            }
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                2,
                "objective_delivered",
                "herdr",
                now,
            )?;
            let allocation = select_confirmed_worker_allocation(&transaction, &command_id, false)?;
            transaction.commit()?;
            Ok(allocation)
        })
        .await
    }

    async fn fail_worker_allocation(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_worker_allocation_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Ok(());
            }
            let now = unix_time_ms()?;
            if !ambiguous {
                let assignment_id = command
                    .result_assignment_id
                    .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
                transaction.execute(
                    "UPDATE assignments
                        SET lifecycle = 'failed', version = version + 1,
                            updated_at_unix_ms = ?1
                      WHERE id = ?2 AND lifecycle = 'allocating'",
                    params![to_i64(now)?, assignment_id],
                )?;
                transaction.execute(
                    "UPDATE assignment_attempts
                        SET lifecycle = 'failed', error_message = ?1,
                            version = version + 1, updated_at_unix_ms = ?2
                      WHERE assignment_id = ?3 AND lifecycle = 'starting'",
                    params![message, to_i64(now)?, assignment_id],
                )?;
                transaction.execute(
                    "UPDATE worker_allocations
                        SET ended_at_unix_ms = ?1
                      WHERE id = ?2 AND ended_at_unix_ms IS NULL",
                    params![
                        to_i64(now)?,
                        command
                            .result_allocation_id
                            .ok_or(ProjectStoreError::RuntimeAllocationMissing)?,
                    ],
                )?;
                insert_lifecycle_event(
                    &transaction,
                    "assignment",
                    &assignment_id,
                    2,
                    "objective_delivery_failed",
                    "herdr",
                    now,
                )?;
            }
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    message,
                    to_i64(now)?,
                    command_id,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn begin_worker_handoff(
        &self,
        source_project_id: &str,
        source_assignment_id: &str,
        command: ConfirmWorkerHandoff,
    ) -> Result<BeginWorkerHandoff, ProjectStoreError> {
        let source_project_id = required_id(source_project_id)?;
        let source_assignment_id = required_id(source_assignment_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_worker_handoff_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&source_project_id, &source_assignment_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => {
                        select_confirmed_worker_handoff(&transaction, &command.command_id, true)
                            .map(Box::new)
                            .map(BeginWorkerHandoff::Replayed)
                    }
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "worker handoff command failed".to_owned()),
                    )),
                    "pending" | "ambiguous" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "worker handoff may have reached Herdr; it was not retried".to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            if source_project_id == command.target_project_id {
                return Err(ProjectStoreError::HandoffTargetMatchesSource);
            }

            let source_project = select_project(&transaction, &source_project_id)?;
            if source_project.version != command.expected_source_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: source_project.version,
                });
            }
            let target_project = select_project(&transaction, &command.target_project_id)?;
            if target_project.version != command.expected_target_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: target_project.version,
                });
            }
            if command.target_role == HandoffTargetRole::Orchestrator {
                let prompt_pending = transaction.query_row(
                    "SELECT EXISTS (
                        SELECT 1
                          FROM orchestrator_prompt_commands opc
                          JOIN command_acknowledgements ca ON ca.id = opc.command_id
                         WHERE opc.project_id = ?1 AND ca.status = 'pending'
                        UNION ALL
                        SELECT 1
                          FROM yard_orchestrator_route_commands yorc
                          JOIN command_acknowledgements ca
                            ON ca.id = yorc.command_id
                         WHERE yorc.target_project_id = ?1
                           AND ca.status = 'pending'
                     )",
                    [&command.target_project_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if prompt_pending {
                    return Err(ProjectStoreError::OrchestratorInterventionInProgress);
                }
            }

            let mut source_record = select_assignment_record(&transaction, &source_assignment_id)?
                .ok_or(ProjectStoreError::AssignmentNotFound)?;
            source_record.assignment.completion_receipt =
                select_completion_receipt(&transaction, &source_assignment_id)?;
            let source_assignment = &source_record.assignment;
            if source_assignment.project_id != source_project_id
                || source_assignment.worker.id != command.worker_id
            {
                return Err(ProjectStoreError::AssignmentNotFound);
            }
            if source_assignment.lifecycle != AssignmentLifecycle::Active {
                return Err(ProjectStoreError::AssignmentNotActive);
            }
            if source_assignment.version != command.expected_source_assignment_version {
                return Err(ProjectStoreError::AssignmentVersionConflict {
                    current_version: source_assignment.version,
                });
            }
            if source_assignment.attempt.id != command.source_attempt_id {
                return Err(ProjectStoreError::AttemptNotCurrent {
                    current_attempt_id: source_assignment.attempt.id.clone(),
                });
            }
            if source_assignment.attempt.lifecycle != AttemptLifecycle::Active {
                return Err(ProjectStoreError::AttemptNotActive);
            }
            if source_assignment.attempt.version != command.expected_source_attempt_version {
                return Err(ProjectStoreError::AttemptVersionConflict {
                    current_version: source_assignment.attempt.version,
                });
            }
            if source_assignment.worker.version != command.expected_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: source_assignment.worker.version,
                });
            }
            let is_orchestrator = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM projects WHERE orchestrator_worker_id = ?1
                 )",
                [&command.worker_id],
                |row| row.get::<_, bool>(0),
            )?;
            if is_orchestrator {
                return Err(ProjectStoreError::OrchestratorHandoffForbidden);
            }
            let prompt_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM assignment_prompt_commands apc
                      JOIN command_acknowledgements ca ON ca.id = apc.command_id
                     WHERE apc.assignment_id = ?1 AND ca.status = 'pending'
                 )",
                [&source_assignment_id],
                |row| row.get::<_, bool>(0),
            )?;
            if prompt_pending {
                return Err(ProjectStoreError::AssignmentInterventionInProgress);
            }
            let allocation_active = transaction.query_row(
                "SELECT ended_at_unix_ms IS NULL
                   FROM worker_allocations
                  WHERE id = ?1",
                [&source_record.allocation.id],
                |row| row.get::<_, bool>(0),
            )?;
            if !allocation_active {
                return Err(ProjectStoreError::AssignmentNotActive);
            }
            let source_runtime = source_assignment
                .worker
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let profile = select_worker_profile_revision(
                &transaction,
                &source_assignment.profile_id,
                source_assignment.profile_version,
            )?;
            let replaced_orchestrator_worker_id = (command.target_role
                == HandoffTargetRole::Orchestrator)
                .then(|| target_project.orchestrator.id.clone());
            let next_worker_version = source_assignment
                .worker
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_assignment_version = source_assignment
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_attempt_version = source_assignment
                .attempt
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_source_project_version = source_project
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_target_project_version = target_project
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;

            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, 'worker_handoff', ?2, 'pending', NULL, ?3, ?3)",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction
                .execute(
                    "INSERT INTO worker_handoff_commands (
                    command_id, source_project_id, source_assignment_id,
                    source_attempt_id, worker_id, expected_worker_version,
                    expected_source_project_version,
                    expected_source_assignment_version,
                    expected_source_attempt_version, target_project_id,
                    expected_target_project_version, target_role, objective, role,
                    isolation_policy, source_runtime_adapter,
                    source_runtime_session, source_runtime_workspace_id,
                    source_terminal_id, source_tab_id, source_pane_id,
                    source_provider_session_source,
                    source_provider_session_provider,
                    source_provider_session_kind, source_provider_session_value,
                    replaced_orchestrator_worker_id
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, 'project_workspace', ?15, ?16, ?17, ?18, ?19,
                    ?20, ?21, ?22, ?23, ?24, ?25
                 )",
                    params![
                        command.command_id,
                        source_project_id,
                        source_assignment_id,
                        command.source_attempt_id,
                        command.worker_id,
                        to_i64(command.expected_worker_version)?,
                        to_i64(command.expected_source_project_version)?,
                        to_i64(command.expected_source_assignment_version)?,
                        to_i64(command.expected_source_attempt_version)?,
                        command.target_project_id,
                        to_i64(command.expected_target_project_version)?,
                        handoff_target_role_value(command.target_role),
                        command.objective,
                        command.role,
                        source_runtime.adapter,
                        source_runtime.session,
                        source_runtime.workspace_id,
                        source_runtime.terminal_id,
                        source_runtime.tab_id,
                        source_runtime.pane_id,
                        source_runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.source.as_str()),
                        source_runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.provider.as_str()),
                        source_runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.kind.as_str()),
                        source_runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.value.as_str()),
                        replaced_orchestrator_worker_id,
                    ],
                )
                .map_err(map_handoff_reservation_error)?;
            let assignment_rows = transaction.execute(
                "UPDATE assignments
                    SET lifecycle = 'handing_off', version = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND lifecycle = 'active' AND version = ?4",
                params![
                    to_i64(next_assignment_version)?,
                    to_i64(now)?,
                    source_assignment_id,
                    to_i64(command.expected_source_assignment_version)?,
                ],
            )?;
            let attempt_rows = transaction.execute(
                "UPDATE assignment_attempts
                    SET lifecycle = 'handing_off', version = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND lifecycle = 'active' AND version = ?4",
                params![
                    to_i64(next_attempt_version)?,
                    to_i64(now)?,
                    command.source_attempt_id,
                    to_i64(command.expected_source_attempt_version)?,
                ],
            )?;
            let worker_rows = transaction.execute(
                "UPDATE workers
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(next_worker_version)?,
                    to_i64(now)?,
                    command.worker_id,
                    to_i64(command.expected_worker_version)?,
                ],
            )?;
            let source_project_rows = transaction.execute(
                "UPDATE projects
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(next_source_project_version)?,
                    to_i64(now)?,
                    source_project_id,
                    to_i64(command.expected_source_project_version)?,
                ],
            )?;
            let target_project_rows = transaction.execute(
                "UPDATE projects
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(next_target_project_version)?,
                    to_i64(now)?,
                    command.target_project_id,
                    to_i64(command.expected_target_project_version)?,
                ],
            )?;
            if assignment_rows != 1
                || attempt_rows != 1
                || worker_rows != 1
                || source_project_rows != 1
                || target_project_rows != 1
            {
                return Err(ProjectStoreError::HandoffSourceChanged);
            }
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &source_assignment_id,
                next_assignment_version,
                "worker_handoff_started",
                "user",
                now,
            )?;
            insert_lifecycle_event(
                &transaction,
                "worker",
                &command.worker_id,
                next_worker_version,
                "runtime_handoff_requested",
                "user",
                now,
            )?;

            let mut context_assignment = source_record.assignment;
            context_assignment.lifecycle = AssignmentLifecycle::HandingOff;
            context_assignment.version = next_assignment_version;
            context_assignment.updated_at_unix_ms = now;
            context_assignment.attempt.lifecycle = AttemptLifecycle::HandingOff;
            context_assignment.attempt.version = next_attempt_version;
            context_assignment.attempt.updated_at_unix_ms = now;
            context_assignment.worker.version = next_worker_version;
            context_assignment.worker.updated_at_unix_ms = now;
            transaction.commit()?;
            Ok(BeginWorkerHandoff::Started(Box::new(
                WorkerHandoffContext {
                    command,
                    source_project,
                    target_project,
                    source_assignment: context_assignment,
                    profile,
                },
            )))
        })
        .await
    }

    async fn claim_worker_handoff_runtime(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime = runtime.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_worker_handoff_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            if let Some(claimed) = command.target_runtime {
                if claimed.matches_runtime(&runtime) {
                    return Ok(());
                }
                return Err(ProjectStoreError::RuntimeHandoffClaimConflict);
            }
            let target = select_project(&transaction, &command.target_project_id)?;
            if runtime.adapter != target.runtime.adapter
                || runtime.session != target.runtime.session
                || runtime.workspace_id != target.runtime.workspace_id
            {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }
            if command.source_runtime.matches_runtime(&runtime) {
                return Err(ProjectStoreError::RuntimeHandoffClaimConflict);
            }
            ensure_worker_binding_available_for(&transaction, &command.worker_id, &runtime)?;
            let now = unix_time_ms()?;
            let claim_rows = transaction
                .execute(
                    "UPDATE worker_handoff_commands
                        SET target_runtime_adapter = ?1,
                            target_runtime_session = ?2,
                            target_runtime_workspace_id = ?3,
                            target_terminal_id = ?4, target_tab_id = ?5,
                            target_pane_id = ?6,
                            target_provider_session_source = ?7,
                            target_provider_session_provider = ?8,
                            target_provider_session_kind = ?9,
                            target_provider_session_value = ?10,
                            target_runtime_claimed_at_unix_ms = ?11
                      WHERE command_id = ?12
                        AND target_terminal_id IS NULL",
                    params![
                        runtime.adapter,
                        runtime.session,
                        runtime.workspace_id,
                        runtime.terminal_id,
                        runtime.tab_id,
                        runtime.pane_id,
                        runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.source.as_str()),
                        runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.provider.as_str()),
                        runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.kind.as_str()),
                        runtime
                            .provider_session
                            .as_ref()
                            .map(|session| session.value.as_str()),
                        to_i64(now)?,
                        command_id,
                    ],
                )
                .map_err(map_handoff_claim_error)?;
            if claim_rows != 1 {
                return Err(ProjectStoreError::RuntimeHandoffClaimConflict);
            }
            let claimed_worker_version = command
                .expected_worker_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            insert_lifecycle_event(
                &transaction,
                "worker",
                &command.worker_id,
                claimed_worker_version,
                "handoff_runtime_claimed",
                "herdr",
                now,
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn finalize_worker_handoff(
        &self,
        command_id: &str,
        runtime: WorkerRuntimeBinding,
    ) -> Result<ConfirmedWorkerHandoff, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime = runtime.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_worker_handoff_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status == "succeeded" {
                return select_confirmed_worker_handoff(&transaction, &command_id, true);
            }
            if command.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let claimed = command
                .target_runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeHandoffClaimMissing)?;
            if !claimed.matches_runtime(&runtime) {
                return Err(ProjectStoreError::RuntimeHandoffClaimConflict);
            }
            let target_project = select_project(&transaction, &command.target_project_id)?;
            if runtime.adapter != target_project.runtime.adapter
                || runtime.session != target_project.runtime.session
                || runtime.workspace_id != target_project.runtime.workspace_id
            {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }
            ensure_runtime_snapshot_current(&transaction, &runtime)?;
            ensure_worker_binding_available_for(&transaction, &command.worker_id, &runtime)?;

            let source_record =
                select_assignment_record(&transaction, &command.source_assignment_id)?
                    .ok_or(ProjectStoreError::AssignmentNotFound)?;
            if source_record.assignment.lifecycle != AssignmentLifecycle::HandingOff
                || source_record.assignment.attempt.lifecycle != AttemptLifecycle::HandingOff
                || source_record.assignment.attempt.id != command.source_attempt_id
            {
                return Err(ProjectStoreError::HandoffSourceChanged);
            }
            let current_runtime = source_record
                .assignment
                .worker
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            if !command.source_runtime.matches_runtime(current_runtime) {
                return Err(ProjectStoreError::HandoffSourceChanged);
            }
            if command.target_role == HandoffTargetRole::Orchestrator
                && command.replaced_orchestrator_worker_id.as_deref()
                    != Some(target_project.orchestrator.id.as_str())
            {
                return Err(ProjectStoreError::TargetOrchestratorChanged);
            }

            let allocation_id = Uuid::now_v7().to_string();
            let assignment_id = Uuid::now_v7().to_string();
            let attempt_id = Uuid::now_v7().to_string();
            let now = unix_time_ms()?;
            insert_retired_runtime_binding(
                &transaction,
                &command_id,
                &command.worker_id,
                "handoff_source",
                current_runtime,
                now,
            )?;
            insert_runtime_cleanup_job(
                &transaction,
                &command_id,
                &command.worker_id,
                "handoff_source",
                current_runtime,
                now,
            )?;
            replace_worker_runtime_binding(&transaction, &command.worker_id, &runtime, now)?;
            let worker_version = source_record.assignment.worker.version;
            let next_worker_version = worker_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let worker_rows = transaction.execute(
                "UPDATE workers
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(next_worker_version)?,
                    to_i64(now)?,
                    command.worker_id,
                    to_i64(worker_version)?,
                ],
            )?;
            let source_assignment_version = source_record
                .assignment
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let source_attempt_version = source_record
                .assignment
                .attempt
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let source_assignment_rows = transaction.execute(
                "UPDATE assignments
                    SET lifecycle = 'handed_off', version = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND lifecycle = 'handing_off'",
                params![
                    to_i64(source_assignment_version)?,
                    to_i64(now)?,
                    command.source_assignment_id,
                ],
            )?;
            let source_attempt_rows = transaction.execute(
                "UPDATE assignment_attempts
                    SET lifecycle = 'handed_off', version = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND lifecycle = 'handing_off'",
                params![
                    to_i64(source_attempt_version)?,
                    to_i64(now)?,
                    command.source_attempt_id,
                ],
            )?;
            let source_allocation_rows = transaction.execute(
                "UPDATE worker_allocations
                    SET ended_at_unix_ms = ?1
                  WHERE id = ?2 AND ended_at_unix_ms IS NULL",
                params![to_i64(now)?, source_record.allocation.id],
            )?;
            if worker_rows != 1
                || source_assignment_rows != 1
                || source_attempt_rows != 1
                || source_allocation_rows != 1
            {
                return Err(ProjectStoreError::HandoffSourceChanged);
            }

            if let Some(replaced_worker_id) = command.replaced_orchestrator_worker_id.as_deref() {
                if let Some(replaced_runtime) = target_project.orchestrator.runtime.as_ref() {
                    insert_runtime_cleanup_job(
                        &transaction,
                        &command_id,
                        replaced_worker_id,
                        "replaced_orchestrator",
                        replaced_runtime,
                        now,
                    )?;
                }
                let replaced_assignment = transaction
                    .query_row(
                        "SELECT id
                           FROM assignments
                          WHERE project_id = ?1 AND worker_id = ?2
                            AND lifecycle = 'active'
                          ORDER BY created_at_unix_ms DESC, id DESC
                          LIMIT 1",
                        params![command.target_project_id, replaced_worker_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                if let Some(replaced_assignment) = replaced_assignment {
                    let replaced_assignment_rows = transaction.execute(
                        "UPDATE assignments
                            SET lifecycle = 'handed_off', version = version + 1,
                                updated_at_unix_ms = ?1
                          WHERE id = ?2 AND lifecycle = 'active'",
                        params![to_i64(now)?, replaced_assignment],
                    )?;
                    let replaced_attempt_rows = transaction.execute(
                        "UPDATE assignment_attempts
                            SET lifecycle = 'handed_off', version = version + 1,
                                updated_at_unix_ms = ?1
                          WHERE assignment_id = ?2 AND lifecycle = 'active'",
                        params![to_i64(now)?, replaced_assignment],
                    )?;
                    if replaced_assignment_rows != 1 || replaced_attempt_rows != 1 {
                        return Err(ProjectStoreError::TargetOrchestratorChanged);
                    }
                }
                let replaced_allocation_rows = transaction.execute(
                    "UPDATE worker_allocations
                        SET ended_at_unix_ms = ?1
                      WHERE worker_id = ?2 AND ended_at_unix_ms IS NULL",
                    params![to_i64(now)?, replaced_worker_id],
                )?;
                if replaced_allocation_rows != 1 {
                    return Err(ProjectStoreError::TargetOrchestratorChanged);
                }
            }

            transaction.execute(
                "INSERT INTO worker_allocations (
                    id, project_id, worker_id, mode, started_by_command_id,
                    started_at_unix_ms, ended_at_unix_ms
                 ) VALUES (?1, ?2, ?3, 'handoff', ?4, ?5, NULL)",
                params![
                    allocation_id,
                    command.target_project_id,
                    command.worker_id,
                    command_id,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO assignments (
                    id, project_id, allocation_id, worker_id, profile_id,
                    profile_version, objective, role, isolation_policy,
                    lifecycle, version, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    'project_workspace', 'active', 1, ?9, ?9
                 )",
                params![
                    assignment_id,
                    command.target_project_id,
                    allocation_id,
                    command.worker_id,
                    source_record.assignment.profile_id,
                    to_i64(source_record.assignment.profile_version)?,
                    command.objective,
                    command.role,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO assignment_attempts (
                    id, assignment_id, ordinal, lifecycle, error_message,
                    version, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, 1, 'active', NULL, 1, ?3, ?3)",
                params![attempt_id, assignment_id, to_i64(now)?],
            )?;

            let source_project_version = transaction.query_row(
                "SELECT version FROM projects WHERE id = ?1",
                [&command.source_project_id],
                |row| row.get::<_, i64>(0),
            )?;
            let source_project_version = to_u64(source_project_version)?;
            let source_project_rows = transaction.execute(
                "UPDATE projects
                    SET version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND version = ?4",
                params![
                    to_i64(
                        source_project_version
                            .checked_add(1)
                            .ok_or(ProjectStoreError::VersionOverflow)?,
                    )?,
                    to_i64(now)?,
                    command.source_project_id,
                    to_i64(source_project_version)?,
                ],
            )?;
            if source_project_rows != 1 {
                return Err(ProjectStoreError::HandoffSourceChanged);
            }
            let target_project_version = target_project.version;
            let next_target_project_version = target_project_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            if command.target_role == HandoffTargetRole::Orchestrator {
                let target_project_rows = transaction.execute(
                    "UPDATE projects
                        SET orchestrator_worker_id = ?1, version = ?2,
                            updated_at_unix_ms = ?3
                      WHERE id = ?4 AND version = ?5
                        AND orchestrator_worker_id = ?6",
                    params![
                        command.worker_id,
                        to_i64(next_target_project_version)?,
                        to_i64(now)?,
                        command.target_project_id,
                        to_i64(target_project_version)?,
                        command
                            .replaced_orchestrator_worker_id
                            .as_deref()
                            .ok_or(ProjectStoreError::TargetOrchestratorChanged)?,
                    ],
                )?;
                if target_project_rows != 1 {
                    return Err(ProjectStoreError::TargetOrchestratorChanged);
                }
            } else {
                let target_project_rows = transaction.execute(
                    "UPDATE projects
                        SET version = ?1, updated_at_unix_ms = ?2
                      WHERE id = ?3 AND version = ?4",
                    params![
                        to_i64(next_target_project_version)?,
                        to_i64(now)?,
                        command.target_project_id,
                        to_i64(target_project_version)?,
                    ],
                )?;
                if target_project_rows != 1 {
                    return Err(ProjectStoreError::HandoffSourceChanged);
                }
            }
            transaction.execute(
                "UPDATE worker_handoff_commands
                    SET result_allocation_id = ?1, result_assignment_id = ?2,
                        finished_at_unix_ms = ?3
                  WHERE command_id = ?4",
                params![allocation_id, assignment_id, to_i64(now)?, command_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &command.source_assignment_id,
                source_assignment_version,
                "worker_handed_off",
                "herdr",
                now,
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                1,
                "worker_handoff_received",
                "herdr",
                now,
            )?;
            insert_lifecycle_event(
                &transaction,
                "worker",
                &command.worker_id,
                next_worker_version,
                "runtime_binding_handed_off",
                "herdr",
                now,
            )?;
            let result = select_confirmed_worker_handoff(&transaction, &command_id, false)?;
            transaction.commit()?;
            Ok(result)
        })
        .await
    }

    async fn fail_worker_handoff(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let command = select_worker_handoff_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if command.status != "pending" {
                return Ok(());
            }
            let now = unix_time_ms()?;
            if !ambiguous {
                let source_record =
                    select_assignment_record(&transaction, &command.source_assignment_id)?
                        .ok_or(ProjectStoreError::AssignmentNotFound)?;
                if source_record.assignment.lifecycle != AssignmentLifecycle::HandingOff
                    || source_record.assignment.attempt.lifecycle != AttemptLifecycle::HandingOff
                {
                    return Err(ProjectStoreError::HandoffSourceChanged);
                }
                transaction.execute(
                    "UPDATE assignments
                        SET lifecycle = 'active', version = version + 1,
                            updated_at_unix_ms = ?1
                      WHERE id = ?2 AND lifecycle = 'handing_off'",
                    params![to_i64(now)?, command.source_assignment_id],
                )?;
                transaction.execute(
                    "UPDATE assignment_attempts
                        SET lifecycle = 'active', version = version + 1,
                            updated_at_unix_ms = ?1
                      WHERE id = ?2 AND lifecycle = 'handing_off'",
                    params![to_i64(now)?, command.source_attempt_id],
                )?;
                transaction.execute(
                    "UPDATE workers
                        SET version = version + 1, updated_at_unix_ms = ?1
                      WHERE id = ?2",
                    params![to_i64(now)?, command.worker_id],
                )?;
                transaction.execute(
                    "UPDATE projects
                        SET version = version + 1, updated_at_unix_ms = ?1
                      WHERE id IN (?2, ?3)",
                    params![
                        to_i64(now)?,
                        command.source_project_id,
                        command.target_project_id,
                    ],
                )?;
                transaction.execute(
                    "UPDATE worker_handoff_commands
                        SET target_runtime_adapter = NULL,
                            target_runtime_session = NULL,
                            target_runtime_workspace_id = NULL,
                            target_terminal_id = NULL,
                            target_tab_id = NULL,
                            target_pane_id = NULL,
                            target_provider_session_source = NULL,
                            target_provider_session_provider = NULL,
                            target_provider_session_kind = NULL,
                            target_provider_session_value = NULL,
                            target_runtime_claimed_at_unix_ms = NULL,
                            finished_at_unix_ms = ?1
                      WHERE command_id = ?2",
                    params![to_i64(now)?, command_id],
                )?;
                insert_lifecycle_event(
                    &transaction,
                    "assignment",
                    &command.source_assignment_id,
                    source_record.assignment.version + 1,
                    "worker_handoff_rolled_back",
                    "yard",
                    now,
                )?;
            }
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    message,
                    to_i64(now)?,
                    command_id,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn end_worker_session(
        &self,
        worker_id: &str,
        command: EndWorkerSession,
    ) -> Result<EndedWorkerSession, ProjectStoreError> {
        let worker_id = required_id(worker_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT wsec.worker_id, wsec.expected_worker_version,
                            wsec.expected_runtime_version, ca.actor, ca.status
                       FROM worker_session_end_commands wsec
                       JOIN command_acknowledgements ca
                         ON ca.id = wsec.command_id
                      WHERE wsec.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row_u64(row, 1)?,
                            row_optional_u64(row, 2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((
                existing_worker_id,
                expected_worker_version,
                expected_runtime_version,
                actor,
                status,
            )) = existing
            {
                if existing_worker_id != worker_id
                    || expected_worker_version != command.expected_worker_version
                    || expected_runtime_version != command.expected_runtime_version
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                if status != "succeeded" {
                    return Err(ProjectStoreError::CommandInProgress);
                }
                let candidate = select_worker_candidate(&transaction, &worker_id)?
                    .ok_or(ProjectStoreError::WorkerNotFound)?;
                let cleanup_pending = runtime_cleanup_pending(&transaction, &command.command_id)?;
                transaction.commit()?;
                return Ok(EndedWorkerSession {
                    command_id: command.command_id,
                    worker: candidate.worker,
                    cleanup_pending,
                    replayed: true,
                });
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let candidate = select_worker_candidate(&transaction, &worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            if candidate.worker.version != command.expected_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: candidate.worker.version,
                });
            }
            match candidate.availability {
                WorkerAvailability::YardOrchestrator => {
                    return Err(ProjectStoreError::YardOrchestratorSessionEndForbidden);
                }
                WorkerAvailability::CoordinationNode => {
                    return Err(ProjectStoreError::CoordinationNodeSessionEndForbidden);
                }
                WorkerAvailability::Orchestrator => {
                    return Err(ProjectStoreError::OrchestratorSessionEndForbidden);
                }
                WorkerAvailability::Assigned => {
                    return Err(ProjectStoreError::WorkerHasActiveAllocation);
                }
                WorkerAvailability::Ended => {
                    return Err(ProjectStoreError::WorkerAlreadyEnded);
                }
                WorkerAvailability::UnassignedLive
                | WorkerAvailability::Resumable
                | WorkerAvailability::Unavailable
                | WorkerAvailability::Ambiguous => {}
            }

            let current_runtime_version = candidate
                .worker
                .runtime
                .as_ref()
                .map(|runtime| runtime.version);
            if current_runtime_version != command.expected_runtime_version {
                return Err(ProjectStoreError::WorkerRuntimeVersionConflict {
                    current_version: current_runtime_version,
                });
            }

            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'worker_session_end', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO worker_session_end_commands (
                    command_id, worker_id, expected_worker_version,
                    expected_runtime_version, finished_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    command.command_id,
                    worker_id,
                    to_i64(command.expected_worker_version)?,
                    command.expected_runtime_version.map(to_i64).transpose()?,
                    to_i64(now)?,
                ],
            )?;

            let cleanup_pending = if let Some(runtime) = candidate.worker.runtime.as_ref() {
                insert_retired_runtime_binding(
                    &transaction,
                    &command.command_id,
                    &worker_id,
                    "worker_session_end",
                    runtime,
                    now,
                )?;
                insert_runtime_cleanup_job(
                    &transaction,
                    &command.command_id,
                    &worker_id,
                    "worker_session_end",
                    runtime,
                    now,
                )?;
                let rows = transaction.execute(
                    "DELETE FROM worker_runtime_bindings
                      WHERE worker_id = ?1 AND version = ?2",
                    params![worker_id, to_i64(runtime.version)?],
                )?;
                if rows != 1 {
                    return Err(ProjectStoreError::WorkerRuntimeVersionConflict {
                        current_version: None,
                    });
                }
                true
            } else {
                false
            };

            let next_worker_version = candidate
                .worker
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let rows = transaction.execute(
                "UPDATE workers
                    SET ended_at_unix_ms = ?1, version = ?2,
                        updated_at_unix_ms = ?1
                  WHERE id = ?3 AND version = ?4
                    AND ended_at_unix_ms IS NULL",
                params![
                    to_i64(now)?,
                    to_i64(next_worker_version)?,
                    worker_id,
                    to_i64(candidate.worker.version)?,
                ],
            )?;
            if rows != 1 {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: candidate.worker.version,
                });
            }
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command.command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "worker",
                &worker_id,
                next_worker_version,
                "worker_session_ended",
                "user",
                now,
            )?;
            let worker = select_worker_candidate(&transaction, &worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?
                .worker;
            transaction.commit()?;
            Ok(EndedWorkerSession {
                command_id: command.command_id,
                worker,
                cleanup_pending,
                replayed: false,
            })
        })
        .await
    }

    async fn list_pending_runtime_cleanups(
        &self,
        command_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PendingRuntimeCleanup>, ProjectStoreError> {
        let command_id = command_id.map(required_command_id).transpose()?;
        let limit = i64::try_from(limit.clamp(1, 100))?;
        self.run(move |connection| {
            let now = to_i64(unix_time_ms()?)?;
            let sql = if command_id.is_some() {
                "SELECT cleanup.id, cleanup.command_id, cleanup.worker_id,
                        cleanup.reason, cleanup.adapter,
                        cleanup.runtime_session, cleanup.runtime_workspace_id,
                        cleanup.terminal_id, cleanup.tab_id, cleanup.pane_id,
                        cleanup.owns_tab, cleanup.attempts, cleanup.last_error,
                        retired.provider_session_source,
                        retired.provider_session_provider,
                        retired.provider_session_kind,
                        retired.provider_session_value
                   FROM runtime_cleanup_jobs cleanup
                   LEFT JOIN retired_runtime_bindings retired
                     ON retired.command_id = cleanup.command_id
                    AND retired.worker_id = cleanup.worker_id
                  WHERE cleanup.status = 'pending' AND cleanup.command_id = ?1
                    AND cleanup.next_attempt_at_unix_ms <= ?2
                  ORDER BY cleanup.created_at_unix_ms, cleanup.id
                  LIMIT ?3"
            } else {
                "SELECT cleanup.id, cleanup.command_id, cleanup.worker_id,
                        cleanup.reason, cleanup.adapter,
                        cleanup.runtime_session, cleanup.runtime_workspace_id,
                        cleanup.terminal_id, cleanup.tab_id, cleanup.pane_id,
                        cleanup.owns_tab, cleanup.attempts, cleanup.last_error,
                        retired.provider_session_source,
                        retired.provider_session_provider,
                        retired.provider_session_kind,
                        retired.provider_session_value
                   FROM runtime_cleanup_jobs cleanup
                   LEFT JOIN retired_runtime_bindings retired
                     ON retired.command_id = cleanup.command_id
                    AND retired.worker_id = cleanup.worker_id
                  WHERE cleanup.status = 'pending'
                    AND cleanup.next_attempt_at_unix_ms <= ?1
                  ORDER BY cleanup.created_at_unix_ms, cleanup.id
                  LIMIT ?2"
            };
            let mut statement = connection.prepare(sql)?;
            let map_row = |row: &rusqlite::Row<'_>| {
                let provider_session_source = row.get::<_, Option<String>>(13)?;
                Ok(PendingRuntimeCleanup {
                    id: row.get(0)?,
                    command_id: row.get(1)?,
                    worker_id: row.get(2)?,
                    reason: row.get(3)?,
                    adapter: row.get(4)?,
                    session: row.get(5)?,
                    workspace_id: row.get(6)?,
                    terminal_id: row.get(7)?,
                    tab_id: row.get(8)?,
                    pane_id: row.get(9)?,
                    provider_session: provider_session_source
                        .map(|source| -> rusqlite::Result<ProviderSessionRef> {
                            Ok(ProviderSessionRef {
                                source,
                                provider: row.get(14)?,
                                kind: row.get(15)?,
                                value: row.get(16)?,
                            })
                        })
                        .transpose()?,
                    owns_tab: row.get(10)?,
                    attempts: row.get(11)?,
                    last_error: row.get(12)?,
                })
            };
            let jobs = if let Some(command_id) = command_id {
                statement
                    .query_map(params![command_id, now, limit], map_row)?
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                statement
                    .query_map(params![now, limit], map_row)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(jobs)
        })
        .await
    }

    async fn succeed_runtime_cleanup(&self, cleanup_id: &str) -> Result<(), ProjectStoreError> {
        let cleanup_id = required_id(cleanup_id)?;
        self.run(move |connection| {
            let now = to_i64(unix_time_ms()?)?;
            let rows = connection.execute(
                "UPDATE runtime_cleanup_jobs
                    SET status = 'succeeded', attempts = attempts + 1,
                        last_error = NULL, updated_at_unix_ms = ?1,
                        completed_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![now, cleanup_id],
            )?;
            if rows == 0 {
                let exists = connection.query_row(
                    "SELECT EXISTS (
                        SELECT 1 FROM runtime_cleanup_jobs
                         WHERE id = ?1 AND status = 'succeeded'
                     )",
                    [&cleanup_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !exists {
                    return Err(ProjectStoreError::RuntimeCleanupNotFound);
                }
            }
            Ok(())
        })
        .await
    }

    async fn fail_runtime_cleanup(
        &self,
        cleanup_id: &str,
        message: &str,
    ) -> Result<(), ProjectStoreError> {
        let cleanup_id = required_id(cleanup_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let now = unix_time_ms()?;
            let next_attempt = now.saturating_add(1_000);
            let rows = connection.execute(
                "UPDATE runtime_cleanup_jobs
                    SET attempts = attempts + 1, last_error = ?1,
                        next_attempt_at_unix_ms = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![message, to_i64(next_attempt)?, to_i64(now)?, cleanup_id,],
            )?;
            if rows != 1 {
                let succeeded = connection.query_row(
                    "SELECT EXISTS (
                        SELECT 1 FROM runtime_cleanup_jobs
                         WHERE id = ?1 AND status = 'succeeded'
                     )",
                    [&cleanup_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !succeeded {
                    return Err(ProjectStoreError::RuntimeCleanupNotFound);
                }
            }
            Ok(())
        })
        .await
    }

    async fn list_project_assignments(
        &self,
        project_id: &str,
    ) -> Result<Assignments, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        self.run(move |connection| {
            let exists = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)",
                [&project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(ProjectStoreError::ProjectNotFound);
            }
            let mut statement = connection.prepare(&format!(
                "{ASSIGNMENT_SELECT}
                 WHERE a.project_id = ?1
                 ORDER BY a.created_at_unix_ms, a.id"
            ))?;
            let mut assignments = statement
                .query_map([project_id], assignment_record_from_row)?
                .map(|record| record.map(|record| record.assignment))
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            for assignment in &mut assignments {
                assignment.completion_receipt =
                    select_completion_receipt(connection, &assignment.id)?;
            }
            Ok(Assignments { assignments })
        })
        .await
    }

    async fn register_artifact(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
        registration: ArtifactRegistration,
    ) -> Result<Artifact, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let assignment_id = required_assignment_id(assignment_id)?;
        let artifact_id = required_id(artifact_id)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_artifact(&transaction, &artifact_id)? {
                if artifact_matches_registration(
                    &existing,
                    &project_id,
                    &assignment_id,
                    &registration,
                ) {
                    return Ok(existing);
                }
                return Err(ProjectStoreError::ArtifactIdConflict);
            }

            let record = select_assignment_record(&transaction, &assignment_id)?
                .ok_or(ProjectStoreError::AssignmentNotFound)?;
            if record.assignment.project_id != project_id {
                return Err(ProjectStoreError::AssignmentNotFound);
            }
            if record.assignment.lifecycle != AssignmentLifecycle::Active {
                return Err(ProjectStoreError::AssignmentNotActive);
            }
            if record.assignment.version != registration.expected_assignment_version {
                return Err(ProjectStoreError::AssignmentVersionConflict {
                    current_version: record.assignment.version,
                });
            }
            if record.assignment.attempt.id != registration.attempt_id {
                return Err(ProjectStoreError::AttemptNotCurrent {
                    current_attempt_id: record.assignment.attempt.id,
                });
            }
            if record.assignment.attempt.lifecycle != AttemptLifecycle::Active {
                return Err(ProjectStoreError::AttemptNotActive);
            }
            if record.assignment.attempt.version != registration.expected_attempt_version {
                return Err(ProjectStoreError::AttemptVersionConflict {
                    current_version: record.assignment.attempt.version,
                });
            }

            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO artifacts (
                    id, project_id, assignment_id, attempt_id, worker_id,
                    kind, media_type, display_name, byte_size, sha256, source,
                    created_by, created_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'upload',
                    ?11, ?12
                 )",
                params![
                    artifact_id,
                    project_id,
                    assignment_id,
                    registration.attempt_id,
                    record.assignment.worker.id,
                    artifact_kind_value(registration.kind),
                    registration.kind.media_type(),
                    registration.display_name,
                    to_i64(registration.byte_size)?,
                    registration.sha256,
                    registration.actor,
                    to_i64(now)?,
                ],
            )?;
            let artifact = select_artifact(&transaction, &artifact_id)?
                .ok_or(ProjectStoreError::ArtifactNotFound)?;
            transaction.commit()?;
            Ok(artifact)
        })
        .await
    }

    async fn get_artifact(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
    ) -> Result<Artifact, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let assignment_id = required_assignment_id(assignment_id)?;
        let artifact_id = required_id(artifact_id)?;
        self.run(move |connection| {
            let artifact = select_artifact(connection, &artifact_id)?
                .ok_or(ProjectStoreError::ArtifactNotFound)?;
            if artifact.project_id != project_id || artifact.assignment_id != assignment_id {
                return Err(ProjectStoreError::ArtifactNotFound);
            }
            Ok(artifact)
        })
        .await
    }

    async fn record_completion_receipt(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: RecordCompletionReceipt,
    ) -> Result<RecordedCompletionReceipt, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let assignment_id = required_assignment_id(assignment_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_completion_command(&transaction, &command.command_id)? {
                if !existing.matches(&project_id, &assignment_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => {
                        select_recorded_completion(&transaction, &command.command_id, true)
                    }
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "completion command failed".to_owned()),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let record = select_assignment_record(&transaction, &assignment_id)?
                .ok_or(ProjectStoreError::AssignmentNotFound)?;
            if record.assignment.project_id != project_id {
                return Err(ProjectStoreError::AssignmentNotFound);
            }
            if record.assignment.lifecycle != AssignmentLifecycle::Active {
                return Err(ProjectStoreError::AssignmentNotActive);
            }
            if record.assignment.version != command.expected_assignment_version {
                return Err(ProjectStoreError::AssignmentVersionConflict {
                    current_version: record.assignment.version,
                });
            }
            if record.assignment.attempt.id != command.attempt_id {
                return Err(ProjectStoreError::AttemptNotCurrent {
                    current_attempt_id: record.assignment.attempt.id,
                });
            }
            if record.assignment.attempt.lifecycle != AttemptLifecycle::Active {
                return Err(ProjectStoreError::AttemptNotActive);
            }
            if record.assignment.attempt.version != command.expected_attempt_version {
                return Err(ProjectStoreError::AttemptVersionConflict {
                    current_version: record.assignment.attempt.version,
                });
            }
            let prompt_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM assignment_prompt_commands apc
                      JOIN command_acknowledgements ca ON ca.id = apc.command_id
                     WHERE apc.assignment_id = ?1
                       AND ca.status = 'pending'
                 )",
                [&assignment_id],
                |row| row.get::<_, bool>(0),
            )?;
            if prompt_pending {
                return Err(ProjectStoreError::AssignmentInterventionInProgress);
            }

            let next_assignment_version = record
                .assignment
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_attempt_version = record
                .assignment
                .attempt
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let receipt_id = Uuid::now_v7().to_string();
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'completion_receipt', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?,],
            )?;
            transaction.execute(
                "INSERT INTO completion_receipt_commands (
                    command_id, project_id, assignment_id, attempt_id,
                    expected_assignment_version, expected_attempt_version,
                    outcome, summary
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'completed', ?7)",
                params![
                    command.command_id,
                    project_id,
                    assignment_id,
                    command.attempt_id,
                    to_i64(command.expected_assignment_version)?,
                    to_i64(command.expected_attempt_version)?,
                    command.summary,
                ],
            )?;
            transaction.execute(
                "INSERT INTO completion_receipts (
                    id, command_id, assignment_id, attempt_id, outcome,
                    summary, actor, created_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?7)",
                params![
                    receipt_id,
                    command.command_id,
                    assignment_id,
                    command.attempt_id,
                    command.summary,
                    command.actor,
                    to_i64(now)?,
                ],
            )?;
            insert_receipt_values(
                &transaction,
                "completion_receipt_artifacts",
                &receipt_id,
                &command.artifact_refs,
            )?;
            insert_receipt_artifacts(
                &transaction,
                &receipt_id,
                &project_id,
                &assignment_id,
                &command.attempt_id,
                &command.artifact_ids,
            )?;
            insert_receipt_values(
                &transaction,
                "completion_receipt_evidence",
                &receipt_id,
                &command.evidence_refs,
            )?;
            insert_receipt_values(
                &transaction,
                "completion_receipt_blockers",
                &receipt_id,
                &command.unresolved_blockers,
            )?;
            let assignment_rows = transaction.execute(
                "UPDATE assignments
                    SET lifecycle = 'completed', version = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND lifecycle = 'active' AND version = ?4",
                params![
                    to_i64(next_assignment_version)?,
                    to_i64(now)?,
                    assignment_id,
                    to_i64(command.expected_assignment_version)?,
                ],
            )?;
            if assignment_rows != 1 {
                return Err(ProjectStoreError::AssignmentNotActive);
            }
            let attempt_rows = transaction.execute(
                "UPDATE assignment_attempts
                    SET lifecycle = 'completed', version = ?1,
                        updated_at_unix_ms = ?2
                  WHERE id = ?3 AND assignment_id = ?4
                    AND lifecycle = 'active' AND version = ?5",
                params![
                    to_i64(next_attempt_version)?,
                    to_i64(now)?,
                    command.attempt_id,
                    assignment_id,
                    to_i64(command.expected_attempt_version)?,
                ],
            )?;
            if attempt_rows != 1 {
                return Err(ProjectStoreError::AttemptNotActive);
            }
            transaction.execute(
                "UPDATE worker_allocations
                    SET ended_at_unix_ms = ?1
                  WHERE id = ?2 AND ended_at_unix_ms IS NULL",
                params![to_i64(now)?, record.allocation.id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command.command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                next_assignment_version,
                "completion_receipt_recorded",
                "user",
                now,
            )?;

            let recorded = select_recorded_completion(&transaction, &command.command_id, false)?;
            transaction.commit()?;
            Ok(recorded)
        })
        .await
    }

    async fn begin_assignment_prompt(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: SendAssignmentPrompt,
    ) -> Result<BeginAssignmentPrompt, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let assignment_id = required_assignment_id(assignment_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_assignment_prompt_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&project_id, &assignment_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => select_prompt_acknowledgement(&transaction, &command.command_id)
                        .map(BeginAssignmentPrompt::Replayed),
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "prompt command failed".to_owned()),
                    )),
                    "ambiguous" | "pending" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "prompt submission may have reached Herdr; it was not retried"
                                .to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let record = select_assignment_record(&transaction, &assignment_id)?
                .ok_or(ProjectStoreError::AssignmentNotFound)?;
            validate_prompt_assignment(&record.assignment, &project_id, &command)?;
            let runtime = record
                .assignment
                .worker
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'assignment_prompt', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO assignment_prompt_commands (
                    command_id, project_id, assignment_id, attempt_id,
                    expected_assignment_version, expected_attempt_version,
                    prompt_text, runtime_session, runtime_pane_id,
                    result_runtime_status, submitted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL)",
                params![
                    command.command_id,
                    project_id,
                    assignment_id,
                    command.attempt_id,
                    to_i64(command.expected_assignment_version)?,
                    to_i64(command.expected_attempt_version)?,
                    command.text,
                    runtime.session,
                    runtime.pane_id,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginAssignmentPrompt::Started {
                command,
                assignment: Box::new(record.assignment),
            })
        })
        .await
    }

    async fn succeed_assignment_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<yard_domain::PromptAcknowledgement, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime_status = required_runtime_status(runtime_status)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_assignment_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status == "succeeded" {
                return select_prompt_acknowledgement(&transaction, &command_id);
            }
            if existing.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE assignment_prompt_commands
                    SET result_runtime_status = ?1, submitted_at_unix_ms = ?2
                  WHERE command_id = ?3
                    AND result_runtime_status IS NULL
                    AND submitted_at_unix_ms IS NULL",
                params![runtime_status, to_i64(now)?, command_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &existing.assignment_id,
                existing.expected_assignment_version,
                "assignment_prompt_submitted",
                "user",
                now,
            )?;
            let acknowledgement = select_prompt_acknowledgement(&transaction, &command_id)?;
            transaction.commit()?;
            Ok(acknowledgement)
        })
        .await
    }

    async fn fail_assignment_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_assignment_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status != "pending" {
                return Ok(());
            }
            let status = if ambiguous { "ambiguous" } else { "failed" };
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![status, message, to_i64(unix_time_ms()?)?, command_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn begin_orchestrator_prompt(
        &self,
        project_id: &str,
        command: SendOrchestratorPrompt,
    ) -> Result<BeginOrchestratorPrompt, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_orchestrator_prompt_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&project_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => select_orchestrator_prompt_acknowledgement(
                        &transaction,
                        &command.command_id,
                    )
                    .map(BeginOrchestratorPrompt::Replayed),
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "orchestrator prompt command failed".to_owned()),
                    )),
                    "ambiguous" | "pending" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "orchestrator prompt may have reached Herdr; it was not retried"
                                .to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let handoff_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM worker_handoff_commands whc
                      JOIN command_acknowledgements ca ON ca.id = whc.command_id
                     WHERE whc.target_project_id = ?1
                       AND whc.target_role = 'orchestrator'
                       AND ca.status = 'pending'
                 )",
                [&project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if handoff_pending {
                return Err(ProjectStoreError::OrchestratorInterventionInProgress);
            }
            let route_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM yard_orchestrator_route_commands yorc
                      JOIN command_acknowledgements ca
                        ON ca.id = yorc.command_id
                     WHERE yorc.target_project_id = ?1
                       AND ca.status = 'pending'
                 )",
                [&project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if route_pending {
                return Err(ProjectStoreError::OrchestratorInterventionInProgress);
            }
            let project = select_project(&transaction, &project_id)?;
            if project.version != command.expected_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: project.version,
                });
            }
            if project.orchestrator.id != command.orchestrator_worker_id {
                return Err(ProjectStoreError::OrchestratorNotCurrent {
                    current_worker_id: project.orchestrator.id,
                });
            }
            let runtime = project
                .orchestrator
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'orchestrator_prompt', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO orchestrator_prompt_commands (
                    command_id, project_id, orchestrator_worker_id,
                    expected_project_version,
                    prompt_text, runtime_session, runtime_pane_id,
                    result_runtime_status, submitted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
                params![
                    command.command_id,
                    project_id,
                    project.orchestrator.id,
                    to_i64(command.expected_project_version)?,
                    command.text,
                    runtime.session,
                    runtime.pane_id,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginOrchestratorPrompt::Started {
                command,
                project: Box::new(project),
            })
        })
        .await
    }

    async fn succeed_orchestrator_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<OrchestratorPromptAcknowledgement, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime_status = required_runtime_status(runtime_status)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_orchestrator_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status == "succeeded" {
                return select_orchestrator_prompt_acknowledgement(&transaction, &command_id);
            }
            if existing.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE orchestrator_prompt_commands
                    SET result_runtime_status = ?1, submitted_at_unix_ms = ?2
                  WHERE command_id = ?3
                    AND result_runtime_status IS NULL
                    AND submitted_at_unix_ms IS NULL",
                params![runtime_status, to_i64(now)?, command_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project",
                &existing.project_id,
                existing.expected_project_version,
                "orchestrator_prompt_submitted",
                "user",
                now,
            )?;
            let acknowledgement =
                select_orchestrator_prompt_acknowledgement(&transaction, &command_id)?;
            transaction.commit()?;
            Ok(acknowledgement)
        })
        .await
    }

    async fn fail_orchestrator_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_orchestrator_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status != "pending" {
                return Ok(());
            }
            let status = if ambiguous { "ambiguous" } else { "failed" };
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![status, message, to_i64(unix_time_ms()?)?, command_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn latest_delivered_project_orchestrator_command_id(
        &self,
        project_id: &str,
    ) -> Result<Option<String>, ProjectStoreError> {
        let project_id = required_id(project_id)?;
        self.run(move |connection| {
            connection
                .query_row(
                    "SELECT command_id
                       FROM (
                           SELECT opc.command_id,
                                  opc.submitted_at_unix_ms AS submitted_at_unix_ms,
                                  1 AS source_precedence
                             FROM orchestrator_prompt_commands opc
                             JOIN command_acknowledgements ca
                               ON ca.id = opc.command_id
                             JOIN projects p
                               ON p.id = opc.project_id
                              AND p.orchestrator_worker_id =
                                  opc.orchestrator_worker_id
                            WHERE opc.project_id = ?1
                              AND ca.status = 'succeeded'
                              AND opc.result_runtime_status IS NOT NULL
                              AND opc.submitted_at_unix_ms IS NOT NULL
                           UNION ALL
                           SELECT yorc.command_id,
                                  yorc.submitted_at_unix_ms,
                                  0 AS source_precedence
                             FROM yard_orchestrator_route_commands yorc
                             JOIN command_acknowledgements ca
                               ON ca.id = yorc.command_id
                             JOIN projects p
                               ON p.id = yorc.target_project_id
                              AND p.orchestrator_worker_id =
                                  yorc.target_orchestrator_worker_id
                            WHERE yorc.target_project_id = ?1
                              AND ca.status = 'succeeded'
                              AND yorc.result_runtime_status IS NOT NULL
                              AND yorc.submitted_at_unix_ms IS NOT NULL
                       )
                      ORDER BY submitted_at_unix_ms DESC,
                               source_precedence DESC,
                               command_id DESC
                      LIMIT 1",
                    [&project_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
        .await
    }

    async fn begin_yard_orchestrator_prompt(
        &self,
        command: SendYardOrchestratorPrompt,
    ) -> Result<BeginYardOrchestratorPrompt, ProjectStoreError> {
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_yard_orchestrator_prompt_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => select_yard_orchestrator_prompt_acknowledgement(
                        &transaction,
                        &command.command_id,
                    )
                    .map(BeginYardOrchestratorPrompt::Replayed),
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing.error_message.unwrap_or_else(|| {
                            "Yard orchestrator prompt command failed".to_owned()
                        }),
                    )),
                    "ambiguous" | "pending" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_else(|| {
                            "Yard orchestrator prompt may have reached Herdr; it was not retried"
                                .to_owned()
                        }),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let orchestrator = select_yard_orchestrator(&transaction)?;
            if orchestrator.version != command.expected_orchestrator_version {
                return Err(ProjectStoreError::YardOrchestratorVersionConflict {
                    current_version: orchestrator.version,
                });
            }
            let worker = orchestrator
                .worker
                .as_ref()
                .ok_or(ProjectStoreError::YardOrchestratorNotConfigured)?;
            if worker.id != command.orchestrator_worker_id {
                return Err(ProjectStoreError::YardOrchestratorNotCurrent {
                    current_worker_id: worker.id.clone(),
                });
            }
            let runtime = worker
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'yard_orchestrator_prompt', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO yard_orchestrator_prompt_commands (
                    command_id, orchestrator_worker_id,
                    expected_orchestrator_version, prompt_text,
                    runtime_session, runtime_pane_id,
                    result_runtime_status, submitted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)",
                params![
                    command.command_id,
                    worker.id,
                    to_i64(command.expected_orchestrator_version)?,
                    command.text,
                    runtime.session,
                    runtime.pane_id,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginYardOrchestratorPrompt::Started {
                command,
                orchestrator: Box::new(orchestrator),
            })
        })
        .await
    }

    async fn succeed_yard_orchestrator_prompt(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<YardOrchestratorPromptAcknowledgement, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime_status = required_runtime_status(runtime_status)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_yard_orchestrator_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status == "succeeded" {
                return select_yard_orchestrator_prompt_acknowledgement(&transaction, &command_id);
            }
            if existing.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE yard_orchestrator_prompt_commands
                    SET result_runtime_status = ?1, submitted_at_unix_ms = ?2
                  WHERE command_id = ?3
                    AND result_runtime_status IS NULL
                    AND submitted_at_unix_ms IS NULL",
                params![runtime_status, to_i64(now)?, command_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "yard_orchestrator",
                "yard",
                existing.expected_orchestrator_version,
                "yard_orchestrator_prompt_submitted",
                "user",
                now,
            )?;
            let acknowledgement =
                select_yard_orchestrator_prompt_acknowledgement(&transaction, &command_id)?;
            transaction.commit()?;
            Ok(acknowledgement)
        })
        .await
    }

    async fn fail_yard_orchestrator_prompt(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_yard_orchestrator_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status != "pending" {
                return Ok(());
            }
            let status = if ambiguous { "ambiguous" } else { "failed" };
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![status, message, to_i64(unix_time_ms()?)?, command_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn latest_delivered_yard_orchestrator_command_id(
        &self,
    ) -> Result<Option<String>, ProjectStoreError> {
        self.run(|connection| {
            connection
                .query_row(
                    "SELECT yopc.command_id
                       FROM yard_orchestrator_prompt_commands yopc
                       JOIN command_acknowledgements ca
                         ON ca.id = yopc.command_id
                       JOIN yard_orchestrator yo
                         ON yo.singleton_id = 1
                        AND yo.worker_id = yopc.orchestrator_worker_id
                      WHERE ca.status = 'succeeded'
                        AND yopc.result_runtime_status IS NOT NULL
                        AND yopc.submitted_at_unix_ms IS NOT NULL
                      ORDER BY yopc.submitted_at_unix_ms DESC,
                               yopc.command_id DESC
                      LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
        .await
    }

    async fn list_yard_orchestrator_routes(
        &self,
        limit: usize,
    ) -> Result<YardOrchestratorRoutes, ProjectStoreError> {
        if limit == 0 || limit > MAX_ROUTE_LIST_LIMIT {
            return Err(ProjectStoreError::RouteListLimitInvalid {
                max: MAX_ROUTE_LIST_LIMIT,
            });
        }
        self.run(move |connection| {
            let mut statement = connection.prepare(&format!(
                "{YARD_ORCHESTRATOR_ROUTE_SELECT}
                 ORDER BY ca.created_at_unix_ms DESC, yorc.command_id DESC
                 LIMIT ?1"
            ))?;
            let routes = statement
                .query_map([i64::try_from(limit)?], yard_orchestrator_route_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(YardOrchestratorRoutes { routes })
        })
        .await
    }

    async fn begin_yard_orchestrator_route(
        &self,
        command: SendYardOrchestratorRoute,
    ) -> Result<BeginYardOrchestratorRoute, ProjectStoreError> {
        let command = command.normalize()?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) =
                select_yard_orchestrator_route(&transaction, &command.command_id)?
            {
                if !yard_orchestrator_route_matches(&existing, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status {
                    CoordinationCommandStatus::Submitted => {
                        Ok(BeginYardOrchestratorRoute::Replayed(existing))
                    }
                    CoordinationCommandStatus::Failed => {
                        Err(ProjectStoreError::CommandPreviouslyFailed(
                            existing.error_message.unwrap_or_else(|| {
                                "Yard orchestrator route command failed".to_owned()
                            }),
                        ))
                    }
                    CoordinationCommandStatus::Ambiguous | CoordinationCommandStatus::Pending => {
                        Err(ProjectStoreError::CommandOutcomeAmbiguous(
                            existing.error_message.unwrap_or_else(|| {
                                "Yard orchestrator route may have reached Herdr; it was not retried"
                                    .to_owned()
                            }),
                        ))
                    }
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let orchestrator = select_yard_orchestrator(&transaction)?;
            if orchestrator.version != command.expected_orchestrator_version {
                return Err(ProjectStoreError::YardOrchestratorVersionConflict {
                    current_version: orchestrator.version,
                });
            }
            let orchestrator_worker = orchestrator
                .worker
                .as_ref()
                .ok_or(ProjectStoreError::YardOrchestratorNotConfigured)?;
            if orchestrator_worker.id != command.orchestrator_worker_id {
                return Err(ProjectStoreError::YardOrchestratorNotCurrent {
                    current_worker_id: orchestrator_worker.id.clone(),
                });
            }
            if orchestrator_worker.runtime.is_none() {
                return Err(ProjectStoreError::RuntimeBindingMissing);
            }

            let handoff_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM worker_handoff_commands whc
                      JOIN command_acknowledgements ca ON ca.id = whc.command_id
                     WHERE whc.target_project_id = ?1
                       AND whc.target_role = 'orchestrator'
                       AND ca.status = 'pending'
                 )",
                [&command.target_project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if handoff_pending {
                return Err(ProjectStoreError::OrchestratorInterventionInProgress);
            }
            let target_intervention_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM orchestrator_prompt_commands opc
                      JOIN command_acknowledgements ca ON ca.id = opc.command_id
                     WHERE opc.project_id = ?1 AND ca.status = 'pending'
                    UNION ALL
                    SELECT 1
                      FROM yard_orchestrator_route_commands yorc
                      JOIN command_acknowledgements ca
                        ON ca.id = yorc.command_id
                     WHERE yorc.target_project_id = ?1
                       AND ca.status = 'pending'
                 )",
                [&command.target_project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if target_intervention_pending {
                return Err(ProjectStoreError::OrchestratorInterventionInProgress);
            }

            let target_project = select_project(&transaction, &command.target_project_id)?;
            if target_project.version != command.expected_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: target_project.version,
                });
            }
            if target_project.orchestrator.id != command.target_orchestrator_worker_id {
                return Err(ProjectStoreError::OrchestratorNotCurrent {
                    current_worker_id: target_project.orchestrator.id,
                });
            }
            let target_runtime = target_project
                .orchestrator
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'yard_orchestrator_route', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO yard_orchestrator_route_commands (
                    command_id, orchestrator_worker_id,
                    expected_orchestrator_version, target_project_id,
                    target_orchestrator_worker_id, expected_project_version,
                    prompt_text, runtime_session, runtime_pane_id,
                    result_runtime_status, submitted_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL
                 )",
                params![
                    command.command_id,
                    command.orchestrator_worker_id,
                    to_i64(command.expected_orchestrator_version)?,
                    command.target_project_id,
                    command.target_orchestrator_worker_id,
                    to_i64(command.expected_project_version)?,
                    command.text,
                    target_runtime.session,
                    target_runtime.pane_id,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginYardOrchestratorRoute::Started {
                command,
                orchestrator: Box::new(orchestrator),
                target_project: Box::new(target_project),
            })
        })
        .await
    }

    async fn succeed_yard_orchestrator_route(
        &self,
        command_id: &str,
        runtime_status: &str,
    ) -> Result<YardOrchestratorRoute, ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let runtime_status = required_runtime_status(runtime_status)?;
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_yard_orchestrator_route(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status == CoordinationCommandStatus::Submitted {
                return Ok(existing);
            }
            if existing.status != CoordinationCommandStatus::Pending {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE yard_orchestrator_route_commands
                    SET result_runtime_status = ?1, submitted_at_unix_ms = ?2
                  WHERE command_id = ?3
                    AND result_runtime_status IS NULL
                    AND submitted_at_unix_ms IS NULL",
                params![runtime_status, to_i64(now)?, command_id],
            )?;
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "project",
                &existing.target_project_id,
                existing.expected_project_version,
                "yard_orchestrator_route_submitted",
                "yard_orchestrator",
                now,
            )?;
            let submitted = select_yard_orchestrator_route(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            transaction.commit()?;
            Ok(submitted)
        })
        .await
    }

    async fn fail_yard_orchestrator_route(
        &self,
        command_id: &str,
        message: &str,
        ambiguous: bool,
    ) -> Result<(), ProjectStoreError> {
        let command_id = required_command_id(command_id)?;
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(ProjectStoreError::CommandFailureMessageRequired);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_yard_orchestrator_route(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status != CoordinationCommandStatus::Pending {
                return Ok(());
            }
            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    message,
                    to_i64(unix_time_ms()?)?,
                    command_id,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn reconcile_runtime_inventory(
        &self,
        inventory: RuntimeInventory,
    ) -> Result<RuntimeReconciliation, ProjectStoreError> {
        if inventory.adapter.trim().is_empty() || inventory.session.trim().is_empty() {
            return Err(ProjectStoreError::InvalidRuntimeInventory);
        }
        self.run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = reconcile_runtime_inventory(&transaction, &inventory)?;
            transaction.commit()?;
            Ok(result)
        })
        .await
    }
}

struct StoredRuntimeBinding {
    worker_id: String,
    worker_version: u64,
    expected_workspace_id: Option<String>,
    runtime: WorkerRuntimeBinding,
}

fn reconcile_runtime_inventory(
    transaction: &Transaction<'_>,
    inventory: &RuntimeInventory,
) -> Result<RuntimeReconciliation, ProjectStoreError> {
    let mut result = empty_reconciliation(inventory);
    if snapshot_is_stale(transaction, inventory)? {
        return Ok(result);
    }

    let bindings = select_runtime_bindings(transaction, &inventory.adapter, &inventory.session)?;
    let bound_terminals: HashMap<&str, &str> = bindings
        .iter()
        .map(|binding| {
            (
                binding.runtime.terminal_id.as_str(),
                binding.worker_id.as_str(),
            )
        })
        .collect();
    let mut accounted_workers = HashSet::new();
    let mut accounted_panes = HashSet::new();

    for binding in &bindings {
        let next = resolve_running_worker(
            binding,
            &bindings,
            inventory,
            &bound_terminals,
            &mut accounted_workers,
        )
        .unwrap_or_else(|| {
            resolve_exited_or_missing(
                binding,
                &bindings,
                inventory,
                &bound_terminals,
                &mut accounted_panes,
            )
        });
        update_reconciliation_counts(&mut result, &next);
        if persist_reconciled_binding(transaction, binding, &next, inventory.observed_at_unix_ms)? {
            result.updated_bindings += 1;
        }
    }

    for worker in &inventory.workers {
        if accounted_workers.contains(&worker.terminal_id)
            || accounted_panes.contains(&worker.terminal_id)
            || bound_terminals.contains_key(worker.terminal_id.as_str())
            || runtime_identity_is_reserved(transaction, inventory, worker)?
        {
            continue;
        }
        adopt_observed_worker(transaction, inventory, worker)?;
        result.adopted_workers += 1;
    }

    persist_reconciliation_watermark(transaction, inventory)?;
    Ok(result)
}

fn runtime_identity_is_reserved(
    transaction: &Transaction<'_>,
    inventory: &RuntimeInventory,
    worker: &ObservedWorker,
) -> Result<bool, ProjectStoreError> {
    let provider = worker.provider_session.as_ref();
    transaction
        .query_row(
            "SELECT
                EXISTS (
                    SELECT 1
                      FROM retired_runtime_bindings rrb
                     WHERE rrb.adapter = ?1
                       AND rrb.runtime_session = ?2
                       AND (
                           rrb.terminal_id = ?3
                           OR (
                               ?4 IS NOT NULL
                               AND rrb.provider_session_source = ?4
                               AND rrb.provider_session_provider = ?5
                               AND rrb.provider_session_kind = ?6
                               AND rrb.provider_session_value = ?7
                           )
                       )
                )
                OR EXISTS (
                    SELECT 1
                      FROM worker_handoff_commands whc
                     WHERE whc.finished_at_unix_ms IS NULL
                       AND whc.target_runtime_adapter = ?1
                       AND whc.target_runtime_session = ?2
                       AND (
                           whc.target_terminal_id = ?3
                           OR (
                               ?4 IS NOT NULL
                               AND whc.target_provider_session_source = ?4
                               AND whc.target_provider_session_provider = ?5
                               AND whc.target_provider_session_kind = ?6
                               AND whc.target_provider_session_value = ?7
                           )
                       )
                )",
            params![
                inventory.adapter,
                inventory.session,
                worker.terminal_id,
                provider.map(|session| session.source.as_str()),
                provider.map(|session| session.provider.as_str()),
                provider.map(|session| session.kind.as_str()),
                provider.map(|session| session.value.as_str()),
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(Into::into)
}

fn empty_reconciliation(inventory: &RuntimeInventory) -> RuntimeReconciliation {
    RuntimeReconciliation {
        adapter: inventory.adapter.clone(),
        session: inventory.session.clone(),
        observed_at_unix_ms: inventory.observed_at_unix_ms,
        adopted_workers: 0,
        updated_bindings: 0,
        missing_bindings: 0,
        ambiguous_bindings: 0,
        exited_processes: 0,
    }
}

fn snapshot_is_stale(
    transaction: &Transaction<'_>,
    inventory: &RuntimeInventory,
) -> Result<bool, ProjectStoreError> {
    let watermark = transaction
        .query_row(
            "SELECT observed_at_unix_ms
               FROM runtime_reconciliation_watermarks
              WHERE adapter = ?1 AND runtime_session = ?2",
            params![inventory.adapter, inventory.session],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(to_u64)
        .transpose()?;
    Ok(watermark.is_some_and(|watermark| watermark > inventory.observed_at_unix_ms))
}

fn persist_reconciliation_watermark(
    transaction: &Transaction<'_>,
    inventory: &RuntimeInventory,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO runtime_reconciliation_watermarks (
            adapter, runtime_session, observed_at_unix_ms
         ) VALUES (?1, ?2, ?3)
         ON CONFLICT(adapter, runtime_session) DO UPDATE
             SET observed_at_unix_ms = excluded.observed_at_unix_ms",
        params![
            inventory.adapter,
            inventory.session,
            to_i64(inventory.observed_at_unix_ms)?,
        ],
    )?;
    Ok(())
}

fn resolve_running_worker(
    binding: &StoredRuntimeBinding,
    bindings: &[StoredRuntimeBinding],
    inventory: &RuntimeInventory,
    bound_terminals: &HashMap<&str, &str>,
    accounted_workers: &mut HashSet<String>,
) -> Option<WorkerRuntimeBinding> {
    if let Some(worker) = inventory
        .workers
        .iter()
        .find(|worker| worker.terminal_id == binding.runtime.terminal_id)
    {
        accounted_workers.insert(worker.terminal_id.clone());
        return Some(if worker_conflicts(binding, worker) {
            ambiguous_runtime(binding)
        } else {
            runtime_from_worker(binding, worker, inventory.observed_at_unix_ms)
        });
    }

    let provider_session = binding.runtime.provider_session.as_ref()?;
    let provider_workers = inventory
        .workers
        .iter()
        .filter(|worker| worker.provider_session.as_ref() == Some(provider_session))
        .collect::<Vec<_>>();
    if provider_workers.is_empty() {
        return None;
    }
    for worker in &provider_workers {
        accounted_workers.insert(worker.terminal_id.clone());
    }
    if provider_workers.len() != 1 || !provider_binding_is_unique(bindings, provider_session) {
        return Some(ambiguous_runtime(binding));
    }

    let worker = provider_workers[0];
    let belongs_to_other_binding = bound_terminals
        .get(worker.terminal_id.as_str())
        .is_some_and(|worker_id| **worker_id != binding.worker_id);
    Some(
        if belongs_to_other_binding || workspace_conflicts(binding, &worker.workspace_id) {
            ambiguous_runtime(binding)
        } else {
            runtime_from_worker(binding, worker, inventory.observed_at_unix_ms)
        },
    )
}

fn resolve_exited_or_missing(
    binding: &StoredRuntimeBinding,
    bindings: &[StoredRuntimeBinding],
    inventory: &RuntimeInventory,
    bound_terminals: &HashMap<&str, &str>,
    accounted_panes: &mut HashSet<String>,
) -> WorkerRuntimeBinding {
    if let Some(pane) = inventory
        .panes
        .iter()
        .find(|pane| pane.terminal_id == binding.runtime.terminal_id)
    {
        accounted_panes.insert(pane.terminal_id.clone());
        return if pane_conflicts(binding, pane, bound_terminals) {
            ambiguous_runtime(binding)
        } else {
            runtime_from_exited_pane(binding, pane, inventory.observed_at_unix_ms)
        };
    }

    let Some(provider_session) = binding.runtime.provider_session.as_ref() else {
        return missing_runtime(binding);
    };
    let provider_panes = inventory
        .panes
        .iter()
        .filter(|pane| pane.provider_session.as_ref() == Some(provider_session))
        .collect::<Vec<_>>();
    for pane in &provider_panes {
        accounted_panes.insert(pane.terminal_id.clone());
    }
    if provider_panes.is_empty() {
        return missing_runtime(binding);
    }
    if provider_panes.len() != 1 || !provider_binding_is_unique(bindings, provider_session) {
        return ambiguous_runtime(binding);
    }

    let pane = provider_panes[0];
    if pane_conflicts(binding, pane, bound_terminals) {
        ambiguous_runtime(binding)
    } else {
        runtime_from_exited_pane(binding, pane, inventory.observed_at_unix_ms)
    }
}

fn provider_binding_is_unique(
    bindings: &[StoredRuntimeBinding],
    provider_session: &ProviderSessionRef,
) -> bool {
    bindings
        .iter()
        .filter(|binding| binding.runtime.provider_session.as_ref() == Some(provider_session))
        .count()
        == 1
}

fn worker_conflicts(binding: &StoredRuntimeBinding, worker: &ObservedWorker) -> bool {
    provider_conflicts(
        binding.runtime.provider_session.as_ref(),
        worker.provider_session.as_ref(),
    ) || workspace_conflicts(binding, &worker.workspace_id)
}

fn pane_conflicts(
    binding: &StoredRuntimeBinding,
    pane: &PaneObservation,
    bound_terminals: &HashMap<&str, &str>,
) -> bool {
    bound_terminals
        .get(pane.terminal_id.as_str())
        .is_some_and(|worker_id| **worker_id != binding.worker_id)
        || provider_conflicts(
            binding.runtime.provider_session.as_ref(),
            pane.provider_session.as_ref(),
        )
        || workspace_conflicts(binding, &pane.workspace_id)
}

fn select_runtime_bindings(
    connection: &Connection,
    adapter: &str,
    session: &str,
) -> Result<Vec<StoredRuntimeBinding>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT wrb.worker_id, wrb.runtime_workspace_id, wrb.terminal_id,
                wrb.tab_id, wrb.pane_id, wrb.provider_session_source,
                wrb.provider_session_provider, wrb.provider_session_kind,
                wrb.provider_session_value, wrb.owns_tab,
                wrb.observation_state, wrb.process_state, wrb.observed_status,
                wrb.state_change_sequence, wrb.runtime_revision, wrb.version,
                wrb.last_observed_at_unix_ms, w.version,
                (
                    SELECT pwb.runtime_workspace_id
                      FROM worker_allocations wa
                      JOIN project_workspace_bindings pwb
                        ON pwb.project_id = wa.project_id
                     WHERE wa.worker_id = wrb.worker_id
                       AND wa.ended_at_unix_ms IS NULL
                     LIMIT 1
                )
           FROM worker_runtime_bindings wrb
           JOIN workers w ON w.id = wrb.worker_id
          WHERE wrb.adapter = ?1 AND wrb.runtime_session = ?2
          ORDER BY wrb.worker_id",
    )?;
    statement
        .query_map(params![adapter, session], |row| {
            let provider_session_source = row.get::<_, Option<String>>(5)?;
            let provider_session = provider_session_source
                .map(|source| -> rusqlite::Result<ProviderSessionRef> {
                    Ok(ProviderSessionRef {
                        source,
                        provider: row.get(6)?,
                        kind: row.get(7)?,
                        value: row.get(8)?,
                    })
                })
                .transpose()?;
            Ok(StoredRuntimeBinding {
                worker_id: row.get(0)?,
                worker_version: row_u64(row, 17)?,
                expected_workspace_id: row.get(18)?,
                runtime: WorkerRuntimeBinding {
                    adapter: adapter.to_owned(),
                    session: session.to_owned(),
                    workspace_id: row.get(1)?,
                    terminal_id: row.get(2)?,
                    tab_id: row.get(3)?,
                    pane_id: row.get(4)?,
                    provider_session,
                    owns_tab: row.get(9)?,
                    observation_state: runtime_observation_state(row, 10)?,
                    process_state: runtime_process_state(row, 11)?,
                    status: observed_status(row, 12)?,
                    state_change_sequence: row_u64(row, 13)?,
                    revision: row_u64(row, 14)?,
                    version: row_u64(row, 15)?,
                    last_observed_at_unix_ms: row_u64(row, 16)?,
                },
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn runtime_from_worker(
    binding: &StoredRuntimeBinding,
    worker: &ObservedWorker,
    observed_at_unix_ms: u64,
) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        adapter: binding.runtime.adapter.clone(),
        session: binding.runtime.session.clone(),
        workspace_id: worker.workspace_id.clone(),
        terminal_id: worker.terminal_id.clone(),
        tab_id: Some(worker.tab_id.clone()),
        pane_id: worker.pane_id.clone(),
        provider_session: worker
            .provider_session
            .clone()
            .or_else(|| binding.runtime.provider_session.clone()),
        owns_tab: binding.runtime.owns_tab,
        observation_state: RuntimeObservationState::Observed,
        process_state: RuntimeProcessState::Running,
        status: worker.status,
        state_change_sequence: worker.state_change_sequence,
        revision: worker.revision,
        version: binding.runtime.version,
        last_observed_at_unix_ms: observed_at_unix_ms,
    }
}

fn runtime_from_exited_pane(
    binding: &StoredRuntimeBinding,
    pane: &PaneObservation,
    observed_at_unix_ms: u64,
) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        adapter: binding.runtime.adapter.clone(),
        session: binding.runtime.session.clone(),
        workspace_id: pane.workspace_id.clone(),
        terminal_id: pane.terminal_id.clone(),
        tab_id: Some(pane.tab_id.clone()),
        pane_id: pane.runtime_id.clone(),
        provider_session: pane
            .provider_session
            .clone()
            .or_else(|| binding.runtime.provider_session.clone()),
        owns_tab: binding.runtime.owns_tab,
        observation_state: RuntimeObservationState::Observed,
        process_state: RuntimeProcessState::Exited,
        status: ObservedStatus::Unknown,
        state_change_sequence: binding.runtime.state_change_sequence,
        revision: pane.revision,
        version: binding.runtime.version,
        last_observed_at_unix_ms: observed_at_unix_ms,
    }
}

fn ambiguous_runtime(binding: &StoredRuntimeBinding) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        observation_state: RuntimeObservationState::Ambiguous,
        process_state: RuntimeProcessState::Unknown,
        status: ObservedStatus::Unknown,
        ..binding.runtime.clone()
    }
}

fn missing_runtime(binding: &StoredRuntimeBinding) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        observation_state: RuntimeObservationState::Missing,
        process_state: RuntimeProcessState::Unknown,
        status: ObservedStatus::Unknown,
        ..binding.runtime.clone()
    }
}

fn provider_conflicts(
    expected: Option<&ProviderSessionRef>,
    actual: Option<&ProviderSessionRef>,
) -> bool {
    matches!((expected, actual), (Some(expected), Some(actual)) if expected != actual)
}

fn workspace_conflicts(binding: &StoredRuntimeBinding, observed_workspace_id: &str) -> bool {
    binding
        .expected_workspace_id
        .as_ref()
        .is_some_and(|expected| expected != observed_workspace_id)
}

fn update_reconciliation_counts(
    result: &mut RuntimeReconciliation,
    runtime: &WorkerRuntimeBinding,
) {
    match runtime.observation_state {
        RuntimeObservationState::Missing => result.missing_bindings += 1,
        RuntimeObservationState::Ambiguous => result.ambiguous_bindings += 1,
        RuntimeObservationState::Observed => {}
    }
    if runtime.process_state == RuntimeProcessState::Exited {
        result.exited_processes += 1;
    }
}

fn persist_reconciled_binding(
    transaction: &Transaction<'_>,
    binding: &StoredRuntimeBinding,
    runtime: &WorkerRuntimeBinding,
    reconciled_at_unix_ms: u64,
) -> Result<bool, ProjectStoreError> {
    if !runtime_binding_changed(binding, runtime) {
        if runtime.observation_state == RuntimeObservationState::Observed {
            transaction.execute(
                "UPDATE worker_runtime_bindings
                    SET last_observed_at_unix_ms = ?1
                  WHERE worker_id = ?2",
                params![to_i64(runtime.last_observed_at_unix_ms)?, binding.worker_id],
            )?;
        }
        return Ok(false);
    }

    let next_binding_version = binding
        .runtime
        .version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    let next_worker_version = binding
        .worker_version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    update_reconciled_binding_row(
        transaction,
        binding,
        runtime,
        next_binding_version,
        reconciled_at_unix_ms,
    )?;
    transaction.execute(
        "UPDATE workers
            SET version = ?1, updated_at_unix_ms = ?2
          WHERE id = ?3 AND version = ?4",
        params![
            to_i64(next_worker_version)?,
            to_i64(reconciled_at_unix_ms)?,
            binding.worker_id,
            to_i64(binding.worker_version)?,
        ],
    )?;
    insert_lifecycle_event(
        transaction,
        "worker",
        &binding.worker_id,
        next_worker_version,
        reconciliation_event_type(binding, runtime),
        "herdr",
        reconciled_at_unix_ms,
    )?;
    Ok(true)
}

fn runtime_binding_changed(binding: &StoredRuntimeBinding, runtime: &WorkerRuntimeBinding) -> bool {
    binding.runtime.workspace_id != runtime.workspace_id
        || binding.runtime.terminal_id != runtime.terminal_id
        || binding.runtime.tab_id != runtime.tab_id
        || binding.runtime.pane_id != runtime.pane_id
        || binding.runtime.provider_session != runtime.provider_session
        || binding.runtime.observation_state != runtime.observation_state
        || binding.runtime.process_state != runtime.process_state
        || binding.runtime.status != runtime.status
        || binding.runtime.state_change_sequence != runtime.state_change_sequence
        || binding.runtime.revision != runtime.revision
}

fn update_reconciled_binding_row(
    transaction: &Transaction<'_>,
    binding: &StoredRuntimeBinding,
    runtime: &WorkerRuntimeBinding,
    next_binding_version: u64,
    reconciled_at_unix_ms: u64,
) -> Result<(), ProjectStoreError> {
    let rows = transaction.execute(
        "UPDATE worker_runtime_bindings
            SET runtime_workspace_id = ?1, terminal_id = ?2, tab_id = ?3,
                pane_id = ?4, provider_session_source = ?5,
                provider_session_provider = ?6, provider_session_kind = ?7,
                provider_session_value = ?8, observation_state = ?9,
                observed_status = ?10, process_state = ?11,
                state_change_sequence = ?12, runtime_revision = ?13,
                version = ?14, last_observed_at_unix_ms = ?15,
                updated_at_unix_ms = ?16
          WHERE worker_id = ?17 AND version = ?18",
        params![
            runtime.workspace_id,
            runtime.terminal_id,
            runtime.tab_id,
            runtime.pane_id,
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.source.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.provider.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.kind.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            runtime_observation_state_value(runtime.observation_state),
            observed_status_value(runtime.status),
            runtime_process_state_value(runtime.process_state),
            to_i64(runtime.state_change_sequence)?,
            to_i64(runtime.revision)?,
            to_i64(next_binding_version)?,
            to_i64(runtime.last_observed_at_unix_ms)?,
            to_i64(reconciled_at_unix_ms)?,
            binding.worker_id,
            to_i64(binding.runtime.version)?,
        ],
    )?;
    if rows != 1 {
        return Err(ProjectStoreError::RuntimeBindingVersionConflict);
    }
    Ok(())
}

fn reconciliation_event_type(
    binding: &StoredRuntimeBinding,
    runtime: &WorkerRuntimeBinding,
) -> &'static str {
    match (
        runtime.observation_state,
        runtime.process_state,
        binding.runtime.observation_state,
    ) {
        (RuntimeObservationState::Missing, _, _) => "runtime_binding_missing",
        (RuntimeObservationState::Ambiguous, _, _) => "runtime_binding_ambiguous",
        (RuntimeObservationState::Observed, RuntimeProcessState::Exited, _) => {
            "runtime_process_exited"
        }
        (
            RuntimeObservationState::Observed,
            RuntimeProcessState::Running,
            RuntimeObservationState::Missing | RuntimeObservationState::Ambiguous,
        ) => "runtime_binding_restored",
        _ => "runtime_binding_reconciled",
    }
}

fn adopt_observed_worker(
    transaction: &Transaction<'_>,
    inventory: &RuntimeInventory,
    worker: &ObservedWorker,
) -> Result<(), ProjectStoreError> {
    let worker_id = Uuid::now_v7().to_string();
    let now = inventory.observed_at_unix_ms;
    transaction.execute(
        "INSERT INTO workers (
            id, profile_id, profile_version, desired_state, version,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES (?1, NULL, NULL, 'running', 1, ?2, ?2)",
        params![worker_id, to_i64(now)?],
    )?;
    let runtime = WorkerRuntimeBinding {
        adapter: inventory.adapter.clone(),
        session: inventory.session.clone(),
        workspace_id: worker.workspace_id.clone(),
        terminal_id: worker.terminal_id.clone(),
        tab_id: Some(worker.tab_id.clone()),
        pane_id: worker.pane_id.clone(),
        provider_session: worker.provider_session.clone(),
        owns_tab: false,
        observation_state: RuntimeObservationState::Observed,
        process_state: RuntimeProcessState::Running,
        status: worker.status,
        state_change_sequence: worker.state_change_sequence,
        revision: worker.revision,
        version: 1,
        last_observed_at_unix_ms: now,
    };
    insert_worker_runtime_binding(transaction, &worker_id, &runtime, now)?;
    insert_lifecycle_event(
        transaction,
        "worker",
        &worker_id,
        1,
        "runtime_worker_adopted",
        "herdr",
        now,
    )
}

const PROJECT_SELECT: &str = "
    SELECT p.id, p.name, p.version, p.created_at_unix_ms, p.updated_at_unix_ms,
           pwb.adapter, pwb.runtime_session, pwb.runtime_workspace_id,
           pp.canvas_x, pp.canvas_y, pp.canvas_width, pp.canvas_height,
           pp.version, pp.updated_at_unix_ms,
           w.id, w.profile_id, w.profile_version,
           CASE WHEN w.ended_at_unix_ms IS NULL
                THEN 'running' ELSE 'ended' END,
           w.version,
           w.created_at_unix_ms, w.updated_at_unix_ms,
           wrb.adapter, wrb.runtime_session, wrb.runtime_workspace_id,
           wrb.terminal_id, wrb.tab_id, wrb.pane_id,
           wrb.provider_session_source, wrb.provider_session_provider,
           wrb.provider_session_kind, wrb.provider_session_value,
           wrb.owns_tab, wrb.observation_state, wrb.process_state,
           wrb.observed_status, wrb.state_change_sequence,
           wrb.runtime_revision, wrb.version,
           wrb.last_observed_at_unix_ms
      FROM projects p
      JOIN project_workspace_bindings pwb ON pwb.project_id = p.id
      JOIN project_placements pp ON pp.project_id = p.id
      JOIN workers w ON w.id = p.orchestrator_worker_id
      LEFT JOIN worker_runtime_bindings wrb ON wrb.worker_id = w.id";

const YARD_ORCHESTRATOR_ROUTE_SELECT: &str = "
    SELECT yorc.command_id, ca.actor, yorc.orchestrator_worker_id,
           yorc.expected_orchestrator_version, yorc.target_project_id,
           yorc.target_orchestrator_worker_id, yorc.expected_project_version,
           yorc.prompt_text, ca.status, ca.error_message,
           yorc.result_runtime_status, ca.created_at_unix_ms,
           ca.updated_at_unix_ms, yorc.submitted_at_unix_ms
      FROM yard_orchestrator_route_commands yorc
      JOIN command_acknowledgements ca ON ca.id = yorc.command_id";

const PROFILE_SELECT: &str = "
    SELECT wp.id, wp.current_version
      FROM worker_profiles wp";

const ASSIGNMENT_SELECT: &str = "
    SELECT a.id, a.project_id, a.allocation_id,
           a.profile_id, a.profile_version, pr.name,
           a.objective, a.role, a.isolation_policy, a.lifecycle,
           a.version, a.created_at_unix_ms, a.updated_at_unix_ms,
           w.id, w.profile_id, w.profile_version,
           CASE WHEN w.ended_at_unix_ms IS NULL
                THEN 'running' ELSE 'ended' END,
           w.version,
           w.created_at_unix_ms, w.updated_at_unix_ms,
           wrb.adapter, wrb.runtime_session, wrb.runtime_workspace_id,
           wrb.terminal_id, wrb.tab_id, wrb.pane_id,
           wrb.provider_session_source, wrb.provider_session_provider,
           wrb.provider_session_kind, wrb.provider_session_value,
           wrb.owns_tab, wrb.observation_state, wrb.process_state,
           wrb.observed_status, wrb.state_change_sequence,
           wrb.runtime_revision, wrb.version, wrb.last_observed_at_unix_ms,
           aa.id, aa.ordinal, aa.lifecycle, aa.error_message, aa.version,
           aa.created_at_unix_ms, aa.updated_at_unix_ms,
           wa.mode, wa.started_by_command_id, wa.started_at_unix_ms
      FROM assignments a
      JOIN worker_allocations wa ON wa.id = a.allocation_id
      JOIN workers w ON w.id = a.worker_id
      LEFT JOIN worker_runtime_bindings wrb ON wrb.worker_id = w.id
      JOIN worker_profile_revisions pr
        ON pr.profile_id = a.profile_id AND pr.version = a.profile_version
      JOIN assignment_attempts aa ON aa.assignment_id = a.id
       AND aa.ordinal = (
           SELECT MAX(latest.ordinal)
             FROM assignment_attempts latest
            WHERE latest.assignment_id = a.id
       )";

const WORKER_CANDIDATE_SELECT: &str = "
    SELECT w.id, w.profile_id, w.profile_version,
           CASE WHEN w.ended_at_unix_ms IS NULL
                THEN 'running' ELSE 'ended' END,
           w.version,
           w.created_at_unix_ms, w.updated_at_unix_ms,
           pr.name, pr.default_role,
           wrb.adapter, wrb.runtime_session, wrb.runtime_workspace_id,
           wrb.terminal_id, wrb.tab_id, wrb.pane_id,
           wrb.provider_session_source, wrb.provider_session_provider,
           wrb.provider_session_kind, wrb.provider_session_value,
           wrb.owns_tab, wrb.observation_state, wrb.process_state,
           wrb.observed_status, wrb.state_change_sequence,
           wrb.runtime_revision, wrb.version, wrb.last_observed_at_unix_ms,
           (
               SELECT yo.worker_id
                 FROM yard_orchestrator yo
                WHERE yo.worker_id = w.id
                LIMIT 1
           ),
           (
               SELECT p.id
                 FROM projects p
                WHERE p.orchestrator_worker_id = w.id
                LIMIT 1
           ),
           (
               SELECT wa.project_id
                 FROM worker_allocations wa
                WHERE wa.worker_id = w.id
                  AND wa.ended_at_unix_ms IS NULL
                ORDER BY wa.started_at_unix_ms DESC, wa.id DESC
                LIMIT 1
           ),
           (
               SELECT a.id
                FROM assignments a
                WHERE a.worker_id = w.id
                  AND a.lifecycle IN ('allocating', 'active', 'handing_off')
                ORDER BY a.created_at_unix_ms DESC, a.id DESC
                LIMIT 1
           ),
           (
               SELECT cn.id
                 FROM coordination_nodes cn
                WHERE cn.worker_id = w.id
                LIMIT 1
           )
      FROM workers w
      LEFT JOIN worker_profile_revisions pr
        ON pr.profile_id = w.profile_id AND pr.version = w.profile_version
      LEFT JOIN worker_runtime_bindings wrb ON wrb.worker_id = w.id";

fn open_connection(path: &Path) -> Result<Connection, ProjectStoreError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(connection)
}

fn acquire_database_lock(path: &Path) -> Result<File, ProjectStoreError> {
    let mut lock_path = OsString::from(path.as_os_str());
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(lock_path))?;
    lock.try_lock_exclusive().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            ProjectStoreError::DatabaseAlreadyOpen
        } else {
            ProjectStoreError::Io(error)
        }
    })?;
    Ok(lock)
}

fn recover_interrupted_prompt_commands(
    connection: &mut Connection,
) -> Result<(), ProjectStoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = unix_time_ms()?;
    transaction.execute(
        "UPDATE command_acknowledgements
            SET status = 'ambiguous',
                error_message = ?1,
                updated_at_unix_ms = ?2
          WHERE status = 'pending'
            AND command_type IN (
                'assignment_prompt',
                'orchestrator_prompt',
                'yard_orchestrator_prompt',
                'yard_orchestrator_route',
                'coordination_node_prompt',
                'coordination_node_route'
            )",
        params![
            "Yard restarted before prompt acknowledgement; delivery may have occurred and was not retried",
            to_i64(now)?,
        ],
    )?;
    transaction.execute(
        "UPDATE coordination_snapshot_projects
            SET delivery_status = 'ambiguous',
                delivery_error = ?1
          WHERE delivery_status = 'pending'",
        ["Yard restarted before snapshot prompt acknowledgement; delivery may have occurred and was not retried"],
    )?;
    recover_automation_runs(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn recover_automation_runs(transaction: &Transaction<'_>) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "UPDATE automation_runs
            SET status = 'submitted',
                version = version + 1,
                runtime_status = COALESCE(
                    (
                        SELECT result_runtime_status
                          FROM yard_orchestrator_prompt_commands
                         WHERE command_id = automation_runs.dispatch_command_id
                    ),
                    (
                        SELECT result_runtime_status
                          FROM orchestrator_prompt_commands
                         WHERE command_id = automation_runs.dispatch_command_id
                    ),
                    (
                        SELECT result_runtime_status
                          FROM coordination_node_prompt_commands
                         WHERE command_id = automation_runs.dispatch_command_id
                    ),
                    'submitted'
                ),
                submitted_at_unix_ms = COALESCE(
                    (
                        SELECT submitted_at_unix_ms
                          FROM yard_orchestrator_prompt_commands
                         WHERE command_id = automation_runs.dispatch_command_id
                    ),
                    (
                        SELECT submitted_at_unix_ms
                          FROM orchestrator_prompt_commands
                         WHERE command_id = automation_runs.dispatch_command_id
                    ),
                    (
                        SELECT submitted_at_unix_ms
                          FROM coordination_node_prompt_commands
                         WHERE command_id = automation_runs.dispatch_command_id
                    ),
                    (
                        SELECT updated_at_unix_ms
                          FROM command_acknowledgements
                         WHERE id = automation_runs.dispatch_command_id
                    )
                ),
                updated_at_unix_ms = (
                    SELECT updated_at_unix_ms
                      FROM command_acknowledgements
                     WHERE id = automation_runs.dispatch_command_id
                )
          WHERE status = 'pending'
            AND EXISTS (
                SELECT 1
                  FROM command_acknowledgements
                 WHERE id = automation_runs.dispatch_command_id
                   AND status = 'succeeded'
            )",
        [],
    )?;
    transaction.execute(
        "UPDATE automation_runs
            SET status = (
                    SELECT status
                      FROM command_acknowledgements
                     WHERE id = automation_runs.dispatch_command_id
                ),
                version = version + 1,
                error_message = (
                    SELECT error_message
                      FROM command_acknowledgements
                     WHERE id = automation_runs.dispatch_command_id
                ),
                updated_at_unix_ms = (
                    SELECT updated_at_unix_ms
                      FROM command_acknowledgements
                     WHERE id = automation_runs.dispatch_command_id
                )
          WHERE status = 'pending'
            AND EXISTS (
                SELECT 1
                  FROM command_acknowledgements
                 WHERE id = automation_runs.dispatch_command_id
                   AND status IN ('failed', 'ambiguous')
            )",
        [],
    )?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn migrate(connection: &mut Connection) -> Result<(), ProjectStoreError> {
    let mut current: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(ProjectStoreError::UnsupportedSchema {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    if current == 0 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(INITIAL_MIGRATION)?;
        transaction.commit()?;
        current = 1;
    }
    if current == 1 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(PROFILE_ASSIGNMENT_MIGRATION)?;
        transaction.commit()?;
        current = 2;
    }
    if current == 2 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(COMPLETION_RECEIPT_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 3;
    }
    if current == 3 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(ASSIGNMENT_PROMPT_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 4;
    }
    if current == 4 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(RUNTIME_RECONCILIATION_MIGRATION)?;
        transaction.commit()?;
        current = 5;
    }
    if current == 5 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(RECONCILIATION_WATERMARK_MIGRATION)?;
        transaction.commit()?;
        current = 6;
    }
    if current == 6 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(WORKER_ALLOCATION_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 7;
    }
    if current == 7 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(PROFILE_PROJECT_CREATION_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 8;
    }
    if current == 8 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(WORKER_HANDOFF_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 9;
    }
    if current == 9 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(RUNTIME_CLEANUP_MIGRATION)?;
        transaction.commit()?;
        current = 10;
    }
    if current == 10 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(ARTIFACT_MIGRATION)?;
        transaction.commit()?;
        current = 11;
    }
    if current == 11 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(WORKSPACE_PROJECT_CREATION_MIGRATION)?;
        transaction.commit()?;
        current = 12;
    }
    if current == 12 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(ORCHESTRATOR_PROMPT_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 13;
    }
    if current == 13 {
        if worker_session_end_schema_exists(connection)? {
            if command_acknowledgements_support_worker_session_end(connection)? {
                connection.execute_batch("PRAGMA user_version = 14;")?;
            } else {
                connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
                let migration = (|| {
                    let transaction =
                        connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    transaction.execute_batch(WORKER_SESSION_END_ACK_REPAIR_MIGRATION)?;
                    ensure_foreign_keys(&transaction)?;
                    transaction.commit()?;
                    Ok::<(), ProjectStoreError>(())
                })();
                let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
                migration?;
                foreign_keys?;
            }
            ensure_foreign_keys(connection)?;
            current = 14;
        } else {
            connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
            let migration = (|| {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                transaction.execute_batch(WORKER_SESSION_END_MIGRATION)?;
                ensure_foreign_keys(&transaction)?;
                transaction.commit()?;
                Ok::<(), ProjectStoreError>(())
            })();
            let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
            migration?;
            foreign_keys?;
            current = 14;
        }
    }
    if current == 14 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(YARD_ORCHESTRATOR_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 15;
    }
    if current == 15 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(COORDINATION_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 16;
    }
    if current == 16 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(COORDINATION_NODES_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
        current = 17;
    }
    if current == 17 {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(AUTOMATIONS_MIGRATION)?;
            ensure_foreign_keys(&transaction)?;
            transaction.commit()?;
            Ok::<(), ProjectStoreError>(())
        })();
        let foreign_keys = connection.execute_batch("PRAGMA foreign_keys = ON;");
        migration?;
        foreign_keys?;
    }
    ensure_foreign_keys(connection)?;
    Ok(())
}

fn ensure_foreign_keys(connection: &Connection) -> Result<(), ProjectStoreError> {
    let foreign_key_error = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()?;
    if foreign_key_error.is_some() {
        return Err(ProjectStoreError::ForeignKeyCheckFailed);
    }
    Ok(())
}

fn worker_session_end_schema_exists(connection: &Connection) -> Result<bool, ProjectStoreError> {
    Ok(table_exists(connection, "worker_session_end_commands")?
        && table_has_column(connection, "workers", "ended_at_unix_ms")?
        && table_has_column(connection, "retired_runtime_bindings", "command_id")?
        && table_has_column(connection, "retired_runtime_bindings", "reason")?
        && !table_has_column(connection, "retired_runtime_bindings", "handoff_command_id")?
        && table_has_column(connection, "runtime_cleanup_jobs", "command_id")?
        && !table_has_column(connection, "runtime_cleanup_jobs", "handoff_command_id")?)
}

fn command_acknowledgements_support_worker_session_end(
    connection: &Connection,
) -> Result<bool, ProjectStoreError> {
    let command_schema = connection
        .query_row(
            "SELECT sql
               FROM sqlite_master
              WHERE type = 'table' AND name = 'command_acknowledgements'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(command_schema.is_some_and(|schema| schema.contains("'worker_session_end'")))
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1
                  FROM sqlite_master
                 WHERE type = 'table' AND name = ?1
             )",
            [table],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, ProjectStoreError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_project_workspace_available(
    transaction: &Transaction<'_>,
    project_runtime: &ProjectRuntimeBinding,
) -> Result<(), ProjectStoreError> {
    ensure_project_workspace_unbound(transaction, project_runtime)?;
    let reserved = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM profile_project_creation_commands
             WHERE runtime_adapter = ?1
               AND runtime_session = ?2
               AND runtime_workspace_id = ?3
               AND finished_at_unix_ms IS NULL
         )",
        params![
            project_runtime.adapter,
            project_runtime.session,
            project_runtime.workspace_id,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if reserved {
        return Err(ProjectStoreError::RuntimeWorkspaceReserved);
    }
    Ok(())
}

fn ensure_workspace_project_creation_available(
    transaction: &Transaction<'_>,
    command: &CreateWorkspaceProjectFromProfile,
) -> Result<(), ProjectStoreError> {
    let reserved = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM workspace_project_creation_commands
             WHERE runtime_adapter = ?1
               AND runtime_session = ?2
               AND cwd = ?3
               AND finished_at_unix_ms IS NULL
         )",
        params![
            command.runtime_adapter,
            command.runtime_session,
            command.cwd,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if reserved {
        return Err(ProjectStoreError::RuntimeWorkspaceReserved);
    }
    Ok(())
}

fn ensure_project_workspace_unbound(
    transaction: &Transaction<'_>,
    project_runtime: &ProjectRuntimeBinding,
) -> Result<(), ProjectStoreError> {
    let workspace_exists = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM project_workspace_bindings
             WHERE adapter = ?1
               AND runtime_session = ?2
               AND runtime_workspace_id = ?3
         )",
        params![
            project_runtime.adapter,
            project_runtime.session,
            project_runtime.workspace_id,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if workspace_exists {
        return Err(ProjectStoreError::RuntimeWorkspaceAlreadyBound);
    }
    Ok(())
}

fn ensure_runtime_snapshot_current(
    transaction: &Transaction<'_>,
    runtime: &WorkerRuntimeBinding,
) -> Result<(), ProjectStoreError> {
    let watermark = transaction
        .query_row(
            "SELECT observed_at_unix_ms
               FROM runtime_reconciliation_watermarks
              WHERE adapter = ?1 AND runtime_session = ?2",
            params![runtime.adapter, runtime.session],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(to_u64)
        .transpose()?;
    if watermark.is_some_and(|watermark| watermark > runtime.last_observed_at_unix_ms) {
        return Err(ProjectStoreError::StaleRuntimeSnapshot);
    }
    Ok(())
}

fn select_reusable_runtime_worker(
    transaction: &Transaction<'_>,
    runtime: &WorkerRuntimeBinding,
) -> Result<Option<String>, ProjectStoreError> {
    let existing = transaction
        .query_row(
            "SELECT wrb.worker_id, w.profile_id,
                    EXISTS (
                        SELECT 1
                          FROM worker_allocations wa
                         WHERE wa.worker_id = wrb.worker_id
                           AND wa.ended_at_unix_ms IS NULL
                    ),
                    wrb.runtime_workspace_id
               FROM worker_runtime_bindings wrb
               JOIN workers w ON w.id = wrb.worker_id
              WHERE wrb.adapter = ?1
                AND wrb.runtime_session = ?2
                AND wrb.terminal_id = ?3",
            params![runtime.adapter, runtime.session, runtime.terminal_id,],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((worker_id, profile_id, allocated, workspace_id)) = existing else {
        return Ok(None);
    };
    if profile_id.is_some() || allocated || workspace_id != runtime.workspace_id {
        return Err(ProjectStoreError::RuntimeWorkerAlreadyBound);
    }
    Ok(Some(worker_id))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn insert_project_aggregate(
    transaction: &Transaction<'_>,
    project_id: &str,
    worker_id: &str,
    project: &CreateProject,
    orchestrator_runtime: &WorkerRuntimeBinding,
    create_worker: bool,
    profile: Option<(&str, u64)>,
    allocation_mode: &str,
    started_by_command_id: Option<&str>,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let now_i64 = to_i64(now)?;
    if create_worker {
        let (profile_id, profile_version) = profile
            .map(|(profile_id, profile_version)| {
                Ok::<_, ProjectStoreError>((Some(profile_id), Some(to_i64(profile_version)?)))
            })
            .transpose()?
            .unwrap_or((None, None));
        transaction.execute(
            "INSERT INTO workers (
                id, profile_id, profile_version, desired_state, version,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES (?1, ?2, ?3, 'running', 1, ?4, ?4)",
            params![worker_id, profile_id, profile_version, now_i64],
        )?;
        insert_worker_runtime_binding(transaction, worker_id, orchestrator_runtime, now)?;
    } else if let Some((profile_id, profile_version)) = profile {
        ensure_worker_binding_available_for(transaction, worker_id, orchestrator_runtime)?;
        let (existing_profile_id, worker_version) = transaction.query_row(
            "SELECT profile_id, version FROM workers WHERE id = ?1",
            [worker_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )?;
        if existing_profile_id.is_some() {
            return Err(ProjectStoreError::RuntimeWorkerAlreadyBound);
        }
        let worker_version = to_u64(worker_version)?;
        let next_worker_version = worker_version
            .checked_add(1)
            .ok_or(ProjectStoreError::VersionOverflow)?;
        transaction.execute(
            "UPDATE workers
                SET profile_id = ?1, profile_version = ?2, version = ?3,
                    updated_at_unix_ms = ?4
              WHERE id = ?5 AND version = ?6 AND profile_id IS NULL",
            params![
                profile_id,
                to_i64(profile_version)?,
                to_i64(next_worker_version)?,
                now_i64,
                worker_id,
                to_i64(worker_version)?,
            ],
        )?;
        replace_worker_runtime_binding(transaction, worker_id, orchestrator_runtime, now)?;
    }
    transaction.execute(
        "INSERT INTO projects (
            id, name, orchestrator_worker_id, version,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES (?1, ?2, ?3, 1, ?4, ?4)",
        params![project_id, project.name, worker_id, now_i64],
    )?;
    transaction.execute(
        "INSERT INTO project_workspace_bindings (
            project_id, adapter, runtime_session, runtime_workspace_id
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            project_id,
            project.runtime.adapter,
            project.runtime.session,
            project.runtime.workspace_id,
        ],
    )?;
    transaction.execute(
        "INSERT INTO project_placements (
            project_id, canvas_x, canvas_y, canvas_width, canvas_height,
            version, updated_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
        params![
            project_id,
            project.placement.x,
            project.placement.y,
            project.placement.width,
            project.placement.height,
            now_i64,
        ],
    )?;
    transaction.execute(
        "INSERT INTO worker_allocations (
            id, project_id, worker_id, mode, started_by_command_id,
            started_at_unix_ms, ended_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        params![
            started_by_command_id.map_or_else(
                || format!("adoption:orchestrator:{project_id}"),
                |_| Uuid::now_v7().to_string(),
            ),
            project_id,
            worker_id,
            allocation_mode,
            started_by_command_id,
            now_i64,
        ],
    )?;
    if profile.is_some() {
        let worker_version = transaction.query_row(
            "SELECT version FROM workers WHERE id = ?1",
            [worker_id],
            |row| row.get::<_, i64>(0),
        )?;
        insert_lifecycle_event(
            transaction,
            "worker",
            worker_id,
            to_u64(worker_version)?,
            "profile_backed_orchestrator_created",
            "herdr",
            now,
        )?;
    }
    Ok(())
}

fn map_project_insert_error(error: ProjectStoreError) -> ProjectStoreError {
    match error {
        ProjectStoreError::Database(ref database_error) if is_unique_constraint(database_error) => {
            ProjectStoreError::RuntimeBindingAlreadyExists
        }
        error => error,
    }
}

fn select_project(connection: &Connection, project_id: &str) -> Result<Project, ProjectStoreError> {
    connection
        .query_row(
            &format!("{PROJECT_SELECT} WHERE p.id = ?1"),
            [project_id],
            project_from_row,
        )
        .optional()?
        .ok_or(ProjectStoreError::ProjectNotFound)
}

fn project_exists(connection: &Connection, project_id: &str) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)",
            [project_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn select_project_relationship(
    connection: &Connection,
    relationship_id: &str,
) -> Result<Option<ProjectRelationship>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT id, source_project_id, target_project_id, kind,
                    version, created_by, created_at_unix_ms,
                    updated_at_unix_ms
               FROM project_relationships
              WHERE id = ?1",
            [relationship_id],
            project_relationship_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn project_relationship_from_row(row: &Row<'_>) -> rusqlite::Result<ProjectRelationship> {
    Ok(ProjectRelationship {
        id: row.get(0)?,
        source_project_id: row.get(1)?,
        target_project_id: row.get(2)?,
        kind: project_relationship_kind_from_row(row, 3)?,
        version: row_u64(row, 4)?,
        created_by: row.get(5)?,
        created_at_unix_ms: row_u64(row, 6)?,
        updated_at_unix_ms: row_u64(row, 7)?,
    })
}

#[derive(Debug)]
struct StoredProjectRelationshipCreate {
    command_id: String,
    actor: String,
    relationship: ProjectRelationship,
}

impl StoredProjectRelationshipCreate {
    fn matches(&self, command: &CreateProjectRelationship) -> bool {
        self.actor == command.actor
            && self.relationship.id == command.relationship_id
            && self.relationship.source_project_id == command.source_project_id
            && self.relationship.target_project_id == command.target_project_id
            && self.relationship.kind == command.kind
    }

    fn created(self, replayed: bool) -> CreatedProjectRelationship {
        CreatedProjectRelationship {
            command_id: self.command_id,
            relationship: self.relationship,
            replayed,
        }
    }
}

fn select_project_relationship_create_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredProjectRelationshipCreate>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT prcc.command_id, ca.actor, prcc.relationship_id,
                    prcc.source_project_id, prcc.target_project_id, prcc.kind,
                    prcc.result_version, prcc.result_created_by,
                    prcc.result_created_at_unix_ms,
                    prcc.result_updated_at_unix_ms
               FROM project_relationship_create_commands prcc
               JOIN command_acknowledgements ca ON ca.id = prcc.command_id
              WHERE prcc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredProjectRelationshipCreate {
                    command_id: row.get(0)?,
                    actor: row.get(1)?,
                    relationship: ProjectRelationship {
                        id: row.get(2)?,
                        source_project_id: row.get(3)?,
                        target_project_id: row.get(4)?,
                        kind: project_relationship_kind_from_row(row, 5)?,
                        version: row_u64(row, 6)?,
                        created_by: row.get(7)?,
                        created_at_unix_ms: row_u64(row, 8)?,
                        updated_at_unix_ms: row_u64(row, 9)?,
                    },
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

#[derive(Debug)]
struct StoredProjectRelationshipDelete {
    command_id: String,
    actor: String,
    relationship_id: String,
    expected_version: u64,
    deleted_at_unix_ms: u64,
}

impl StoredProjectRelationshipDelete {
    fn matches(&self, relationship_id: &str, command: &DeleteProjectRelationship) -> bool {
        self.actor == command.actor
            && self.relationship_id == relationship_id
            && self.expected_version == command.expected_version
    }

    fn deleted(self, replayed: bool) -> DeletedProjectRelationship {
        DeletedProjectRelationship {
            command_id: self.command_id,
            relationship_id: self.relationship_id,
            deleted_at_unix_ms: self.deleted_at_unix_ms,
            replayed,
        }
    }
}

fn select_project_relationship_delete_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredProjectRelationshipDelete>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT prdc.command_id, ca.actor, prdc.relationship_id,
                    prdc.expected_version, prdc.deleted_at_unix_ms
               FROM project_relationship_delete_commands prdc
               JOIN command_acknowledgements ca ON ca.id = prdc.command_id
              WHERE prdc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredProjectRelationshipDelete {
                    command_id: row.get(0)?,
                    actor: row.get(1)?,
                    relationship_id: row.get(2)?,
                    expected_version: row_u64(row, 3)?,
                    deleted_at_unix_ms: row_u64(row, 4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_confirmed_project_creation(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<ConfirmedProjectCreation, ProjectStoreError> {
    let command = select_profile_project_creation_command(connection, command_id)?
        .ok_or(ProjectStoreError::CommandNotFound)?;
    let project_id = command
        .result_project_id
        .ok_or(ProjectStoreError::ProjectCreationResultMissing)?;
    Ok(ConfirmedProjectCreation {
        command_id: command_id.to_owned(),
        project: select_project(connection, &project_id)?,
        replayed,
    })
}

fn select_confirmed_workspace_project_creation(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<ConfirmedProjectCreation, ProjectStoreError> {
    let command = select_workspace_project_creation_command(connection, command_id)?
        .ok_or(ProjectStoreError::CommandNotFound)?;
    let project_id = command
        .result_project_id
        .ok_or(ProjectStoreError::ProjectCreationResultMissing)?;
    let runtime_workspace_id = command
        .result_runtime_workspace_id
        .ok_or(ProjectStoreError::ProjectCreationResultMissing)?;
    let project = select_project(connection, &project_id)?;
    if project.runtime.workspace_id != runtime_workspace_id {
        return Err(ProjectStoreError::ProjectCreationResultMissing);
    }
    Ok(ConfirmedProjectCreation {
        command_id: command_id.to_owned(),
        project,
        replayed,
    })
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    let runtime_adapter = row.get::<_, Option<String>>(21)?;
    let provider_session_source = row.get::<_, Option<String>>(27)?;
    let provider_session = provider_session_source
        .map(|source| -> rusqlite::Result<ProviderSessionRef> {
            Ok(ProviderSessionRef {
                source,
                provider: row.get(28)?,
                kind: row.get(29)?,
                value: row.get(30)?,
            })
        })
        .transpose()?;
    let desired_state = match row.get::<_, String>(17)?.as_str() {
        "running" => WorkerDesiredState::Running,
        "ended" => WorkerDesiredState::Ended,
        value => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                17,
                Type::Text,
                format!("unsupported worker desired state '{value}'").into(),
            ));
        }
    };
    let runtime = runtime_adapter
        .map(|adapter| -> rusqlite::Result<WorkerRuntimeBinding> {
            Ok(WorkerRuntimeBinding {
                adapter,
                session: row.get(22)?,
                workspace_id: row.get(23)?,
                terminal_id: row.get(24)?,
                tab_id: row.get(25)?,
                pane_id: row.get(26)?,
                provider_session,
                owns_tab: row.get(31)?,
                observation_state: runtime_observation_state(row, 32)?,
                process_state: runtime_process_state(row, 33)?,
                status: observed_status(row, 34)?,
                state_change_sequence: row_u64(row, 35)?,
                revision: row_u64(row, 36)?,
                version: row_u64(row, 37)?,
                last_observed_at_unix_ms: row_u64(row, 38)?,
            })
        })
        .transpose()?;

    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row_u64(row, 2)?,
        created_at_unix_ms: row_u64(row, 3)?,
        updated_at_unix_ms: row_u64(row, 4)?,
        runtime: ProjectRuntimeBinding {
            adapter: row.get(5)?,
            session: row.get(6)?,
            workspace_id: row.get(7)?,
        },
        placement: ProjectPlacement {
            geometry: CanvasPlacement {
                x: row.get(8)?,
                y: row.get(9)?,
                width: row.get(10)?,
                height: row.get(11)?,
            },
            version: row_u64(row, 12)?,
            updated_at_unix_ms: row_u64(row, 13)?,
        },
        orchestrator: Worker {
            id: row.get(14)?,
            profile_id: row.get(15)?,
            profile_version: row_optional_u64(row, 16)?,
            desired_state,
            runtime,
            version: row_u64(row, 18)?,
            created_at_unix_ms: row_u64(row, 19)?,
            updated_at_unix_ms: row_u64(row, 20)?,
        },
    })
}

fn profile_key_from_row(row: &Row<'_>) -> rusqlite::Result<(String, u64)> {
    Ok((row.get(0)?, row_u64(row, 1)?))
}

fn select_worker_profile(
    connection: &Connection,
    profile_id: &str,
) -> Result<WorkerProfile, ProjectStoreError> {
    let version = select_current_profile_version(connection, profile_id)?;
    select_worker_profile_revision(connection, profile_id, version)
}

fn select_current_profile_version(
    connection: &Connection,
    profile_id: &str,
) -> Result<u64, ProjectStoreError> {
    connection
        .query_row(
            "SELECT current_version FROM worker_profiles WHERE id = ?1",
            [profile_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(to_u64)
        .transpose()?
        .ok_or(ProjectStoreError::ProfileNotFound)
}

fn select_worker_profile_revision(
    connection: &Connection,
    profile_id: &str,
    version: u64,
) -> Result<WorkerProfile, ProjectStoreError> {
    let mut profile = connection
        .query_row(
            "SELECT pr.name, pr.runtime_adapter, pr.provider, pr.model,
                    pr.default_role, pr.instructions_ref, pr.sandbox_policy,
                    pr.worktree_policy, pr.permission_policy,
                    pr.completion_contract, wp.created_at_unix_ms,
                    wp.updated_at_unix_ms
               FROM worker_profile_revisions pr
               JOIN worker_profiles wp ON wp.id = pr.profile_id
              WHERE pr.profile_id = ?1 AND pr.version = ?2",
            params![profile_id, to_i64(version)?],
            |row| {
                Ok(WorkerProfile {
                    id: profile_id.to_owned(),
                    spec: WorkerProfileSpec {
                        name: row.get(0)?,
                        runtime_adapter: row.get(1)?,
                        provider: row.get(2)?,
                        model: row.get(3)?,
                        default_role: row.get(4)?,
                        instructions_ref: row.get(5)?,
                        tools: Vec::new(),
                        skills: Vec::new(),
                        mcp_servers: Vec::new(),
                        sandbox_policy: row.get(6)?,
                        worktree_policy: row.get(7)?,
                        permission_policy: row.get(8)?,
                        completion_contract: row.get(9)?,
                    },
                    version,
                    created_at_unix_ms: row_u64(row, 10)?,
                    updated_at_unix_ms: row_u64(row, 11)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::ProfileNotFound)?;
    profile.spec.tools =
        select_profile_values(connection, "worker_profile_tools", profile_id, version)?;
    profile.spec.skills =
        select_profile_values(connection, "worker_profile_skills", profile_id, version)?;
    profile.spec.mcp_servers = select_profile_values(
        connection,
        "worker_profile_mcp_servers",
        profile_id,
        version,
    )?;
    Ok(profile)
}

fn select_profile_values(
    connection: &Connection,
    table: &'static str,
    profile_id: &str,
    version: u64,
) -> Result<Vec<String>, ProjectStoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT value FROM {table}
          WHERE profile_id = ?1 AND profile_version = ?2
          ORDER BY position"
    ))?;
    statement
        .query_map(params![profile_id, to_i64(version)?], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn insert_profile_revision(
    transaction: &Transaction<'_>,
    profile_id: &str,
    version: u64,
    spec: &WorkerProfileSpec,
    now: u64,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO worker_profile_revisions (
            profile_id, version, name, runtime_adapter, provider, model,
            default_role, instructions_ref, sandbox_policy, worktree_policy,
            permission_policy, completion_contract, created_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
         )",
        params![
            profile_id,
            to_i64(version)?,
            spec.name,
            spec.runtime_adapter,
            spec.provider,
            spec.model,
            spec.default_role,
            spec.instructions_ref,
            spec.sandbox_policy,
            spec.worktree_policy,
            spec.permission_policy,
            spec.completion_contract,
            to_i64(now)?,
        ],
    )?;
    insert_profile_values(
        transaction,
        "worker_profile_tools",
        profile_id,
        version,
        &spec.tools,
    )?;
    insert_profile_values(
        transaction,
        "worker_profile_skills",
        profile_id,
        version,
        &spec.skills,
    )?;
    insert_profile_values(
        transaction,
        "worker_profile_mcp_servers",
        profile_id,
        version,
        &spec.mcp_servers,
    )?;
    Ok(())
}

fn insert_profile_values(
    transaction: &Transaction<'_>,
    table: &'static str,
    profile_id: &str,
    version: u64,
    values: &[String],
) -> Result<(), ProjectStoreError> {
    let sql = format!(
        "INSERT INTO {table} (profile_id, profile_version, position, value)
         VALUES (?1, ?2, ?3, ?4)"
    );
    for (position, value) in values.iter().enumerate() {
        transaction.execute(
            &sql,
            params![
                profile_id,
                to_i64(version)?,
                i64::try_from(position)?,
                value,
            ],
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct StoredProfileProjectCreationCommand {
    project_name: String,
    runtime: ProjectRuntimeBinding,
    profile_id: String,
    profile_version: u64,
    orchestrator_objective: String,
    placement: CanvasPlacement,
    actor: String,
    status: String,
    error_message: Option<String>,
    result_project_id: Option<String>,
}

impl StoredProfileProjectCreationCommand {
    fn matches(&self, command: &CreateProjectFromProfile) -> bool {
        self.project_name == command.name
            && self.runtime == command.runtime
            && self.profile_id == command.profile_id
            && self.profile_version == command.expected_profile_version
            && self.orchestrator_objective == command.orchestrator_objective
            && self.placement == command.placement
            && self.actor == command.actor
    }
}

#[derive(Debug)]
struct StoredWorkspaceProjectCreationCommand {
    project_name: String,
    runtime_adapter: String,
    runtime_session: String,
    workspace_label: String,
    cwd: String,
    profile_id: String,
    profile_version: u64,
    orchestrator_objective: String,
    placement: CanvasPlacement,
    actor: String,
    status: String,
    error_message: Option<String>,
    result_runtime_workspace_id: Option<String>,
    result_project_id: Option<String>,
}

impl StoredWorkspaceProjectCreationCommand {
    fn matches(&self, command: &CreateWorkspaceProjectFromProfile) -> bool {
        self.project_name == command.name
            && self.runtime_adapter == command.runtime_adapter
            && self.runtime_session == command.runtime_session
            && self.workspace_label == command.workspace_label
            && self.cwd == command.cwd
            && self.profile_id == command.profile_id
            && self.profile_version == command.expected_profile_version
            && self.orchestrator_objective == command.orchestrator_objective
            && self.placement == command.placement
            && self.actor == command.actor
    }
}

#[derive(Debug)]
struct StoredAllocationCommand {
    project_id: String,
    profile_id: String,
    profile_version: u64,
    expected_project_version: u64,
    objective: String,
    role: String,
    isolation_policy: String,
    actor: String,
    status: String,
    error_message: Option<String>,
    result_assignment_id: Option<String>,
}

#[derive(Debug)]
struct StoredWorkerAllocationCommand {
    project_id: String,
    worker_id: String,
    expected_worker_version: u64,
    requested_profile_id: Option<String>,
    requested_profile_version: Option<u64>,
    expected_project_version: u64,
    objective: String,
    role: String,
    isolation_policy: String,
    replace_runtime: bool,
    actor: String,
    status: String,
    error_message: Option<String>,
    result_allocation_id: Option<String>,
    result_assignment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredRuntimeIdentity {
    adapter: String,
    session: String,
    workspace_id: String,
    terminal_id: String,
    tab_id: Option<String>,
    pane_id: String,
    provider_session: Option<ProviderSessionRef>,
}

impl StoredRuntimeIdentity {
    fn matches_runtime(&self, runtime: &WorkerRuntimeBinding) -> bool {
        self.adapter == runtime.adapter
            && self.session == runtime.session
            && self.workspace_id == runtime.workspace_id
            && self.terminal_id == runtime.terminal_id
            && (self.provider_session.is_none()
                || self.provider_session.as_ref() == runtime.provider_session.as_ref())
    }
}

#[derive(Debug)]
struct StoredWorkerHandoffCommand {
    source_project_id: String,
    source_assignment_id: String,
    source_attempt_id: String,
    worker_id: String,
    expected_worker_version: u64,
    expected_source_project_version: u64,
    expected_source_assignment_version: u64,
    expected_source_attempt_version: u64,
    target_project_id: String,
    expected_target_project_version: u64,
    target_role: HandoffTargetRole,
    objective: String,
    role: String,
    isolation_policy: String,
    source_runtime: StoredRuntimeIdentity,
    target_runtime: Option<StoredRuntimeIdentity>,
    replaced_orchestrator_worker_id: Option<String>,
    result_allocation_id: Option<String>,
    result_assignment_id: Option<String>,
    actor: String,
    status: String,
    error_message: Option<String>,
}

impl StoredWorkerHandoffCommand {
    fn matches(
        &self,
        source_project_id: &str,
        source_assignment_id: &str,
        command: &ConfirmWorkerHandoff,
    ) -> bool {
        self.source_project_id == source_project_id
            && self.source_assignment_id == source_assignment_id
            && self.source_attempt_id == command.source_attempt_id
            && self.worker_id == command.worker_id
            && self.expected_worker_version == command.expected_worker_version
            && self.expected_source_project_version == command.expected_source_project_version
            && self.expected_source_assignment_version == command.expected_source_assignment_version
            && self.expected_source_attempt_version == command.expected_source_attempt_version
            && self.target_project_id == command.target_project_id
            && self.expected_target_project_version == command.expected_target_project_version
            && self.target_role == command.target_role
            && self.objective == command.objective
            && self.role == command.role
            && self.isolation_policy == "project_workspace"
            && self.actor == command.actor
    }
}

impl StoredWorkerAllocationCommand {
    fn matches(&self, project_id: &str, command: &ConfirmWorkerAllocation) -> bool {
        self.project_id == project_id
            && self.worker_id == command.worker_id
            && self.expected_worker_version == command.expected_worker_version
            && self.requested_profile_id == command.profile_id
            && self.requested_profile_version == command.expected_profile_version
            && self.expected_project_version == command.expected_project_version
            && self.objective == command.objective
            && self.role == command.role
            && self.isolation_policy == "project_workspace"
            && self.actor == command.actor
    }
}

impl StoredAllocationCommand {
    fn matches(&self, project_id: &str, command: &ConfirmProfileAllocation) -> bool {
        self.project_id == project_id
            && self.profile_id == command.profile_id
            && self.profile_version == command.expected_profile_version
            && self.expected_project_version == command.expected_project_version
            && self.objective == command.objective
            && self.role == command.role
            && self.isolation_policy == "project_workspace"
            && self.actor == command.actor
    }
}

fn select_profile_project_creation_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredProfileProjectCreationCommand>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT ppcc.project_name, ppcc.runtime_adapter,
                    ppcc.runtime_session, ppcc.runtime_workspace_id,
                    ppcc.profile_id, ppcc.profile_version,
                    ppcc.orchestrator_objective, ppcc.canvas_x, ppcc.canvas_y,
                    ppcc.canvas_width, ppcc.canvas_height, ca.actor, ca.status,
                    ca.error_message, ppcc.result_project_id
               FROM profile_project_creation_commands ppcc
               JOIN command_acknowledgements ca ON ca.id = ppcc.command_id
              WHERE ppcc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredProfileProjectCreationCommand {
                    project_name: row.get(0)?,
                    runtime: ProjectRuntimeBinding {
                        adapter: row.get(1)?,
                        session: row.get(2)?,
                        workspace_id: row.get(3)?,
                    },
                    profile_id: row.get(4)?,
                    profile_version: row_u64(row, 5)?,
                    orchestrator_objective: row.get(6)?,
                    placement: CanvasPlacement {
                        x: row.get(7)?,
                        y: row.get(8)?,
                        width: row.get(9)?,
                        height: row.get(10)?,
                    },
                    actor: row.get(11)?,
                    status: row.get(12)?,
                    error_message: row.get(13)?,
                    result_project_id: row.get(14)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_workspace_project_creation_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredWorkspaceProjectCreationCommand>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT wpcc.project_name, wpcc.runtime_adapter,
                    wpcc.runtime_session, wpcc.workspace_label, wpcc.cwd,
                    wpcc.profile_id, wpcc.profile_version,
                    wpcc.orchestrator_objective, wpcc.canvas_x, wpcc.canvas_y,
                    wpcc.canvas_width, wpcc.canvas_height, ca.actor, ca.status,
                    ca.error_message, wpcc.result_runtime_workspace_id,
                    wpcc.result_project_id
               FROM workspace_project_creation_commands wpcc
               JOIN command_acknowledgements ca ON ca.id = wpcc.command_id
              WHERE wpcc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredWorkspaceProjectCreationCommand {
                    project_name: row.get(0)?,
                    runtime_adapter: row.get(1)?,
                    runtime_session: row.get(2)?,
                    workspace_label: row.get(3)?,
                    cwd: row.get(4)?,
                    profile_id: row.get(5)?,
                    profile_version: row_u64(row, 6)?,
                    orchestrator_objective: row.get(7)?,
                    placement: CanvasPlacement {
                        x: row.get(8)?,
                        y: row.get(9)?,
                        width: row.get(10)?,
                        height: row.get(11)?,
                    },
                    actor: row.get(12)?,
                    status: row.get(13)?,
                    error_message: row.get(14)?,
                    result_runtime_workspace_id: row.get(15)?,
                    result_project_id: row.get(16)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_worker_allocation_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredWorkerAllocationCommand>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT wac.project_id, wac.worker_id,
                    wac.expected_worker_version, wac.requested_profile_id,
                    wac.requested_profile_version,
                    wac.expected_project_version, wac.objective, wac.role,
                    wac.isolation_policy, wac.replace_runtime,
                    ca.actor, ca.status, ca.error_message,
                    wac.result_allocation_id, wac.result_assignment_id
               FROM worker_allocation_commands wac
               JOIN command_acknowledgements ca ON ca.id = wac.command_id
              WHERE wac.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredWorkerAllocationCommand {
                    project_id: row.get(0)?,
                    worker_id: row.get(1)?,
                    expected_worker_version: row_u64(row, 2)?,
                    requested_profile_id: row.get(3)?,
                    requested_profile_version: row_optional_u64(row, 4)?,
                    expected_project_version: row_u64(row, 5)?,
                    objective: row.get(6)?,
                    role: row.get(7)?,
                    isolation_policy: row.get(8)?,
                    replace_runtime: row.get(9)?,
                    actor: row.get(10)?,
                    status: row.get(11)?,
                    error_message: row.get(12)?,
                    result_allocation_id: row.get(13)?,
                    result_assignment_id: row.get(14)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_worker_handoff_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredWorkerHandoffCommand>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT whc.source_project_id, whc.source_assignment_id,
                    whc.source_attempt_id, whc.worker_id,
                    whc.expected_worker_version,
                    whc.expected_source_project_version,
                    whc.expected_source_assignment_version,
                    whc.expected_source_attempt_version,
                    whc.target_project_id, whc.expected_target_project_version,
                    whc.target_role, whc.objective, whc.role,
                    whc.isolation_policy, whc.source_runtime_adapter,
                    whc.source_runtime_session, whc.source_runtime_workspace_id,
                    whc.source_terminal_id, whc.source_tab_id,
                    whc.source_pane_id, whc.source_provider_session_source,
                    whc.source_provider_session_provider,
                    whc.source_provider_session_kind,
                    whc.source_provider_session_value,
                    whc.target_runtime_adapter, whc.target_runtime_session,
                    whc.target_runtime_workspace_id, whc.target_terminal_id,
                    whc.target_tab_id, whc.target_pane_id,
                    whc.target_provider_session_source,
                    whc.target_provider_session_provider,
                    whc.target_provider_session_kind,
                    whc.target_provider_session_value,
                    whc.replaced_orchestrator_worker_id,
                    whc.result_allocation_id, whc.result_assignment_id,
                    ca.actor, ca.status, ca.error_message
               FROM worker_handoff_commands whc
               JOIN command_acknowledgements ca ON ca.id = whc.command_id
              WHERE whc.command_id = ?1",
            [command_id],
            |row| {
                let source_provider = provider_session_from_columns(row, 20)?;
                let target_adapter = row.get::<_, Option<String>>(24)?;
                let target_runtime = target_adapter
                    .map(|adapter| -> rusqlite::Result<StoredRuntimeIdentity> {
                        Ok(StoredRuntimeIdentity {
                            adapter,
                            session: row.get(25)?,
                            workspace_id: row.get(26)?,
                            terminal_id: row.get(27)?,
                            tab_id: row.get(28)?,
                            pane_id: row.get(29)?,
                            provider_session: provider_session_from_columns(row, 30)?,
                        })
                    })
                    .transpose()?;
                Ok(StoredWorkerHandoffCommand {
                    source_project_id: row.get(0)?,
                    source_assignment_id: row.get(1)?,
                    source_attempt_id: row.get(2)?,
                    worker_id: row.get(3)?,
                    expected_worker_version: row_u64(row, 4)?,
                    expected_source_project_version: row_u64(row, 5)?,
                    expected_source_assignment_version: row_u64(row, 6)?,
                    expected_source_attempt_version: row_u64(row, 7)?,
                    target_project_id: row.get(8)?,
                    expected_target_project_version: row_u64(row, 9)?,
                    target_role: handoff_target_role_from_row(row, 10)?,
                    objective: row.get(11)?,
                    role: row.get(12)?,
                    isolation_policy: row.get(13)?,
                    source_runtime: StoredRuntimeIdentity {
                        adapter: row.get(14)?,
                        session: row.get(15)?,
                        workspace_id: row.get(16)?,
                        terminal_id: row.get(17)?,
                        tab_id: row.get(18)?,
                        pane_id: row.get(19)?,
                        provider_session: source_provider,
                    },
                    target_runtime,
                    replaced_orchestrator_worker_id: row.get(34)?,
                    result_allocation_id: row.get(35)?,
                    result_assignment_id: row.get(36)?,
                    actor: row.get(37)?,
                    status: row.get(38)?,
                    error_message: row.get(39)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_allocation_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredAllocationCommand>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT pac.project_id, pac.profile_id, pac.profile_version,
                    pac.expected_project_version, pac.objective, pac.role,
                    pac.isolation_policy, ca.actor, ca.status, ca.error_message,
                    pac.result_assignment_id
               FROM profile_allocation_commands pac
               JOIN command_acknowledgements ca ON ca.id = pac.command_id
              WHERE pac.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredAllocationCommand {
                    project_id: row.get(0)?,
                    profile_id: row.get(1)?,
                    profile_version: row_u64(row, 2)?,
                    expected_project_version: row_u64(row, 3)?,
                    objective: row.get(4)?,
                    role: row.get(5)?,
                    isolation_policy: row.get(6)?,
                    actor: row.get(7)?,
                    status: row.get(8)?,
                    error_message: row.get(9)?,
                    result_assignment_id: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn ensure_worker_binding_available(
    transaction: &Transaction<'_>,
    runtime: &WorkerRuntimeBinding,
) -> Result<(), ProjectStoreError> {
    let exists = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM worker_runtime_bindings
             WHERE adapter = ?1
               AND runtime_session = ?2
               AND terminal_id = ?3
         )",
        params![runtime.adapter, runtime.session, runtime.terminal_id],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        Err(ProjectStoreError::RuntimeWorkerAlreadyBound)
    } else {
        Ok(())
    }
}

fn ensure_worker_binding_available_for(
    transaction: &Transaction<'_>,
    worker_id: &str,
    runtime: &WorkerRuntimeBinding,
) -> Result<(), ProjectStoreError> {
    let exists = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM worker_runtime_bindings
             WHERE adapter = ?1
               AND runtime_session = ?2
               AND terminal_id = ?3
               AND worker_id <> ?4
         )",
        params![
            runtime.adapter,
            runtime.session,
            runtime.terminal_id,
            worker_id,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        Err(ProjectStoreError::RuntimeWorkerAlreadyBound)
    } else {
        Ok(())
    }
}

fn insert_runtime_cleanup_job(
    transaction: &Transaction<'_>,
    command_id: &str,
    worker_id: &str,
    reason: &str,
    runtime: &WorkerRuntimeBinding,
    now: u64,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO runtime_cleanup_jobs (
            id, command_id, worker_id, reason, adapter,
            runtime_session, runtime_workspace_id, terminal_id, tab_id,
            pane_id, owns_tab, status, attempts, last_error,
            next_attempt_at_unix_ms, created_at_unix_ms,
            updated_at_unix_ms, completed_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            'pending', 0, NULL, ?12, ?12, ?12, NULL
         )",
        params![
            Uuid::now_v7().to_string(),
            command_id,
            worker_id,
            reason,
            runtime.adapter,
            runtime.session,
            runtime.workspace_id,
            runtime.terminal_id,
            runtime.tab_id,
            runtime.pane_id,
            runtime.owns_tab,
            to_i64(now)?,
        ],
    )?;
    Ok(())
}

fn insert_retired_runtime_binding(
    transaction: &Transaction<'_>,
    command_id: &str,
    worker_id: &str,
    reason: &str,
    runtime: &WorkerRuntimeBinding,
    now: u64,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO retired_runtime_bindings (
            id, worker_id, command_id, reason, adapter, runtime_session,
            runtime_workspace_id, terminal_id, tab_id, pane_id,
            provider_session_source, provider_session_provider,
            provider_session_kind, provider_session_value,
            retired_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15
         )",
        params![
            Uuid::now_v7().to_string(),
            worker_id,
            command_id,
            reason,
            runtime.adapter,
            runtime.session,
            runtime.workspace_id,
            runtime.terminal_id,
            runtime.tab_id,
            runtime.pane_id,
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.source.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.provider.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.kind.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            to_i64(now)?,
        ],
    )?;
    Ok(())
}

fn runtime_cleanup_pending(
    transaction: &Transaction<'_>,
    command_id: &str,
) -> Result<bool, ProjectStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS (
                SELECT 1
                  FROM runtime_cleanup_jobs
                 WHERE command_id = ?1 AND status = 'pending'
             )",
            [command_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn insert_worker_runtime_binding(
    transaction: &Transaction<'_>,
    worker_id: &str,
    runtime: &WorkerRuntimeBinding,
    now: u64,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO worker_runtime_bindings (
            worker_id, adapter, runtime_session, runtime_workspace_id,
            terminal_id, tab_id, pane_id, provider_session_source,
            provider_session_provider, provider_session_kind,
            provider_session_value, owns_tab, observation_state,
            observed_status, process_state, state_change_sequence,
            runtime_revision, version, last_observed_at_unix_ms,
            updated_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
         )",
        params![
            worker_id,
            runtime.adapter,
            runtime.session,
            runtime.workspace_id,
            runtime.terminal_id,
            runtime.tab_id,
            runtime.pane_id,
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.source.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.provider.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.kind.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            runtime.owns_tab,
            runtime_observation_state_value(runtime.observation_state),
            observed_status_value(runtime.status),
            runtime_process_state_value(runtime.process_state),
            to_i64(runtime.state_change_sequence)?,
            to_i64(runtime.revision)?,
            to_i64(runtime.version)?,
            to_i64(runtime.last_observed_at_unix_ms)?,
            to_i64(now)?,
        ],
    )?;
    Ok(())
}

fn replace_worker_runtime_binding(
    transaction: &Transaction<'_>,
    worker_id: &str,
    runtime: &WorkerRuntimeBinding,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let current_version = transaction.query_row(
        "SELECT version FROM worker_runtime_bindings WHERE worker_id = ?1",
        [worker_id],
        |row| row.get::<_, i64>(0),
    )?;
    let current_version = to_u64(current_version)?;
    let next_version = current_version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    let rows = transaction.execute(
        "UPDATE worker_runtime_bindings
            SET adapter = ?1, runtime_session = ?2, runtime_workspace_id = ?3,
                terminal_id = ?4, tab_id = ?5, pane_id = ?6,
                provider_session_source = ?7, provider_session_provider = ?8,
                provider_session_kind = ?9, provider_session_value = ?10,
                owns_tab = ?11, observation_state = ?12,
                observed_status = ?13, process_state = ?14,
                state_change_sequence = ?15, runtime_revision = ?16,
                version = ?17, last_observed_at_unix_ms = ?18,
                updated_at_unix_ms = ?19
          WHERE worker_id = ?20 AND version = ?21",
        params![
            runtime.adapter,
            runtime.session,
            runtime.workspace_id,
            runtime.terminal_id,
            runtime.tab_id,
            runtime.pane_id,
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.source.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.provider.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.kind.as_str()),
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            runtime.owns_tab,
            runtime_observation_state_value(runtime.observation_state),
            observed_status_value(runtime.status),
            runtime_process_state_value(runtime.process_state),
            to_i64(runtime.state_change_sequence)?,
            to_i64(runtime.revision)?,
            to_i64(next_version)?,
            to_i64(runtime.last_observed_at_unix_ms)?,
            to_i64(now)?,
            worker_id,
            to_i64(current_version)?,
        ],
    )?;
    if rows != 1 {
        return Err(ProjectStoreError::RuntimeBindingVersionConflict);
    }
    Ok(())
}

fn select_worker_candidate(
    connection: &Connection,
    worker_id: &str,
) -> Result<Option<WorkerCandidate>, ProjectStoreError> {
    connection
        .query_row(
            &format!("{WORKER_CANDIDATE_SELECT} WHERE w.id = ?1"),
            [worker_id],
            worker_candidate_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn select_yard_orchestrator(
    connection: &Connection,
) -> Result<YardOrchestrator, ProjectStoreError> {
    let (worker_id, version, created_at_unix_ms, updated_at_unix_ms) = connection.query_row(
        "SELECT worker_id, version, created_at_unix_ms, updated_at_unix_ms
           FROM yard_orchestrator
          WHERE singleton_id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row_u64(row, 1)?,
                row_u64(row, 2)?,
                row_u64(row, 3)?,
            ))
        },
    )?;
    let worker = worker_id
        .map(|worker_id| {
            select_worker_candidate(connection, &worker_id)?
                .map(|candidate| candidate.worker)
                .ok_or(ProjectStoreError::WorkerNotFound)
        })
        .transpose()?;
    Ok(YardOrchestrator {
        worker,
        version,
        created_at_unix_ms,
        updated_at_unix_ms,
    })
}

fn select_configured_yard_orchestrator(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<ConfiguredYardOrchestrator, ProjectStoreError> {
    let (worker_id, result_version, replaced_worker_id, finished_at_unix_ms) = connection
        .query_row(
            "SELECT worker_id, result_orchestrator_version,
                    replaced_worker_id, finished_at_unix_ms
               FROM yard_orchestrator_configure_commands
              WHERE command_id = ?1",
            [command_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row_u64(row, 1)?,
                    row.get::<_, Option<String>>(2)?,
                    row_u64(row, 3)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::CommandNotFound)?;
    let worker = select_worker_candidate(connection, &worker_id)?
        .map(|candidate| candidate.worker)
        .ok_or(ProjectStoreError::WorkerNotFound)?;
    let created_at_unix_ms = connection.query_row(
        "SELECT created_at_unix_ms
           FROM yard_orchestrator
          WHERE singleton_id = 1",
        [],
        |row| row_u64(row, 0),
    )?;
    Ok(ConfiguredYardOrchestrator {
        command_id: command_id.to_owned(),
        orchestrator: YardOrchestrator {
            worker: Some(worker),
            version: result_version,
            created_at_unix_ms,
            updated_at_unix_ms: finished_at_unix_ms,
        },
        replaced_worker_id,
        replayed,
    })
}

fn worker_candidate_from_row(row: &Row<'_>) -> rusqlite::Result<WorkerCandidate> {
    let provider_session_source = row.get::<_, Option<String>>(15)?;
    let provider_session = provider_session_source
        .map(|source| -> rusqlite::Result<ProviderSessionRef> {
            Ok(ProviderSessionRef {
                source,
                provider: row.get(16)?,
                kind: row.get(17)?,
                value: row.get(18)?,
            })
        })
        .transpose()?;
    let runtime_adapter = row.get::<_, Option<String>>(9)?;
    let runtime = runtime_adapter
        .map(|adapter| -> rusqlite::Result<WorkerRuntimeBinding> {
            Ok(WorkerRuntimeBinding {
                adapter,
                session: row.get(10)?,
                workspace_id: row.get(11)?,
                terminal_id: row.get(12)?,
                tab_id: row.get(13)?,
                pane_id: row.get(14)?,
                provider_session,
                owns_tab: row.get(19)?,
                observation_state: runtime_observation_state(row, 20)?,
                process_state: runtime_process_state(row, 21)?,
                status: observed_status(row, 22)?,
                state_change_sequence: row_u64(row, 23)?,
                revision: row_u64(row, 24)?,
                version: row_u64(row, 25)?,
                last_observed_at_unix_ms: row_u64(row, 26)?,
            })
        })
        .transpose()?;
    let desired_state = match row.get::<_, String>(3)?.as_str() {
        "running" => WorkerDesiredState::Running,
        "ended" => WorkerDesiredState::Ended,
        value => return Err(enum_conversion_error(3, "worker desired state", value)),
    };
    let profile_id = row.get::<_, Option<String>>(1)?;
    let yard_orchestrator_worker_id = row.get::<_, Option<String>>(27)?;
    let orchestrator_project_id = row.get::<_, Option<String>>(28)?;
    let active_project_id = row.get::<_, Option<String>>(29)?;
    let assignment_id = row.get::<_, Option<String>>(30)?;
    let coordination_node_id = row.get::<_, Option<String>>(31)?;
    let (availability, project_id, reason) = worker_availability(
        desired_state,
        yard_orchestrator_worker_id.is_some(),
        coordination_node_id,
        orchestrator_project_id,
        active_project_id,
        profile_id.as_deref(),
        runtime.as_ref(),
    );
    Ok(WorkerCandidate {
        worker: Worker {
            id: row.get(0)?,
            profile_id,
            profile_version: row_optional_u64(row, 2)?,
            desired_state,
            runtime,
            version: row_u64(row, 4)?,
            created_at_unix_ms: row_u64(row, 5)?,
            updated_at_unix_ms: row_u64(row, 6)?,
        },
        profile_name: row.get(7)?,
        default_role: row.get(8)?,
        availability,
        project_id,
        assignment_id,
        reason,
    })
}

fn worker_availability(
    desired_state: WorkerDesiredState,
    is_yard_orchestrator: bool,
    coordination_node_id: Option<String>,
    orchestrator_project_id: Option<String>,
    active_project_id: Option<String>,
    profile_id: Option<&str>,
    runtime: Option<&WorkerRuntimeBinding>,
) -> (WorkerAvailability, Option<String>, Option<String>) {
    if desired_state == WorkerDesiredState::Ended {
        return (
            WorkerAvailability::Ended,
            None,
            Some("Session ended by explicit user action".to_owned()),
        );
    }
    if is_yard_orchestrator {
        return (
            WorkerAvailability::YardOrchestrator,
            None,
            Some("Worker is the required Yard orchestrator".to_owned()),
        );
    }
    if let Some(node_id) = coordination_node_id {
        return (
            WorkerAvailability::CoordinationNode,
            None,
            Some(format!("Worker is reserved by coordination node {node_id}")),
        );
    }
    if let Some(project_id) = orchestrator_project_id {
        return (
            WorkerAvailability::Orchestrator,
            Some(project_id),
            Some("Worker is the project's required orchestrator".to_owned()),
        );
    }
    if let Some(project_id) = active_project_id {
        return (
            WorkerAvailability::Assigned,
            Some(project_id),
            Some("Worker already has an active project allocation".to_owned()),
        );
    }
    if runtime
        .is_some_and(|runtime| runtime.observation_state == RuntimeObservationState::Ambiguous)
    {
        return (
            WorkerAvailability::Ambiguous,
            None,
            Some("Runtime identity is ambiguous and requires manual resolution".to_owned()),
        );
    }
    let is_live = runtime.is_some_and(|runtime| {
        runtime.observation_state == RuntimeObservationState::Observed
            && runtime.process_state == RuntimeProcessState::Running
            && runtime.provider_session.is_some()
    });
    if is_live {
        return (WorkerAvailability::UnassignedLive, None, None);
    }
    if profile_id.is_some_and(|profile_id| profile_id != BLANK_WORKER_PROFILE_ID) {
        let reason = match runtime {
            None => "Runtime can be restored from the worker's pinned profile",
            Some(runtime) if runtime.process_state == RuntimeProcessState::Exited => {
                "Runtime process exited and can be explicitly replaced"
            }
            Some(runtime) if runtime.observation_state == RuntimeObservationState::Missing => {
                "Runtime is missing and can be explicitly replaced"
            }
            Some(_) => "Runtime cannot be safely reused and can be explicitly replaced",
        };
        return (WorkerAvailability::Resumable, None, Some(reason.to_owned()));
    }
    (
        WorkerAvailability::Unavailable,
        None,
        Some("Worker has no stable live runtime or pinned profile for replacement".to_owned()),
    )
}

fn resolve_worker_allocation_profile(
    connection: &Connection,
    worker: &Worker,
    command: &ConfirmWorkerAllocation,
) -> Result<(String, u64), ProjectStoreError> {
    match (&worker.profile_id, worker.profile_version) {
        (Some(profile_id), Some(profile_version)) => {
            if command.profile_id.is_some() || command.expected_profile_version.is_some() {
                return Err(ProjectStoreError::WorkerProfileAlreadyPinned);
            }
            Ok((profile_id.clone(), profile_version))
        }
        (None, None) => {
            if let (Some(profile_id), Some(expected_version)) =
                (&command.profile_id, command.expected_profile_version)
            {
                let current_version = select_current_profile_version(connection, profile_id)?;
                if current_version != expected_version {
                    return Err(ProjectStoreError::ProfileVersionConflict { current_version });
                }
                Ok((profile_id.clone(), expected_version))
            } else {
                ensure_blank_worker_profile(connection)?;
                Ok((BLANK_WORKER_PROFILE_ID.to_owned(), 1))
            }
        }
        _ => Err(ProjectStoreError::WorkerProfileRequired),
    }
}

fn ensure_blank_worker_profile(connection: &Connection) -> Result<(), ProjectStoreError> {
    if connection
        .query_row(
            "SELECT 1 FROM worker_profiles WHERE id = ?1",
            [BLANK_WORKER_PROFILE_ID],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Ok(());
    }

    let now = unix_time_ms()?;
    connection.execute(
        "INSERT INTO worker_profiles (
            id, name, current_version, created_at_unix_ms, updated_at_unix_ms
         ) VALUES (?1, ?2, 1, ?3, ?3)",
        params![
            BLANK_WORKER_PROFILE_ID,
            BLANK_WORKER_PROFILE_NAME,
            to_i64(now)?
        ],
    )?;
    connection.execute(
        "INSERT INTO worker_profile_revisions (
            profile_id, version, name, runtime_adapter, provider, model,
            default_role, instructions_ref, sandbox_policy, worktree_policy,
            permission_policy, completion_contract, created_at_unix_ms
         ) VALUES (
            ?1, 1, ?2, 'herdr', 'runtime_default', NULL,
            'worker', NULL, 'runtime_default', 'project_workspace',
            'runtime_default', 'manual_receipt', ?3
         )",
        params![
            BLANK_WORKER_PROFILE_ID,
            BLANK_WORKER_PROFILE_NAME,
            to_i64(now)?
        ],
    )?;
    Ok(())
}

struct AssignmentRecord {
    assignment: Assignment,
    allocation: WorkerAllocation,
}

#[allow(clippy::too_many_lines)]
fn assignment_record_from_row(row: &Row<'_>) -> rusqlite::Result<AssignmentRecord> {
    let runtime_adapter = row.get::<_, Option<String>>(20)?;
    let provider_session_source = row.get::<_, Option<String>>(26)?;
    let provider_session = provider_session_source
        .map(|source| -> rusqlite::Result<ProviderSessionRef> {
            Ok(ProviderSessionRef {
                source,
                provider: row.get(27)?,
                kind: row.get(28)?,
                value: row.get(29)?,
            })
        })
        .transpose()?;
    let runtime = runtime_adapter
        .map(|adapter| -> rusqlite::Result<WorkerRuntimeBinding> {
            Ok(WorkerRuntimeBinding {
                adapter,
                session: row.get(21)?,
                workspace_id: row.get(22)?,
                terminal_id: row.get(23)?,
                tab_id: row.get(24)?,
                pane_id: row.get(25)?,
                provider_session,
                owns_tab: row.get(30)?,
                observation_state: runtime_observation_state(row, 31)?,
                process_state: runtime_process_state(row, 32)?,
                status: observed_status(row, 33)?,
                state_change_sequence: row_u64(row, 34)?,
                revision: row_u64(row, 35)?,
                version: row_u64(row, 36)?,
                last_observed_at_unix_ms: row_u64(row, 37)?,
            })
        })
        .transpose()?;
    let desired_state = match row.get::<_, String>(16)?.as_str() {
        "running" => WorkerDesiredState::Running,
        "ended" => WorkerDesiredState::Ended,
        value => return Err(enum_conversion_error(16, "worker desired state", value)),
    };
    let isolation_policy = match row.get::<_, String>(8)?.as_str() {
        "project_workspace" => IsolationPolicy::ProjectWorkspace,
        value => return Err(enum_conversion_error(8, "isolation policy", value)),
    };
    let lifecycle = match row.get::<_, String>(9)?.as_str() {
        "allocating" => AssignmentLifecycle::Allocating,
        "active" => AssignmentLifecycle::Active,
        "handing_off" => AssignmentLifecycle::HandingOff,
        "handed_off" => AssignmentLifecycle::HandedOff,
        "completed" => AssignmentLifecycle::Completed,
        "failed" => AssignmentLifecycle::Failed,
        value => return Err(enum_conversion_error(9, "assignment lifecycle", value)),
    };
    let attempt_lifecycle = match row.get::<_, String>(40)?.as_str() {
        "starting" => AttemptLifecycle::Starting,
        "active" => AttemptLifecycle::Active,
        "handing_off" => AttemptLifecycle::HandingOff,
        "handed_off" => AttemptLifecycle::HandedOff,
        "completed" => AttemptLifecycle::Completed,
        "failed" => AttemptLifecycle::Failed,
        value => return Err(enum_conversion_error(40, "attempt lifecycle", value)),
    };
    let allocation_mode = match row.get::<_, String>(45)?.as_str() {
        "create_new" => AllocationMode::CreateNew,
        "adopt_existing" => AllocationMode::AdoptExisting,
        "handoff" => AllocationMode::Handoff,
        value => return Err(enum_conversion_error(45, "allocation mode", value)),
    };
    let assignment_id = row.get::<_, String>(0)?;
    let worker_id = row.get::<_, String>(13)?;
    let allocation_id = row.get::<_, String>(2)?;
    let project_id = row.get::<_, String>(1)?;

    Ok(AssignmentRecord {
        allocation: WorkerAllocation {
            id: allocation_id.clone(),
            project_id: project_id.clone(),
            worker_id: worker_id.clone(),
            mode: allocation_mode,
            started_by_command_id: row.get(46)?,
            started_at_unix_ms: row_u64(row, 47)?,
        },
        assignment: Assignment {
            id: assignment_id.clone(),
            project_id,
            allocation_id,
            worker: Worker {
                id: worker_id,
                profile_id: row.get(14)?,
                profile_version: row_optional_u64(row, 15)?,
                desired_state,
                runtime,
                version: row_u64(row, 17)?,
                created_at_unix_ms: row_u64(row, 18)?,
                updated_at_unix_ms: row_u64(row, 19)?,
            },
            profile_id: row.get(3)?,
            profile_version: row_u64(row, 4)?,
            profile_name: row.get(5)?,
            objective: row.get(6)?,
            role: row.get(7)?,
            isolation_policy,
            lifecycle,
            attempt: AssignmentAttempt {
                id: row.get(38)?,
                assignment_id,
                ordinal: row.get::<_, u32>(39).map_err(|error| {
                    enum_conversion_error(39, "attempt ordinal", &error.to_string())
                })?,
                lifecycle: attempt_lifecycle,
                error: row.get(41)?,
                version: row_u64(row, 42)?,
                created_at_unix_ms: row_u64(row, 43)?,
                updated_at_unix_ms: row_u64(row, 44)?,
            },
            completion_receipt: None,
            version: row_u64(row, 10)?,
            created_at_unix_ms: row_u64(row, 11)?,
            updated_at_unix_ms: row_u64(row, 12)?,
        },
    })
}

fn select_confirmed_allocation(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<ConfirmedAllocation, ProjectStoreError> {
    let assignment_id = connection
        .query_row(
            "SELECT result_assignment_id
               FROM profile_allocation_commands
              WHERE command_id = ?1",
            [command_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
    let mut record = select_assignment_record(connection, &assignment_id)?
        .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
    record.assignment.completion_receipt = select_completion_receipt(connection, &assignment_id)?;
    Ok(ConfirmedAllocation {
        command_id: command_id.to_owned(),
        allocation: record.allocation,
        assignment: record.assignment,
        replayed,
    })
}

fn select_confirmed_worker_allocation(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<ConfirmedAllocation, ProjectStoreError> {
    let assignment_id = connection
        .query_row(
            "SELECT result_assignment_id
               FROM worker_allocation_commands
              WHERE command_id = ?1",
            [command_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
    let mut record = select_assignment_record(connection, &assignment_id)?
        .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
    record.assignment.completion_receipt = select_completion_receipt(connection, &assignment_id)?;
    Ok(ConfirmedAllocation {
        command_id: command_id.to_owned(),
        allocation: record.allocation,
        assignment: record.assignment,
        replayed,
    })
}

fn select_confirmed_worker_handoff(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<ConfirmedWorkerHandoff, ProjectStoreError> {
    let command = select_worker_handoff_command(connection, command_id)?
        .ok_or(ProjectStoreError::CommandNotFound)?;
    let assignment_id = command
        .result_assignment_id
        .as_deref()
        .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
    let mut target_record = select_assignment_record(connection, assignment_id)?
        .ok_or(ProjectStoreError::RuntimeAllocationMissing)?;
    if command.result_allocation_id.as_deref() != Some(target_record.allocation.id.as_str()) {
        return Err(ProjectStoreError::RuntimeAllocationMissing);
    }
    if target_record.assignment.project_id != command.target_project_id
        || target_record.assignment.worker.id != command.worker_id
        || target_record.allocation.project_id != command.target_project_id
        || target_record.allocation.worker_id != command.worker_id
    {
        return Err(ProjectStoreError::RuntimeAllocationMissing);
    }
    target_record.assignment.completion_receipt =
        select_completion_receipt(connection, assignment_id)?;
    let mut source_record = select_assignment_record(connection, &command.source_assignment_id)?
        .ok_or(ProjectStoreError::AssignmentNotFound)?;
    if source_record.assignment.project_id != command.source_project_id
        || source_record.assignment.worker.id != command.worker_id
    {
        return Err(ProjectStoreError::AssignmentNotFound);
    }
    source_record.assignment.completion_receipt =
        select_completion_receipt(connection, &command.source_assignment_id)?;
    Ok(ConfirmedWorkerHandoff {
        command_id: command_id.to_owned(),
        source_assignment: source_record.assignment,
        allocation: target_record.allocation,
        assignment: target_record.assignment,
        target_role: command.target_role,
        replaced_orchestrator_worker_id: command.replaced_orchestrator_worker_id,
        replayed,
    })
}

fn select_assignment_record(
    connection: &Connection,
    assignment_id: &str,
) -> Result<Option<AssignmentRecord>, ProjectStoreError> {
    connection
        .query_row(
            &format!("{ASSIGNMENT_SELECT} WHERE a.id = ?1"),
            [assignment_id],
            assignment_record_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn validate_prompt_assignment(
    assignment: &Assignment,
    project_id: &str,
    command: &SendAssignmentPrompt,
) -> Result<(), ProjectStoreError> {
    if assignment.project_id != project_id {
        return Err(ProjectStoreError::AssignmentNotFound);
    }
    if assignment.lifecycle != AssignmentLifecycle::Active {
        return Err(ProjectStoreError::AssignmentNotActive);
    }
    if assignment.version != command.expected_assignment_version {
        return Err(ProjectStoreError::AssignmentVersionConflict {
            current_version: assignment.version,
        });
    }
    if assignment.attempt.id != command.attempt_id {
        return Err(ProjectStoreError::AttemptNotCurrent {
            current_attempt_id: assignment.attempt.id.clone(),
        });
    }
    if assignment.attempt.lifecycle != AttemptLifecycle::Active {
        return Err(ProjectStoreError::AttemptNotActive);
    }
    if assignment.attempt.version != command.expected_attempt_version {
        return Err(ProjectStoreError::AttemptVersionConflict {
            current_version: assignment.attempt.version,
        });
    }
    Ok(())
}

#[derive(Debug)]
struct StoredAssignmentPrompt {
    project_id: String,
    assignment_id: String,
    attempt_id: String,
    expected_assignment_version: u64,
    expected_attempt_version: u64,
    text: String,
    actor: String,
    status: String,
    error_message: Option<String>,
}

impl StoredAssignmentPrompt {
    fn matches(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: &SendAssignmentPrompt,
    ) -> bool {
        self.project_id == project_id
            && self.assignment_id == assignment_id
            && self.attempt_id == command.attempt_id
            && self.expected_assignment_version == command.expected_assignment_version
            && self.expected_attempt_version == command.expected_attempt_version
            && self.text == command.text
            && self.actor == command.actor
    }
}

fn select_assignment_prompt_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredAssignmentPrompt>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT apc.project_id, apc.assignment_id, apc.attempt_id,
                    apc.expected_assignment_version,
                    apc.expected_attempt_version, apc.prompt_text,
                    ca.actor, ca.status, ca.error_message
               FROM assignment_prompt_commands apc
               JOIN command_acknowledgements ca ON ca.id = apc.command_id
              WHERE apc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredAssignmentPrompt {
                    project_id: row.get(0)?,
                    assignment_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    expected_assignment_version: row_u64(row, 3)?,
                    expected_attempt_version: row_u64(row, 4)?,
                    text: row.get(5)?,
                    actor: row.get(6)?,
                    status: row.get(7)?,
                    error_message: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_prompt_acknowledgement(
    connection: &Connection,
    command_id: &str,
) -> Result<yard_domain::PromptAcknowledgement, ProjectStoreError> {
    connection
        .query_row(
            "SELECT command_id, assignment_id, attempt_id,
                    result_runtime_status, submitted_at_unix_ms
               FROM assignment_prompt_commands
              WHERE command_id = ?1
                AND result_runtime_status IS NOT NULL
                AND submitted_at_unix_ms IS NOT NULL",
            [command_id],
            |row| {
                Ok(yard_domain::PromptAcknowledgement {
                    command_id: row.get(0)?,
                    assignment_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    runtime_status: row.get(3)?,
                    submitted_at_unix_ms: row_u64(row, 4)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::PromptAcknowledgementNotFound)
}

#[derive(Debug)]
struct StoredOrchestratorPrompt {
    project_id: String,
    orchestrator_worker_id: String,
    expected_project_version: u64,
    text: String,
    actor: String,
    status: String,
    error_message: Option<String>,
}

impl StoredOrchestratorPrompt {
    fn matches(&self, project_id: &str, command: &SendOrchestratorPrompt) -> bool {
        self.project_id == project_id
            && self.orchestrator_worker_id == command.orchestrator_worker_id
            && self.expected_project_version == command.expected_project_version
            && self.text == command.text
            && self.actor == command.actor
    }
}

fn select_orchestrator_prompt_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredOrchestratorPrompt>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT opc.project_id, opc.orchestrator_worker_id,
                    opc.expected_project_version,
                    opc.prompt_text, ca.actor, ca.status, ca.error_message
               FROM orchestrator_prompt_commands opc
               JOIN command_acknowledgements ca ON ca.id = opc.command_id
              WHERE opc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredOrchestratorPrompt {
                    project_id: row.get(0)?,
                    orchestrator_worker_id: row.get(1)?,
                    expected_project_version: row_u64(row, 2)?,
                    text: row.get(3)?,
                    actor: row.get(4)?,
                    status: row.get(5)?,
                    error_message: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_orchestrator_prompt_acknowledgement(
    connection: &Connection,
    command_id: &str,
) -> Result<OrchestratorPromptAcknowledgement, ProjectStoreError> {
    connection
        .query_row(
            "SELECT command_id, project_id, orchestrator_worker_id,
                    result_runtime_status, submitted_at_unix_ms
               FROM orchestrator_prompt_commands
              WHERE command_id = ?1
                AND result_runtime_status IS NOT NULL
                AND submitted_at_unix_ms IS NOT NULL",
            [command_id],
            |row| {
                Ok(OrchestratorPromptAcknowledgement {
                    command_id: row.get(0)?,
                    project_id: row.get(1)?,
                    worker_id: row.get(2)?,
                    runtime_status: row.get(3)?,
                    submitted_at_unix_ms: row_u64(row, 4)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::OrchestratorPromptAcknowledgementNotFound)
}

#[derive(Debug)]
struct StoredYardOrchestratorPrompt {
    orchestrator_worker_id: String,
    expected_orchestrator_version: u64,
    text: String,
    actor: String,
    status: String,
    error_message: Option<String>,
}

impl StoredYardOrchestratorPrompt {
    fn matches(&self, command: &SendYardOrchestratorPrompt) -> bool {
        self.orchestrator_worker_id == command.orchestrator_worker_id
            && self.expected_orchestrator_version == command.expected_orchestrator_version
            && self.text == command.text
            && self.actor == command.actor
    }
}

fn select_yard_orchestrator_prompt_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredYardOrchestratorPrompt>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT yopc.orchestrator_worker_id,
                    yopc.expected_orchestrator_version,
                    yopc.prompt_text, ca.actor, ca.status, ca.error_message
               FROM yard_orchestrator_prompt_commands yopc
               JOIN command_acknowledgements ca ON ca.id = yopc.command_id
              WHERE yopc.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredYardOrchestratorPrompt {
                    orchestrator_worker_id: row.get(0)?,
                    expected_orchestrator_version: row_u64(row, 1)?,
                    text: row.get(2)?,
                    actor: row.get(3)?,
                    status: row.get(4)?,
                    error_message: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_yard_orchestrator_prompt_acknowledgement(
    connection: &Connection,
    command_id: &str,
) -> Result<YardOrchestratorPromptAcknowledgement, ProjectStoreError> {
    connection
        .query_row(
            "SELECT command_id, orchestrator_worker_id,
                    result_runtime_status, submitted_at_unix_ms
               FROM yard_orchestrator_prompt_commands
              WHERE command_id = ?1
                AND result_runtime_status IS NOT NULL
                AND submitted_at_unix_ms IS NOT NULL",
            [command_id],
            |row| {
                Ok(YardOrchestratorPromptAcknowledgement {
                    command_id: row.get(0)?,
                    worker_id: row.get(1)?,
                    runtime_status: row.get(2)?,
                    submitted_at_unix_ms: row_u64(row, 3)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::YardOrchestratorPromptAcknowledgementNotFound)
}

fn select_yard_orchestrator_route(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<YardOrchestratorRoute>, ProjectStoreError> {
    connection
        .query_row(
            &format!("{YARD_ORCHESTRATOR_ROUTE_SELECT} WHERE yorc.command_id = ?1"),
            [command_id],
            yard_orchestrator_route_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn yard_orchestrator_route_from_row(row: &Row<'_>) -> rusqlite::Result<YardOrchestratorRoute> {
    Ok(YardOrchestratorRoute {
        command_id: row.get(0)?,
        actor: row.get(1)?,
        orchestrator_worker_id: row.get(2)?,
        expected_orchestrator_version: row_u64(row, 3)?,
        target_project_id: row.get(4)?,
        target_orchestrator_worker_id: row.get(5)?,
        expected_project_version: row_u64(row, 6)?,
        text: row.get(7)?,
        status: coordination_command_status_from_row(row, 8)?,
        error_message: row.get(9)?,
        runtime_status: row.get(10)?,
        created_at_unix_ms: row_u64(row, 11)?,
        updated_at_unix_ms: row_u64(row, 12)?,
        submitted_at_unix_ms: row_optional_u64(row, 13)?,
    })
}

fn yard_orchestrator_route_matches(
    existing: &YardOrchestratorRoute,
    command: &SendYardOrchestratorRoute,
) -> bool {
    existing.actor == command.actor
        && existing.orchestrator_worker_id == command.orchestrator_worker_id
        && existing.expected_orchestrator_version == command.expected_orchestrator_version
        && existing.target_project_id == command.target_project_id
        && existing.target_orchestrator_worker_id == command.target_orchestrator_worker_id
        && existing.expected_project_version == command.expected_project_version
        && existing.text == command.text
}

#[derive(Debug)]
struct StoredCompletionCommand {
    project_id: String,
    assignment_id: String,
    attempt_id: String,
    expected_assignment_version: u64,
    expected_attempt_version: u64,
    outcome: String,
    summary: String,
    actor: String,
    status: String,
    error_message: Option<String>,
    artifact_refs: Vec<String>,
    artifact_ids: Vec<String>,
    evidence_refs: Vec<String>,
    unresolved_blockers: Vec<String>,
}

impl StoredCompletionCommand {
    fn matches(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: &RecordCompletionReceipt,
    ) -> bool {
        self.project_id == project_id
            && self.assignment_id == assignment_id
            && self.attempt_id == command.attempt_id
            && self.expected_assignment_version == command.expected_assignment_version
            && self.expected_attempt_version == command.expected_attempt_version
            && self.outcome == "completed"
            && self.summary == command.summary
            && self.actor == command.actor
            && self.artifact_refs == command.artifact_refs
            && self.artifact_ids == command.artifact_ids
            && self.evidence_refs == command.evidence_refs
            && self.unresolved_blockers == command.unresolved_blockers
    }
}

fn select_completion_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredCompletionCommand>, ProjectStoreError> {
    let command = connection
        .query_row(
            "SELECT crc.project_id, crc.assignment_id, crc.attempt_id,
                    crc.expected_assignment_version,
                    crc.expected_attempt_version, crc.outcome, crc.summary,
                    ca.actor, ca.status, ca.error_message, cr.id
               FROM completion_receipt_commands crc
               JOIN command_acknowledgements ca ON ca.id = crc.command_id
               LEFT JOIN completion_receipts cr ON cr.command_id = crc.command_id
              WHERE crc.command_id = ?1",
            [command_id],
            |row| {
                Ok((
                    StoredCompletionCommand {
                        project_id: row.get(0)?,
                        assignment_id: row.get(1)?,
                        attempt_id: row.get(2)?,
                        expected_assignment_version: row_u64(row, 3)?,
                        expected_attempt_version: row_u64(row, 4)?,
                        outcome: row.get(5)?,
                        summary: row.get(6)?,
                        actor: row.get(7)?,
                        status: row.get(8)?,
                        error_message: row.get(9)?,
                        artifact_refs: Vec::new(),
                        artifact_ids: Vec::new(),
                        evidence_refs: Vec::new(),
                        unresolved_blockers: Vec::new(),
                    },
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((mut command, receipt_id)) = command else {
        return Ok(None);
    };
    if let Some(receipt_id) = receipt_id {
        command.artifact_refs =
            select_receipt_values(connection, "completion_receipt_artifacts", &receipt_id)?;
        command.artifact_ids = select_receipt_artifact_ids(connection, &receipt_id)?;
        command.evidence_refs =
            select_receipt_values(connection, "completion_receipt_evidence", &receipt_id)?;
        command.unresolved_blockers =
            select_receipt_values(connection, "completion_receipt_blockers", &receipt_id)?;
    }
    Ok(Some(command))
}

fn command_id_exists(connection: &Connection, command_id: &str) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM command_acknowledgements WHERE id = ?1
             )",
            [command_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn select_completion_receipt(
    connection: &Connection,
    assignment_id: &str,
) -> Result<Option<CompletionReceipt>, ProjectStoreError> {
    let receipt = connection
        .query_row(
            "SELECT id, assignment_id, attempt_id, outcome, summary, actor,
                    created_at_unix_ms
               FROM completion_receipts
              WHERE assignment_id = ?1",
            [assignment_id],
            |row| {
                let outcome = match row.get::<_, String>(3)?.as_str() {
                    "completed" => CompletionOutcome::Completed,
                    value => {
                        return Err(enum_conversion_error(3, "completion outcome", value));
                    }
                };
                Ok(CompletionReceipt {
                    id: row.get(0)?,
                    assignment_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    outcome,
                    summary: row.get(4)?,
                    artifact_refs: Vec::new(),
                    artifacts: Vec::new(),
                    evidence_refs: Vec::new(),
                    unresolved_blockers: Vec::new(),
                    actor: row.get(5)?,
                    created_at_unix_ms: row_u64(row, 6)?,
                })
            },
        )
        .optional()?;
    let Some(mut receipt) = receipt else {
        return Ok(None);
    };
    receipt.artifact_refs =
        select_receipt_values(connection, "completion_receipt_artifacts", &receipt.id)?;
    receipt.artifacts = select_receipt_artifacts(connection, &receipt.id)?;
    receipt.evidence_refs =
        select_receipt_values(connection, "completion_receipt_evidence", &receipt.id)?;
    receipt.unresolved_blockers =
        select_receipt_values(connection, "completion_receipt_blockers", &receipt.id)?;
    Ok(Some(receipt))
}

fn select_recorded_completion(
    connection: &Connection,
    command_id: &str,
    replayed: bool,
) -> Result<RecordedCompletionReceipt, ProjectStoreError> {
    let assignment_id = connection
        .query_row(
            "SELECT assignment_id
               FROM completion_receipt_commands
              WHERE command_id = ?1",
            [command_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(ProjectStoreError::CommandNotFound)?;
    let mut record = select_assignment_record(connection, &assignment_id)?
        .ok_or(ProjectStoreError::AssignmentNotFound)?;
    let receipt = select_completion_receipt(connection, &assignment_id)?
        .ok_or(ProjectStoreError::CompletionReceiptNotFound)?;
    record.assignment.completion_receipt = Some(receipt.clone());
    Ok(RecordedCompletionReceipt {
        command_id: command_id.to_owned(),
        receipt,
        assignment: record.assignment,
        replayed,
    })
}

fn artifact_kind_value(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Markdown => "markdown",
        ArtifactKind::Html => "html",
    }
}

fn select_artifact(
    connection: &Connection,
    artifact_id: &str,
) -> Result<Option<Artifact>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT id, project_id, assignment_id, attempt_id, worker_id,
                    kind, media_type, display_name, byte_size, sha256, source,
                    created_by, created_at_unix_ms
               FROM artifacts
              WHERE id = ?1",
            [artifact_id],
            artifact_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn artifact_from_row(row: &Row<'_>) -> rusqlite::Result<Artifact> {
    let kind = match row.get::<_, String>(5)?.as_str() {
        "markdown" => ArtifactKind::Markdown,
        "html" => ArtifactKind::Html,
        value => return Err(enum_conversion_error(5, "artifact kind", value)),
    };
    let source = match row.get::<_, String>(10)?.as_str() {
        "upload" => ArtifactSource::Upload,
        value => return Err(enum_conversion_error(10, "artifact source", value)),
    };
    Ok(Artifact {
        id: row.get(0)?,
        project_id: row.get(1)?,
        assignment_id: row.get(2)?,
        attempt_id: row.get(3)?,
        worker_id: row.get(4)?,
        kind,
        media_type: row.get(6)?,
        display_name: row.get(7)?,
        byte_size: row_u64(row, 8)?,
        sha256: row.get(9)?,
        source,
        created_by: row.get(11)?,
        created_at_unix_ms: row_u64(row, 12)?,
    })
}

fn artifact_matches_registration(
    artifact: &Artifact,
    project_id: &str,
    assignment_id: &str,
    registration: &ArtifactRegistration,
) -> bool {
    artifact.project_id == project_id
        && artifact.assignment_id == assignment_id
        && artifact.attempt_id == registration.attempt_id
        && artifact.kind == registration.kind
        && artifact.media_type == registration.kind.media_type()
        && artifact.display_name == registration.display_name
        && artifact.byte_size == registration.byte_size
        && artifact.sha256 == registration.sha256
        && artifact.source == ArtifactSource::Upload
        && artifact.created_by == registration.actor
}

fn insert_receipt_artifacts(
    transaction: &Transaction<'_>,
    receipt_id: &str,
    project_id: &str,
    assignment_id: &str,
    attempt_id: &str,
    artifact_ids: &[String],
) -> Result<(), ProjectStoreError> {
    for (position, artifact_id) in artifact_ids.iter().enumerate() {
        let artifact = select_artifact(transaction, artifact_id)?
            .ok_or(ProjectStoreError::ArtifactNotFound)?;
        if artifact.project_id != project_id
            || artifact.assignment_id != assignment_id
            || artifact.attempt_id != attempt_id
        {
            return Err(ProjectStoreError::ArtifactScopeMismatch);
        }
        transaction.execute(
            "INSERT INTO completion_receipt_artifact_links (
                receipt_id, position, artifact_id, assignment_id, attempt_id
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                receipt_id,
                i64::try_from(position)?,
                artifact_id,
                assignment_id,
                attempt_id,
            ],
        )?;
    }
    Ok(())
}

fn select_receipt_artifact_ids(
    connection: &Connection,
    receipt_id: &str,
) -> Result<Vec<String>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT artifact_id
           FROM completion_receipt_artifact_links
          WHERE receipt_id = ?1
          ORDER BY position",
    )?;
    statement
        .query_map([receipt_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn select_receipt_artifacts(
    connection: &Connection,
    receipt_id: &str,
) -> Result<Vec<Artifact>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT a.id, a.project_id, a.assignment_id, a.attempt_id, a.worker_id,
                a.kind, a.media_type, a.display_name, a.byte_size, a.sha256,
                a.source, a.created_by, a.created_at_unix_ms
           FROM completion_receipt_artifact_links l
           JOIN artifacts a ON a.id = l.artifact_id
          WHERE l.receipt_id = ?1
          ORDER BY l.position",
    )?;
    statement
        .query_map([receipt_id], artifact_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn insert_receipt_values(
    transaction: &Transaction<'_>,
    table: &str,
    receipt_id: &str,
    values: &[String],
) -> Result<(), ProjectStoreError> {
    let sql = format!("INSERT INTO {table} (receipt_id, position, value) VALUES (?1, ?2, ?3)");
    for (position, value) in values.iter().enumerate() {
        transaction.execute(&sql, params![receipt_id, i64::try_from(position)?, value])?;
    }
    Ok(())
}

fn select_receipt_values(
    connection: &Connection,
    table: &str,
    receipt_id: &str,
) -> Result<Vec<String>, ProjectStoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT value FROM {table} WHERE receipt_id = ?1 ORDER BY position"
    ))?;
    statement
        .query_map([receipt_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn insert_lifecycle_event(
    transaction: &Transaction<'_>,
    aggregate_type: &str,
    aggregate_id: &str,
    aggregate_version: u64,
    event_type: &str,
    source: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO lifecycle_events (
            aggregate_type, aggregate_id, aggregate_version,
            event_type, source, created_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            aggregate_type,
            aggregate_id,
            to_i64(aggregate_version)?,
            event_type,
            source,
            to_i64(now)?,
        ],
    )?;
    Ok(())
}

fn enum_conversion_error(index: usize, kind: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        Type::Text,
        format!("unsupported {kind} '{value}'").into(),
    )
}

fn provider_session_from_columns(
    row: &Row<'_>,
    start: usize,
) -> rusqlite::Result<Option<ProviderSessionRef>> {
    row.get::<_, Option<String>>(start)?
        .map(|source| {
            Ok(ProviderSessionRef {
                source,
                provider: row.get(start + 1)?,
                kind: row.get(start + 2)?,
                value: row.get(start + 3)?,
            })
        })
        .transpose()
}

fn handoff_target_role_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<HandoffTargetRole> {
    match row.get::<_, String>(index)?.as_str() {
        "member" => Ok(HandoffTargetRole::Member),
        "orchestrator" => Ok(HandoffTargetRole::Orchestrator),
        value => Err(enum_conversion_error(index, "handoff target role", value)),
    }
}

const fn handoff_target_role_value(role: HandoffTargetRole) -> &'static str {
    match role {
        HandoffTargetRole::Member => "member",
        HandoffTargetRole::Orchestrator => "orchestrator",
    }
}

fn project_relationship_kind_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<ProjectRelationshipKind> {
    match row.get::<_, String>(index)?.as_str() {
        "depends_on" => Ok(ProjectRelationshipKind::DependsOn),
        value => Err(enum_conversion_error(
            index,
            "project relationship kind",
            value,
        )),
    }
}

const fn project_relationship_kind_value(kind: ProjectRelationshipKind) -> &'static str {
    match kind {
        ProjectRelationshipKind::DependsOn => "depends_on",
    }
}

fn coordination_command_status_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<CoordinationCommandStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "pending" => Ok(CoordinationCommandStatus::Pending),
        "succeeded" => Ok(CoordinationCommandStatus::Submitted),
        "failed" => Ok(CoordinationCommandStatus::Failed),
        "ambiguous" => Ok(CoordinationCommandStatus::Ambiguous),
        value => Err(enum_conversion_error(
            index,
            "coordination command status",
            value,
        )),
    }
}

fn runtime_observation_state(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<RuntimeObservationState> {
    match row.get::<_, String>(index)?.as_str() {
        "observed" => Ok(RuntimeObservationState::Observed),
        "missing" => Ok(RuntimeObservationState::Missing),
        "ambiguous" => Ok(RuntimeObservationState::Ambiguous),
        value => Err(enum_conversion_error(
            index,
            "runtime observation state",
            value,
        )),
    }
}

const fn runtime_observation_state_value(state: RuntimeObservationState) -> &'static str {
    match state {
        RuntimeObservationState::Observed => "observed",
        RuntimeObservationState::Missing => "missing",
        RuntimeObservationState::Ambiguous => "ambiguous",
    }
}

fn runtime_process_state(row: &Row<'_>, index: usize) -> rusqlite::Result<RuntimeProcessState> {
    match row.get::<_, String>(index)?.as_str() {
        "running" => Ok(RuntimeProcessState::Running),
        "exited" => Ok(RuntimeProcessState::Exited),
        "unknown" => Ok(RuntimeProcessState::Unknown),
        value => Err(enum_conversion_error(index, "runtime process state", value)),
    }
}

const fn runtime_process_state_value(state: RuntimeProcessState) -> &'static str {
    match state {
        RuntimeProcessState::Running => "running",
        RuntimeProcessState::Exited => "exited",
        RuntimeProcessState::Unknown => "unknown",
    }
}

fn observed_status(row: &Row<'_>, index: usize) -> rusqlite::Result<ObservedStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "idle" => Ok(ObservedStatus::Idle),
        "working" => Ok(ObservedStatus::Working),
        "blocked" => Ok(ObservedStatus::Blocked),
        "done" => Ok(ObservedStatus::Done),
        "unknown" => Ok(ObservedStatus::Unknown),
        value => Err(enum_conversion_error(index, "observed status", value)),
    }
}

const fn observed_status_value(status: ObservedStatus) -> &'static str {
    match status {
        ObservedStatus::Idle => "idle",
        ObservedStatus::Working => "working",
        ObservedStatus::Blocked => "blocked",
        ObservedStatus::Done => "done",
        ObservedStatus::Unknown => "unknown",
    }
}

fn required_id(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProjectStoreError::ProjectNotFound)
    } else {
        Ok(value.to_owned())
    }
}

fn required_assignment_id(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProjectStoreError::AssignmentNotFound)
    } else {
        Ok(value.to_owned())
    }
}

fn required_relationship_id(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(yard_domain::CoordinationValidationError::Required {
            field: "relationship_id",
        }
        .into());
    }
    if value.len() > 120 {
        return Err(yard_domain::CoordinationValidationError::TooLong {
            field: "relationship_id",
            max_bytes: 120,
        }
        .into());
    }
    Ok(value.to_owned())
}

fn required_profile_id(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProjectStoreError::ProfileNotFound)
    } else {
        Ok(value.to_owned())
    }
}

fn required_command_id(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProjectStoreError::CommandNotFound)
    } else {
        Ok(value.to_owned())
    }
}

fn required_runtime_status(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 120 {
        Err(ProjectStoreError::RuntimeStatusInvalid)
    } else {
        Ok(value.to_owned())
    }
}

fn row_u64(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Integer, Box::new(error))
    })
}

fn row_optional_u64(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            u64::try_from(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(index, Type::Integer, Box::new(error))
            })
        })
        .transpose()
}

fn unix_time_ms() -> Result<u64, ProjectStoreError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

fn to_i64(value: u64) -> Result<i64, ProjectStoreError> {
    Ok(value.try_into()?)
}

fn to_u64(value: i64) -> Result<u64, ProjectStoreError> {
    value.try_into().map_err(ProjectStoreError::from)
}

fn is_unique_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    )
}

fn map_handoff_reservation_error(error: rusqlite::Error) -> ProjectStoreError {
    if is_unique_constraint(&error) {
        ProjectStoreError::HandoffTargetReserved
    } else {
        error.into()
    }
}

fn map_handoff_claim_error(error: rusqlite::Error) -> ProjectStoreError {
    if is_unique_constraint(&error) {
        ProjectStoreError::RuntimeHandoffClaimConflict
    } else {
        error.into()
    }
}

#[derive(Debug, Error)]
pub enum ProjectStoreError {
    #[error("SQLite operation failed: {0}")]
    Database(rusqlite::Error),
    #[error("SQLite database is busy")]
    DatabaseBusy,
    #[error("another Yard process already owns this database")]
    DatabaseAlreadyOpen,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Task(#[from] task::JoinError),
    #[error(transparent)]
    Clock(#[from] SystemTimeError),
    #[error(transparent)]
    IntegerConversion(#[from] TryFromIntError),
    #[error(transparent)]
    InvalidProject(#[from] yard_domain::ProjectValidationError),
    #[error(transparent)]
    InvalidProfile(#[from] yard_domain::ProfileValidationError),
    #[error(transparent)]
    InvalidAssignment(#[from] yard_domain::AssignmentValidationError),
    #[error(transparent)]
    InvalidCompletionReceipt(#[from] yard_domain::CompletionValidationError),
    #[error(transparent)]
    InvalidIntervention(#[from] yard_domain::InterventionValidationError),
    #[error(transparent)]
    InvalidWorkerSession(#[from] yard_domain::WorkerSessionValidationError),
    #[error(transparent)]
    InvalidYardOrchestrator(#[from] yard_domain::YardOrchestratorValidationError),
    #[error(transparent)]
    InvalidCoordination(#[from] yard_domain::CoordinationValidationError),
    #[error(transparent)]
    InvalidCoordinationNode(#[from] yard_domain::CoordinationNodeValidationError),
    #[error(transparent)]
    InvalidAutomation(#[from] yard_domain::AutomationValidationError),
    #[error("automation was not found")]
    AutomationNotFound,
    #[error("an automation with this ID already exists or existed")]
    AutomationIdConflict,
    #[error("automation version conflict; current version is {current_version}")]
    AutomationVersionConflict { current_version: u64 },
    #[error("automation placement version conflict; current version is {current_version}")]
    AutomationPlacementVersionConflict { current_version: u64 },
    #[error("automation scope project was not found")]
    AutomationScopeProjectNotFound,
    #[error("automation scope coordination node was not found")]
    AutomationScopeNodeNotFound,
    #[error("automation scope requires a workstream coordination node")]
    AutomationScopeNodeKindMismatch,
    #[error("selected automation project {project_id} was not found")]
    AutomationSelectedProjectNotFound { project_id: String },
    #[error("selected automation project is not attached to its workstream")]
    AutomationProjectNotAttached,
    #[error("active automations require a service-computed next run and paused automations do not")]
    AutomationNextRunInvalid,
    #[error("automation is paused")]
    AutomationPaused,
    #[error("automation schedule changed; current next run is {current_next_run_at:?}")]
    AutomationScheduleConflict { current_next_run_at: Option<u64> },
    #[error("automation run was not found")]
    AutomationRunNotFound,
    #[error("an automation run with this ID already exists or existed")]
    AutomationRunIdConflict,
    #[error("automation dispatch command ID is already reserved")]
    AutomationDispatchCommandIdConflict,
    #[error("an automation run is already pending")]
    AutomationRunInProgress,
    #[error("automation list limit must contain between 1 and {max} records")]
    AutomationListLimitInvalid { max: usize },
    #[error("automation run submission time precedes the run claim")]
    AutomationSubmittedAtInvalid,
    #[error("coordination node was not found")]
    CoordinationNodeNotFound,
    #[error("a coordination node with this ID already exists or existed")]
    CoordinationNodeIdConflict,
    #[error("coordination node kind does not support this operation")]
    CoordinationNodeKindMismatch,
    #[error("coordination node managed path does not match its kind")]
    CoordinationNodeManagedPathInvalid,
    #[error("coordination node version conflict; current version is {current_version}")]
    CoordinationNodeVersionConflict { current_version: u64 },
    #[error("coordination node placement version conflict; current version is {current_version}")]
    CoordinationNodePlacementVersionConflict { current_version: u64 },
    #[error("coordination node is already provisioned")]
    CoordinationNodeAlreadyProvisioned,
    #[error("coordination node has not been provisioned")]
    CoordinationNodeNotProvisioned,
    #[error("coordination node worker changed; current worker is {current_worker_id}")]
    CoordinationNodeWorkerChanged { current_worker_id: String },
    #[error("a coordination node intervention is still in progress")]
    CoordinationNodeInterventionInProgress,
    #[error("target project is not attached to this coordination node")]
    CoordinationNodeProjectNotAttached,
    #[error("attached project {project_id} was not found")]
    AttachedProjectNotFound { project_id: String },
    #[error("worker profile revision does not match the provision command")]
    WorkerProfileRevisionMismatch,
    #[error("the coordination node worker session cannot end")]
    CoordinationNodeSessionEndForbidden,
    #[error("coordination snapshot was not found")]
    CoordinationSnapshotNotFound,
    #[error("a coordination snapshot with this ID already exists")]
    CoordinationSnapshotIdConflict,
    #[error("snapshot projects do not exactly match the node attachments")]
    SnapshotAttachmentMismatch,
    #[error("snapshot project record was not found")]
    SnapshotProjectNotFound,
    #[error("project was not found")]
    ProjectNotFound,
    #[error("the relationship source project was not found")]
    RelationshipSourceProjectNotFound,
    #[error("the relationship target project was not found")]
    RelationshipTargetProjectNotFound,
    #[error("project relationship was not found")]
    ProjectRelationshipNotFound,
    #[error("a project relationship with this ID already exists or existed")]
    ProjectRelationshipIdConflict,
    #[error("this typed project relationship already exists")]
    ProjectRelationshipAlreadyExists,
    #[error("project relationship version conflict; current version is {current_version}")]
    ProjectRelationshipVersionConflict { current_version: u64 },
    #[error("worker profile was not found")]
    ProfileNotFound,
    #[error("worker was not found")]
    WorkerNotFound,
    #[error("a worker profile with this name already exists")]
    ProfileNameAlreadyExists,
    #[error("worker profile version conflict; current version is {current_version}")]
    ProfileVersionConflict { current_version: u64 },
    #[error("worker version conflict; current version is {current_version}")]
    WorkerVersionConflict { current_version: u64 },
    #[error("worker runtime version conflict; current version is {current_version:?}")]
    WorkerRuntimeVersionConflict { current_version: Option<u64> },
    #[error("the worker session has already ended")]
    WorkerAlreadyEnded,
    #[error("the project orchestrator session cannot end without a replacement")]
    OrchestratorSessionEndForbidden,
    #[error("the Yard orchestrator session cannot end without a replacement")]
    YardOrchestratorSessionEndForbidden,
    #[error("the worker has an active project allocation")]
    WorkerHasActiveAllocation,
    #[error("worker is not allocatable while availability is {availability:?}")]
    WorkerNotAvailable { availability: WorkerAvailability },
    #[error("a profile selection is required for this adopted worker")]
    WorkerProfileRequired,
    #[error("the worker already has a pinned profile revision")]
    WorkerProfileAlreadyPinned,
    #[error("the worker has an invalid partial profile binding")]
    InvalidWorkerProfileBinding,
    #[error("assignment was not found")]
    AssignmentNotFound,
    #[error("assignment is not active")]
    AssignmentNotActive,
    #[error("assignment version conflict; current version is {current_version}")]
    AssignmentVersionConflict { current_version: u64 },
    #[error("assignment attempt is not active")]
    AttemptNotActive,
    #[error("assignment attempt version conflict; current version is {current_version}")]
    AttemptVersionConflict { current_version: u64 },
    #[error("assignment attempt is stale; current attempt is {current_attempt_id}")]
    AttemptNotCurrent { current_attempt_id: String },
    #[error("a worker cannot be handed off to its current project")]
    HandoffTargetMatchesSource,
    #[error("project orchestrators cannot be moved through worker handoff")]
    OrchestratorHandoffForbidden,
    #[error("project orchestrator changed; current worker is {current_worker_id}")]
    OrchestratorNotCurrent { current_worker_id: String },
    #[error("an orchestrator intervention or replacement is still in progress")]
    OrchestratorInterventionInProgress,
    #[error("the target project already has an unfinished orchestrator handoff")]
    HandoffTargetReserved,
    #[error("the source assignment or runtime changed during handoff")]
    HandoffSourceChanged,
    #[error("the target project's orchestrator changed during handoff")]
    TargetOrchestratorChanged,
    #[error("the handoff command has not claimed a target runtime")]
    RuntimeHandoffClaimMissing,
    #[error("the handoff target runtime conflicts with another durable identity")]
    RuntimeHandoffClaimConflict,
    #[error("an assignment intervention is still in progress")]
    AssignmentInterventionInProgress,
    #[error("completion receipt was not found")]
    CompletionReceiptNotFound,
    #[error("artifact was not found")]
    ArtifactNotFound,
    #[error("artifact ID is already associated with different content or provenance")]
    ArtifactIdConflict,
    #[error("artifact does not belong to the completion assignment and attempt")]
    ArtifactScopeMismatch,
    #[error("prompt acknowledgement was not found")]
    PromptAcknowledgementNotFound,
    #[error("orchestrator prompt acknowledgement was not found")]
    OrchestratorPromptAcknowledgementNotFound,
    #[error("Yard orchestrator prompt acknowledgement was not found")]
    YardOrchestratorPromptAcknowledgementNotFound,
    #[error("runtime cleanup job was not found")]
    RuntimeCleanupNotFound,
    #[error("project version conflict; current version is {current_version}")]
    ProjectVersionConflict { current_version: u64 },
    #[error("Yard orchestrator version conflict; current version is {current_version}")]
    YardOrchestratorVersionConflict { current_version: u64 },
    #[error("the Yard orchestrator has not been configured")]
    YardOrchestratorNotConfigured,
    #[error("Yard orchestrator changed; current worker is {current_worker_id}")]
    YardOrchestratorNotCurrent { current_worker_id: String },
    #[error("a Yard orchestrator intervention is still in progress")]
    YardOrchestratorInterventionInProgress,
    #[error("the Herdr workspace is already bound to a Yard project")]
    RuntimeWorkspaceAlreadyBound,
    #[error("the Herdr workspace is reserved by another project creation command")]
    RuntimeWorkspaceReserved,
    #[error("the observed worker is already bound to a Yard worker")]
    RuntimeWorkerAlreadyBound,
    #[error("the worker runtime does not belong to the target project workspace")]
    RuntimeWorkspaceMismatch,
    #[error("the worker allocation does not require runtime replacement")]
    RuntimeReplacementNotRequired,
    #[error("a runtime binding was created concurrently")]
    RuntimeBindingAlreadyExists,
    #[error("the runtime snapshot is older than the durable reconciliation watermark")]
    StaleRuntimeSnapshot,
    #[error("runtime binding changed during reconciliation")]
    RuntimeBindingVersionConflict,
    #[error("project placement version conflict; current version is {current_version}")]
    VersionConflict { current_version: u64 },
    #[error("project placement version cannot be incremented")]
    VersionOverflow,
    #[error("command ID is already associated with different input")]
    IdempotencyConflict,
    #[error("command is still in progress")]
    CommandInProgress,
    #[error("command previously failed: {0}")]
    CommandPreviouslyFailed(String),
    #[error("command outcome is ambiguous: {0}")]
    CommandOutcomeAmbiguous(String),
    #[error("command was not found")]
    CommandNotFound,
    #[error("command is not pending")]
    CommandNotPending,
    #[error("allocation command does not have a durable runtime allocation")]
    RuntimeAllocationMissing,
    #[error("project creation command does not have a durable project result")]
    ProjectCreationResultMissing,
    #[error("assignment worker has no runtime binding")]
    RuntimeBindingMissing,
    #[error("runtime inventory adapter and session are required")]
    InvalidRuntimeInventory,
    #[error("runtime status must contain between 1 and 120 bytes")]
    RuntimeStatusInvalid,
    #[error("route list limit must contain between 1 and {max} records")]
    RouteListLimitInvalid { max: usize },
    #[error("allocation command failure message is required")]
    CommandFailureMessageRequired,
    #[error("database foreign key validation failed after migration")]
    ForeignKeyCheckFailed,
    #[error("database schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: i64, supported: i64 },
}

impl From<rusqlite::Error> for ProjectStoreError {
    fn from(error: rusqlite::Error) -> Self {
        match &error {
            rusqlite::Error::SqliteFailure(failure, _)
                if matches!(
                    failure.code,
                    ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
                ) =>
            {
                Self::DatabaseBusy
            }
            _ => Self::Database(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use rusqlite::Connection;
    use tempfile::TempDir;
    use uuid::Uuid;
    use yard_domain::{
        AllocationMode, ArtifactKind, ArtifactRegistration, AssignmentLifecycle, AttemptLifecycle,
        CanvasPlacement, CompletionOutcome, ConfigureYardOrchestrator, ConfirmProfileAllocation,
        ConfirmWorkerAllocation, ConfirmWorkerHandoff, CoordinationCommandStatus,
        CoordinationNodeKind, CreateCoordinationNode, CreateProject, CreateProjectFromProfile,
        CreateProjectRelationship, CreateWorkerProfile, CreateWorkspaceProjectFromProfile,
        DeleteProjectRelationship, EndWorkerSession, FocusObservation, HandoffTargetRole,
        IsolationPolicy, ObservedStatus, ObservedWorker, PaneObservation, Project,
        ProjectRelationshipKind, ProjectRuntimeBinding, ProviderSessionRef,
        ProvisionCoordinationNode, RecordCompletionReceipt, RequestCoordinationSnapshot,
        RuntimeInventory, RuntimeObservationState, RuntimeProcessState, SendAssignmentPrompt,
        SendCoordinationNodePrompt, SendCoordinationNodeRoute, SendOrchestratorPrompt,
        SendYardOrchestratorPrompt, SendYardOrchestratorRoute, SnapshotCollectionStatus,
        UpdateCoordinationNode, UpdateCoordinationNodePlacement, UpdateProjectPlacement,
        UpdateWorkerProfile, WorkerAvailability, WorkerProfile, WorkerProfileSpec,
        WorkerRuntimeBinding, YardOrchestrator,
    };

    use super::{
        BLANK_WORKER_PROFILE_ID, BLANK_WORKER_PROFILE_NAME, BeginAssignmentPrompt,
        BeginCoordinationNodePrompt, BeginCoordinationNodeRoute, BeginOrchestratorPrompt,
        BeginProfileAllocation, BeginProfileProjectCreation, BeginWorkerAllocation,
        BeginWorkerHandoff, BeginWorkspaceProjectCreation, BeginYardOrchestratorPrompt,
        BeginYardOrchestratorRoute, COMPLETION_RECEIPT_MIGRATION, INITIAL_MIGRATION,
        PROFILE_ASSIGNMENT_MIGRATION, ProjectStoreError, SCHEMA_VERSION, SnapshotDeliveryResult,
        SnapshotProjectFolder, SqliteProjectStore, YardStore,
    };

    fn draft(workspace_id: &str, terminal_id: &str) -> (CreateProject, WorkerRuntimeBinding) {
        (
            CreateProject {
                name: "Runtime API".to_owned(),
                runtime: ProjectRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "default".to_owned(),
                    workspace_id: workspace_id.to_owned(),
                },
                orchestrator_observed_worker_id: terminal_id.to_owned(),
                placement: CanvasPlacement {
                    x: 80.0,
                    y: 70.0,
                    width: 322.0,
                    height: 240.0,
                },
            },
            WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "default".to_owned(),
                workspace_id: workspace_id.to_owned(),
                terminal_id: terminal_id.to_owned(),
                tab_id: Some("tab-1".to_owned()),
                pane_id: "pane-1".to_owned(),
                provider_session: None,
                owns_tab: false,
                observation_state: RuntimeObservationState::Observed,
                process_state: RuntimeProcessState::Running,
                status: ObservedStatus::Idle,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 1,
            },
        )
    }

    async fn open_store(temp: &TempDir) -> SqliteProjectStore {
        SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
            .await
            .unwrap()
    }

    fn drop_automation_schema_for_downgrade(connection: &Connection) {
        connection
            .execute_batch(
                "DROP TABLE automation_run_now_commands;
                 DROP TABLE automation_run_projects;
                 DROP TABLE automation_runs;
                 DROP TABLE automation_pause_commands;
                 DROP TABLE automation_placement_commands;
                 DROP TABLE automation_update_command_projects;
                 DROP TABLE automation_update_commands;
                 DROP TABLE automation_create_command_projects;
                 DROP TABLE automation_create_commands;
                 DROP TABLE automation_selected_projects;
                 DROP TABLE automation_placements;
                 DROP TABLE automations;",
            )
            .unwrap();
    }

    #[tokio::test]
    async fn database_path_has_one_store_owner() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&path).await.unwrap();

        let error = SqliteProjectStore::open(&path).await.unwrap_err();
        assert!(matches!(error, ProjectStoreError::DatabaseAlreadyOpen));

        drop(store);
        SqliteProjectStore::open(path).await.unwrap();
    }

    fn profile_spec(name: &str) -> WorkerProfileSpec {
        WorkerProfileSpec {
            name: name.to_owned(),
            runtime_adapter: "herdr".to_owned(),
            provider: "codex".to_owned(),
            model: Some("gpt-5.4".to_owned()),
            default_role: "implementer".to_owned(),
            instructions_ref: None,
            tools: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            sandbox_policy: "runtime_default".to_owned(),
            worktree_policy: "project_workspace".to_owned(),
            permission_policy: "runtime_default".to_owned(),
            completion_contract: "manual_receipt".to_owned(),
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

    fn profile_project_command(
        profile: &WorkerProfile,
        workspace_id: &str,
        command_id: &str,
    ) -> CreateProjectFromProfile {
        CreateProjectFromProfile {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            name: "Runtime API".to_owned(),
            runtime: ProjectRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "default".to_owned(),
                workspace_id: workspace_id.to_owned(),
            },
            profile_id: profile.id.clone(),
            expected_profile_version: profile.version,
            orchestrator_objective: "Coordinate implementation of the Runtime API.".to_owned(),
            placement: CanvasPlacement {
                x: 80.0,
                y: 70.0,
                width: 322.0,
                height: 240.0,
            },
        }
    }

    fn workspace_project_command(
        profile: &WorkerProfile,
        command_id: &str,
    ) -> CreateWorkspaceProjectFromProfile {
        CreateWorkspaceProjectFromProfile {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            name: "Runtime API".to_owned(),
            runtime_adapter: "herdr".to_owned(),
            runtime_session: "default".to_owned(),
            workspace_label: "Runtime API workspace".to_owned(),
            cwd: "/work/runtime-api".to_owned(),
            profile_id: profile.id.clone(),
            expected_profile_version: profile.version,
            orchestrator_objective: "Coordinate implementation of the Runtime API.".to_owned(),
            placement: CanvasPlacement {
                x: 80.0,
                y: 70.0,
                width: 322.0,
                height: 240.0,
            },
        }
    }

    fn observed_worker(
        terminal_id: &str,
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        provider_session: Option<ProviderSessionRef>,
    ) -> ObservedWorker {
        ObservedWorker {
            runtime_id: terminal_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            tab_id: tab_id.to_owned(),
            pane_id: pane_id.to_owned(),
            name: Some(terminal_id.to_owned()),
            provider: Some("codex".to_owned()),
            display_provider: Some("Codex".to_owned()),
            status: ObservedStatus::Working,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 7,
            cwd: None,
            foreground_cwd: None,
            tokens: BTreeMap::new(),
            provider_session,
            revision: 9,
        }
    }

    fn observed_pane(
        terminal_id: &str,
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        provider_session: Option<ProviderSessionRef>,
    ) -> PaneObservation {
        PaneObservation {
            runtime_id: pane_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            tab_id: tab_id.to_owned(),
            focused: false,
            cwd: None,
            foreground_cwd: None,
            label: None,
            provider: None,
            display_provider: None,
            status: ObservedStatus::Unknown,
            tokens: BTreeMap::new(),
            provider_session,
            revision: 11,
        }
    }

    fn inventory(
        observed_at_unix_ms: u64,
        workers: Vec<ObservedWorker>,
        panes: Vec<PaneObservation>,
    ) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "test".to_owned(),
            protocol: 19,
            observed_at_unix_ms,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes,
            workers,
            child_agents: Vec::new(),
        }
    }

    async fn configure_yard_orchestrator_for_routes(
        store: &SqliteProjectStore,
    ) -> YardOrchestrator {
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-yard-routes",
                    "workspace-yard-routes",
                    "tab-yard-routes",
                    "pane-yard-routes",
                    Some(provider_session("session-yard-routes")),
                )],
                Vec::new(),
            ))
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
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-yard-routes")
            })
            .unwrap();
        let current = store.get_yard_orchestrator().await.unwrap();
        store
            .configure_yard_orchestrator(ConfigureYardOrchestrator {
                command_id: "configure-yard-for-routes".to_owned(),
                actor: "local-user".to_owned(),
                worker_id: candidate.worker.id,
                expected_worker_version: candidate.worker.version,
                expected_orchestrator_version: current.version,
            })
            .await
            .unwrap()
            .orchestrator
    }

    fn yard_route_command(
        command_id: &str,
        orchestrator: &YardOrchestrator,
        target_project: &Project,
    ) -> SendYardOrchestratorRoute {
        SendYardOrchestratorRoute {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            expected_orchestrator_version: orchestrator.version,
            orchestrator_worker_id: orchestrator.worker.as_ref().unwrap().id.clone(),
            target_project_id: target_project.id.clone(),
            expected_project_version: target_project.version,
            target_orchestrator_worker_id: target_project.orchestrator.id.clone(),
            text: "Coordinate the target project.".to_owned(),
        }
    }

    async fn create_active_assignment(
        store: &SqliteProjectStore,
    ) -> (String, yard_domain::Assignment) {
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        let allocation = ConfirmProfileAllocation {
            command_id: "prompt-test-allocation".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_project_version: project.version,
            objective: "Implement direct intervention.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        store
            .begin_profile_allocation(&project.id, allocation.clone())
            .await
            .unwrap();
        let (_, mut runtime) = draft("workspace-1", "terminal-2");
        runtime.tab_id = Some("tab-2".to_owned());
        runtime.pane_id = "pane-2".to_owned();
        runtime.owns_tab = true;
        store
            .persist_runtime_allocation(&allocation.command_id, runtime)
            .await
            .unwrap();
        let active = store
            .activate_profile_allocation(&allocation.command_id)
            .await
            .unwrap()
            .assignment;
        (project.id, active)
    }

    async fn create_handoff_fixture(
        store: &SqliteProjectStore,
    ) -> (Project, yard_domain::Assignment, Project) {
        let (source_project_id, assignment) = create_active_assignment(store).await;
        let source_project = store.get_project(&source_project_id).await.unwrap();
        let (mut target_draft, target_orchestrator) =
            draft("workspace-2", "terminal-target-orchestrator");
        target_draft.name = "Target API".to_owned();
        let target_project = store
            .create_project(target_draft, target_orchestrator)
            .await
            .unwrap();
        (source_project, assignment, target_project)
    }

    fn handoff_command(
        command_id: &str,
        source_project: &Project,
        source_assignment: &yard_domain::Assignment,
        target_project: &Project,
        target_role: HandoffTargetRole,
    ) -> ConfirmWorkerHandoff {
        ConfirmWorkerHandoff {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            worker_id: source_assignment.worker.id.clone(),
            expected_worker_version: source_assignment.worker.version,
            expected_source_project_version: source_project.version,
            target_project_id: target_project.id.clone(),
            expected_target_project_version: target_project.version,
            source_attempt_id: source_assignment.attempt.id.clone(),
            expected_source_assignment_version: source_assignment.version,
            expected_source_attempt_version: source_assignment.attempt.version,
            target_role,
            objective: "Continue implementation in the target project.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        }
    }

    fn handoff_runtime(workspace_id: &str, terminal_id: &str) -> WorkerRuntimeBinding {
        let (_, mut runtime) = draft(workspace_id, terminal_id);
        runtime.tab_id = Some(format!("tab-{terminal_id}"));
        runtime.pane_id = format!("pane-{terminal_id}");
        runtime.provider_session = Some(provider_session(&format!("session-{terminal_id}")));
        runtime.owns_tab = true;
        runtime.last_observed_at_unix_ms = 50;
        runtime
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn worker_handoff_preserves_identity_replays_and_excludes_runtime_duplicates() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (source_project, source_assignment, target_project) =
            create_handoff_fixture(&store).await;
        let command = handoff_command(
            "handoff-member",
            &source_project,
            &source_assignment,
            &target_project,
            HandoffTargetRole::Member,
        );
        let started = store
            .begin_worker_handoff(&source_project.id, &source_assignment.id, command.clone())
            .await
            .unwrap();
        let BeginWorkerHandoff::Started(context) = started else {
            panic!("expected a new worker handoff");
        };
        assert_eq!(
            context.source_assignment.lifecycle,
            AssignmentLifecycle::HandingOff
        );
        assert_eq!(
            context.source_assignment.attempt.lifecycle,
            AttemptLifecycle::HandingOff
        );

        let runtime = handoff_runtime("workspace-2", "terminal-handed-off");
        store
            .claim_worker_handoff_runtime(&command.command_id, runtime.clone())
            .await
            .unwrap();
        let claimed_inventory = inventory(
            40,
            vec![
                observed_worker("terminal-2", "workspace-1", "tab-2", "pane-2", None),
                observed_worker(
                    "terminal-handed-off",
                    "workspace-2",
                    "tab-terminal-handed-off",
                    "pane-terminal-handed-off",
                    runtime.provider_session.clone(),
                ),
            ],
            Vec::new(),
        );
        let claimed_reconciliation = store
            .reconcile_runtime_inventory(claimed_inventory)
            .await
            .unwrap();
        assert_eq!(claimed_reconciliation.adopted_workers, 0);

        let confirmed = store
            .finalize_worker_handoff(&command.command_id, runtime.clone())
            .await
            .unwrap();
        assert_eq!(
            confirmed.source_assignment.lifecycle,
            AssignmentLifecycle::HandedOff
        );
        assert_eq!(
            confirmed.source_assignment.attempt.lifecycle,
            AttemptLifecycle::HandedOff
        );
        assert_eq!(confirmed.assignment.lifecycle, AssignmentLifecycle::Active);
        assert_eq!(confirmed.assignment.worker.id, source_assignment.worker.id);
        assert_eq!(confirmed.allocation.mode, AllocationMode::Handoff);
        assert_eq!(confirmed.assignment.project_id, target_project.id);
        assert_eq!(
            confirmed
                .assignment
                .worker
                .runtime
                .as_ref()
                .unwrap()
                .terminal_id,
            "terminal-handed-off"
        );

        let after_inventory = inventory(
            60,
            vec![
                observed_worker("terminal-2", "workspace-1", "tab-2", "pane-2", None),
                observed_worker(
                    "terminal-handed-off",
                    "workspace-2",
                    "tab-terminal-handed-off",
                    "pane-terminal-handed-off",
                    runtime.provider_session,
                ),
            ],
            Vec::new(),
        );
        let reconciled = store
            .reconcile_runtime_inventory(after_inventory)
            .await
            .unwrap();
        assert_eq!(reconciled.adopted_workers, 0);
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .begin_worker_handoff(&source_project.id, &source_assignment.id, command)
            .await
            .unwrap();
        let BeginWorkerHandoff::Replayed(replayed) = replayed else {
            panic!("expected durable handoff replay");
        };
        assert!(replayed.replayed);
        assert_eq!(replayed.assignment.id, confirmed.assignment.id);
        let pending = reopened
            .list_pending_runtime_cleanups(Some("handoff-member"), 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].worker_id, source_assignment.worker.id);
        assert_eq!(pending[0].reason, "handoff_source");
        assert_eq!(pending[0].terminal_id, "terminal-2");
        assert!(pending[0].owns_tab);

        let cleanup_id = pending[0].id.clone();
        reopened
            .fail_runtime_cleanup(&cleanup_id, "Herdr is temporarily unavailable")
            .await
            .unwrap();
        assert!(
            reopened
                .list_pending_runtime_cleanups(Some("handoff-member"), 10)
                .await
                .unwrap()
                .is_empty()
        );
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let retired_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM retired_runtime_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        let source_allocation_ended: bool = connection
            .query_row(
                "SELECT ended_at_unix_ms IS NOT NULL
                   FROM worker_allocations
                  WHERE id = ?1",
                [&source_assignment.allocation_id],
                |row| row.get(0),
            )
            .unwrap();
        let (cleanup_attempts, cleanup_error): (i64, Option<String>) = connection
            .query_row(
                "SELECT attempts, last_error
                   FROM runtime_cleanup_jobs
                  WHERE id = ?1",
                [&cleanup_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE runtime_cleanup_jobs
                    SET next_attempt_at_unix_ms = 0
                  WHERE id = ?1",
                [&cleanup_id],
            )
            .unwrap();
        assert_eq!(retired_count, 1);
        assert!(source_allocation_ended);
        assert_eq!(cleanup_attempts, 1);
        assert_eq!(
            cleanup_error.as_deref(),
            Some("Herdr is temporarily unavailable")
        );
        drop(connection);

        drop(reopened);
        let retried = open_store(&temp).await;
        let pending = retried
            .list_pending_runtime_cleanups(Some("handoff-member"), 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].attempts, 1);
        retried.succeed_runtime_cleanup(&cleanup_id).await.unwrap();
        retried.succeed_runtime_cleanup(&cleanup_id).await.unwrap();
        assert!(
            retried
                .list_pending_runtime_cleanups(Some("handoff-member"), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn orchestrator_handoff_replaces_directly_and_cannot_be_moved_again() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (source_project, source_assignment, target_project) =
            create_handoff_fixture(&store).await;
        let old_orchestrator_id = target_project.orchestrator.id.clone();
        let command = handoff_command(
            "handoff-orchestrator",
            &source_project,
            &source_assignment,
            &target_project,
            HandoffTargetRole::Orchestrator,
        );
        store
            .begin_worker_handoff(&source_project.id, &source_assignment.id, command.clone())
            .await
            .unwrap();
        let runtime = handoff_runtime("workspace-2", "terminal-new-orchestrator");
        store
            .claim_worker_handoff_runtime(&command.command_id, runtime.clone())
            .await
            .unwrap();
        let confirmed = store
            .finalize_worker_handoff(&command.command_id, runtime)
            .await
            .unwrap();
        let replaced_project = store.get_project(&target_project.id).await.unwrap();

        assert_eq!(
            replaced_project.orchestrator.id,
            source_assignment.worker.id
        );
        assert_eq!(
            confirmed.replaced_orchestrator_worker_id.as_deref(),
            Some(old_orchestrator_id.as_str())
        );
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let old_allocation_active: bool = connection
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM worker_allocations
                     WHERE worker_id = ?1 AND ended_at_unix_ms IS NULL
                 )",
                [&old_orchestrator_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!old_allocation_active);
        drop(connection);
        let pending = store
            .list_pending_runtime_cleanups(Some("handoff-orchestrator"), 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(
            pending
                .iter()
                .map(|cleanup| cleanup.reason.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["handoff_source", "replaced_orchestrator"])
        );
        let replaced_cleanup = pending
            .iter()
            .find(|cleanup| cleanup.reason == "replaced_orchestrator")
            .unwrap();
        assert_eq!(replaced_cleanup.worker_id, old_orchestrator_id);
        assert_eq!(replaced_cleanup.terminal_id, "terminal-target-orchestrator");
        assert!(!replaced_cleanup.owns_tab);

        let (mut third_draft, third_orchestrator) =
            draft("workspace-3", "terminal-third-orchestrator");
        third_draft.name = "Third API".to_owned();
        let third_project = store
            .create_project(third_draft, third_orchestrator)
            .await
            .unwrap();
        let current_target = store.get_project(&target_project.id).await.unwrap();
        let forbidden = handoff_command(
            "move-current-orchestrator",
            &current_target,
            &confirmed.assignment,
            &third_project,
            HandoffTargetRole::Member,
        );
        let error = store
            .begin_worker_handoff(&target_project.id, &confirmed.assignment.id, forbidden)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ProjectStoreError::OrchestratorHandoffForbidden
        ));
    }

    #[tokio::test]
    async fn orchestrator_prompt_and_replacement_allow_exactly_one_reservation() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (source_project, source_assignment, target_project) =
            create_handoff_fixture(&store).await;
        let handoff = handoff_command(
            "racing-orchestrator-handoff",
            &source_project,
            &source_assignment,
            &target_project,
            HandoffTargetRole::Orchestrator,
        );
        let prompt = SendOrchestratorPrompt {
            command_id: "racing-orchestrator-prompt".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: target_project.version,
            orchestrator_worker_id: target_project.orchestrator.id.clone(),
            text: "Review worker allocation before replacement.".to_owned(),
        };
        let handoff_store = store.clone();
        let prompt_store = store.clone();
        let source_project_id = source_project.id.clone();
        let source_assignment_id = source_assignment.id.clone();
        let target_project_id = target_project.id.clone();

        let (handoff_result, prompt_result) = tokio::join!(
            handoff_store.begin_worker_handoff(&source_project_id, &source_assignment_id, handoff,),
            prompt_store.begin_orchestrator_prompt(&target_project_id, prompt),
        );

        assert_ne!(handoff_result.is_ok(), prompt_result.is_ok());
        let ((Err(losing_error), Ok(_)) | (Ok(_), Err(losing_error))) =
            (handoff_result, prompt_result)
        else {
            unreachable!("exactly one reservation must succeed");
        };
        assert!(matches!(
            losing_error,
            ProjectStoreError::OrchestratorInterventionInProgress
        ));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn failed_handoff_rolls_back_but_ambiguous_handoff_stays_reserved() {
        let failed_temp = TempDir::new().unwrap();
        let failed_store = open_store(&failed_temp).await;
        let (source_project, source_assignment, target_project) =
            create_handoff_fixture(&failed_store).await;
        let failed_command = handoff_command(
            "failed-handoff",
            &source_project,
            &source_assignment,
            &target_project,
            HandoffTargetRole::Member,
        );
        failed_store
            .begin_worker_handoff(
                &source_project.id,
                &source_assignment.id,
                failed_command.clone(),
            )
            .await
            .unwrap();
        failed_store
            .fail_worker_handoff(&failed_command.command_id, "worker was not created", false)
            .await
            .unwrap();
        let rolled_back = failed_store
            .list_project_assignments(&source_project.id)
            .await
            .unwrap()
            .assignments
            .into_iter()
            .find(|assignment| assignment.id == source_assignment.id)
            .unwrap();
        let failed_replay = failed_store
            .begin_worker_handoff(&source_project.id, &source_assignment.id, failed_command)
            .await
            .unwrap_err();
        assert_eq!(rolled_back.lifecycle, AssignmentLifecycle::Active);
        assert_eq!(rolled_back.attempt.lifecycle, AttemptLifecycle::Active);
        assert!(matches!(
            failed_replay,
            ProjectStoreError::CommandPreviouslyFailed(_)
        ));

        let ambiguous_temp = TempDir::new().unwrap();
        let ambiguous_store = open_store(&ambiguous_temp).await;
        let (source_project, source_assignment, target_project) =
            create_handoff_fixture(&ambiguous_store).await;
        let ambiguous_command = handoff_command(
            "ambiguous-handoff",
            &source_project,
            &source_assignment,
            &target_project,
            HandoffTargetRole::Member,
        );
        ambiguous_store
            .begin_worker_handoff(
                &source_project.id,
                &source_assignment.id,
                ambiguous_command.clone(),
            )
            .await
            .unwrap();
        let claimed = handoff_runtime("workspace-2", "terminal-ambiguous-handoff");
        ambiguous_store
            .claim_worker_handoff_runtime(&ambiguous_command.command_id, claimed.clone())
            .await
            .unwrap();
        ambiguous_store
            .fail_worker_handoff(
                &ambiguous_command.command_id,
                "prompt acknowledgement timed out",
                true,
            )
            .await
            .unwrap();
        let retained = ambiguous_store
            .list_project_assignments(&source_project.id)
            .await
            .unwrap()
            .assignments
            .into_iter()
            .find(|assignment| assignment.id == source_assignment.id)
            .unwrap();
        let ambiguous_replay = ambiguous_store
            .begin_worker_handoff(&source_project.id, &source_assignment.id, ambiguous_command)
            .await
            .unwrap_err();
        let reconciliation = ambiguous_store
            .reconcile_runtime_inventory(inventory(
                60,
                vec![
                    observed_worker("terminal-2", "workspace-1", "tab-2", "pane-2", None),
                    observed_worker(
                        "terminal-ambiguous-handoff",
                        "workspace-2",
                        "tab-terminal-ambiguous-handoff",
                        "pane-terminal-ambiguous-handoff",
                        claimed.provider_session,
                    ),
                ],
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(retained.lifecycle, AssignmentLifecycle::HandingOff);
        assert_eq!(retained.attempt.lifecycle, AttemptLifecycle::HandingOff);
        assert!(matches!(
            ambiguous_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert_eq!(reconciliation.adopted_workers, 0);
    }

    #[tokio::test]
    async fn project_and_durable_orchestrator_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project, worker) = draft("workspace-1", "terminal-1");
        let created = store.create_project(project, worker).await.unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let loaded = reopened.get_project(&created.id).await.unwrap();

        assert_eq!(loaded, created);
        assert_ne!(loaded.id, loaded.orchestrator.id);
        assert_eq!(
            loaded.orchestrator.runtime.as_ref().unwrap().terminal_id,
            "terminal-1"
        );
    }

    #[tokio::test]
    async fn profile_backed_project_creation_replays_without_duplicate_runtime_state() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = profile_project_command(&profile, "workspace-1", "create-project-1");
        assert!(matches!(
            store
                .begin_profile_project_creation(command.clone())
                .await
                .unwrap(),
            BeginProfileProjectCreation::Started(_)
        ));
        let (_, mut runtime) = draft("workspace-1", "terminal-1");
        runtime.provider_session = Some(provider_session("orchestrator-session"));
        runtime.owns_tab = true;
        let created = store
            .finalize_profile_project_creation(&command.command_id, runtime)
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .begin_profile_project_creation(command)
            .await
            .unwrap();
        let BeginProfileProjectCreation::Replayed(replayed) = replayed else {
            panic!("expected exact project-creation replay");
        };
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let project_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        let allocation: (String, Option<String>) = connection
            .query_row(
                "SELECT mode, started_by_command_id
                   FROM worker_allocations
                  WHERE worker_id = ?1",
                [&created.project.orchestrator.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert!(replayed.replayed);
        assert_eq!(replayed.project.id, created.project.id);
        assert_eq!(
            replayed.project.orchestrator.profile_id.as_deref(),
            Some(profile.id.as_str())
        );
        assert_eq!(project_count, 1);
        assert_eq!(
            allocation,
            ("create_new".to_owned(), Some("create-project-1".to_owned()))
        );
    }

    #[tokio::test]
    async fn pending_project_creation_reserves_workspace_from_replay_and_adoption() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = profile_project_command(&profile, "workspace-1", "create-project-1");
        store
            .begin_profile_project_creation(command.clone())
            .await
            .unwrap();

        let replay_error = store
            .begin_profile_project_creation(command)
            .await
            .unwrap_err();
        let second_error = store
            .begin_profile_project_creation(profile_project_command(
                &profile,
                "workspace-1",
                "create-project-2",
            ))
            .await
            .unwrap_err();
        let (project, runtime) = draft("workspace-1", "terminal-1");
        let adoption_error = store.create_project(project, runtime).await.unwrap_err();

        assert!(matches!(
            replay_error,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert!(matches!(
            second_error,
            ProjectStoreError::RuntimeWorkspaceReserved
        ));
        assert!(matches!(
            adoption_error,
            ProjectStoreError::RuntimeWorkspaceReserved
        ));
    }

    #[tokio::test]
    async fn failed_project_creation_releases_workspace_reservation() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = profile_project_command(&profile, "workspace-1", "create-project-1");
        store
            .begin_profile_project_creation(command.clone())
            .await
            .unwrap();
        store
            .fail_profile_project_creation(
                &command.command_id,
                "workspace has no usable working directory",
                false,
            )
            .await
            .unwrap();

        let next = store
            .begin_profile_project_creation(profile_project_command(
                &profile,
                "workspace-1",
                "create-project-2",
            ))
            .await
            .unwrap();

        assert!(matches!(next, BeginProfileProjectCreation::Started(_)));
    }

    #[tokio::test]
    async fn finalization_pins_a_reconciled_profileless_runtime_worker() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = profile_project_command(&profile, "workspace-1", "create-project-1");
        store
            .begin_profile_project_creation(command.clone())
            .await
            .unwrap();
        let observed = observed_worker(
            "terminal-1",
            "workspace-1",
            "tab-1",
            "pane-1",
            Some(provider_session("orchestrator-session")),
        );
        store
            .reconcile_runtime_inventory(inventory(5, vec![observed], Vec::new()))
            .await
            .unwrap();
        let adopted = store
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
            .unwrap()
            .worker;
        let (_, mut runtime) = draft("workspace-1", "terminal-1");
        runtime.provider_session = Some(provider_session("orchestrator-session"));
        runtime.last_observed_at_unix_ms = 5;

        let created = store
            .finalize_profile_project_creation(&command.command_id, runtime)
            .await
            .unwrap();

        assert_eq!(created.project.orchestrator.id, adopted.id);
        assert_eq!(
            created.project.orchestrator.profile_id.as_deref(),
            Some(profile.id.as_str())
        );
        assert_eq!(created.project.orchestrator.profile_version, Some(1));
        assert_eq!(created.project.orchestrator.version, 2);
    }

    #[tokio::test]
    async fn workspace_backed_project_creation_replays_after_reopen_without_duplicates() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = workspace_project_command(&profile, "create-workspace-project-1");
        let started = store
            .begin_workspace_project_creation(command.clone())
            .await
            .unwrap();
        let BeginWorkspaceProjectCreation::Started(context) = started else {
            panic!("expected workspace project creation to start");
        };
        assert_eq!(context.command, command);
        assert_eq!(context.profile, profile);

        let (_, mut runtime) = draft("created-workspace-1", "terminal-1");
        runtime.provider_session = Some(provider_session("orchestrator-session"));
        runtime.owns_tab = true;
        let created = store
            .finalize_workspace_project_creation(&command.command_id, runtime)
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .begin_workspace_project_creation(command)
            .await
            .unwrap();
        let BeginWorkspaceProjectCreation::Replayed(replayed) = replayed else {
            panic!("expected exact workspace project-creation replay");
        };
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let project_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        let durable_result: (String, String, String) = connection
            .query_row(
                "SELECT result_runtime_workspace_id, result_project_id,
                        result_worker_id
                   FROM workspace_project_creation_commands
                  WHERE command_id = 'create-workspace-project-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let lifecycle_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type = 'workspace_backed_project_created'",
                [&created.project.id],
                |row| row.get(0),
            )
            .unwrap();
        let allocation: (String, Option<String>) = connection
            .query_row(
                "SELECT mode, started_by_command_id
                   FROM worker_allocations
                  WHERE worker_id = ?1",
                [&created.project.orchestrator.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert!(replayed.replayed);
        assert_eq!(replayed.project.id, created.project.id);
        assert_eq!(replayed.project.runtime.workspace_id, "created-workspace-1");
        assert_eq!(
            replayed.project.orchestrator.profile_id.as_deref(),
            Some(profile.id.as_str())
        );
        assert_eq!(project_count, 1);
        assert_eq!(
            durable_result,
            (
                "created-workspace-1".to_owned(),
                created.project.id,
                created.project.orchestrator.id,
            )
        );
        assert_eq!(lifecycle_count, 1);
        assert_eq!(
            allocation,
            (
                "create_new".to_owned(),
                Some("create-workspace-project-1".to_owned()),
            )
        );
    }

    #[tokio::test]
    async fn pending_and_ambiguous_workspace_project_creation_are_not_retried() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = workspace_project_command(&profile, "create-workspace-project-1");
        store
            .begin_workspace_project_creation(command.clone())
            .await
            .unwrap();

        let pending_replay = store
            .begin_workspace_project_creation(command.clone())
            .await
            .unwrap_err();
        store
            .fail_workspace_project_creation(&command.command_id, "runtime response was lost", true)
            .await
            .unwrap();
        let ambiguous_replay = store
            .begin_workspace_project_creation(command)
            .await
            .unwrap_err();
        let equivalent_command = store
            .begin_workspace_project_creation(workspace_project_command(
                &profile,
                "create-workspace-project-2",
            ))
            .await
            .unwrap_err();

        assert!(matches!(
            pending_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert!(matches!(
            ambiguous_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert!(matches!(
            equivalent_command,
            ProjectStoreError::RuntimeWorkspaceReserved
        ));
    }

    #[tokio::test]
    async fn failed_workspace_project_creation_releases_intent_reservation() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = workspace_project_command(&profile, "create-workspace-project-1");
        store
            .begin_workspace_project_creation(command.clone())
            .await
            .unwrap();
        store
            .fail_workspace_project_creation(
                &command.command_id,
                "runtime rejected the workspace request",
                false,
            )
            .await
            .unwrap();

        let next = store
            .begin_workspace_project_creation(workspace_project_command(
                &profile,
                "create-workspace-project-2",
            ))
            .await
            .unwrap();
        let original = store
            .begin_workspace_project_creation(command)
            .await
            .unwrap_err();

        assert!(matches!(next, BeginWorkspaceProjectCreation::Started(_)));
        assert!(matches!(
            original,
            ProjectStoreError::CommandPreviouslyFailed(_)
        ));
    }

    #[tokio::test]
    async fn workspace_project_finalization_rejects_runtime_mismatches() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (existing_project, existing_runtime) = draft("bound-workspace", "terminal-1");
        store
            .create_project(existing_project, existing_runtime)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = workspace_project_command(&profile, "create-workspace-project-1");
        store
            .begin_workspace_project_creation(command.clone())
            .await
            .unwrap();

        let (_, mut wrong_adapter) = draft("created-workspace", "terminal-2");
        wrong_adapter.adapter = "other-adapter".to_owned();
        let adapter_error = store
            .finalize_workspace_project_creation(&command.command_id, wrong_adapter)
            .await
            .unwrap_err();
        let (_, mut wrong_session) = draft("created-workspace", "terminal-2");
        wrong_session.session = "other-session".to_owned();
        let session_error = store
            .finalize_workspace_project_creation(&command.command_id, wrong_session)
            .await
            .unwrap_err();
        let (_, bound_runtime) = draft("bound-workspace", "terminal-2");
        let bound_error = store
            .finalize_workspace_project_creation(&command.command_id, bound_runtime)
            .await
            .unwrap_err();

        store
            .reconcile_runtime_inventory(inventory(5, Vec::new(), Vec::new()))
            .await
            .unwrap();
        let (_, stale_runtime) = draft("created-workspace", "terminal-2");
        let stale_error = store
            .finalize_workspace_project_creation(&command.command_id, stale_runtime)
            .await
            .unwrap_err();
        let (_, mut reused_runtime) = draft("created-workspace", "terminal-1");
        reused_runtime.last_observed_at_unix_ms = 5;
        let worker_error = store
            .finalize_workspace_project_creation(&command.command_id, reused_runtime)
            .await
            .unwrap_err();

        assert!(matches!(
            adapter_error,
            ProjectStoreError::RuntimeWorkspaceMismatch
        ));
        assert!(matches!(
            session_error,
            ProjectStoreError::RuntimeWorkspaceMismatch
        ));
        assert!(matches!(
            bound_error,
            ProjectStoreError::RuntimeWorkspaceAlreadyBound
        ));
        assert!(matches!(
            stale_error,
            ProjectStoreError::StaleRuntimeSnapshot
        ));
        assert!(matches!(
            worker_error,
            ProjectStoreError::RuntimeWorkerAlreadyBound
        ));
    }

    #[tokio::test]
    async fn populated_v11_database_migrates_without_project_creation_data_loss() {
        let temp = TempDir::new().unwrap();
        let database_path = temp.path().join("yard.sqlite3");
        let store = open_store(&temp).await;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Orchestrator"),
            })
            .await
            .unwrap();
        let command = profile_project_command(&profile, "workspace-1", "legacy-create-project");
        store
            .begin_profile_project_creation(command.clone())
            .await
            .unwrap();
        let (_, runtime) = draft("workspace-1", "terminal-1");
        let created = store
            .finalize_profile_project_creation(&command.command_id, runtime)
            .await
            .unwrap();
        drop(store);

        let connection = Connection::open(&database_path).unwrap();
        drop_automation_schema_for_downgrade(&connection);
        connection
            .execute_batch(
                "DROP TABLE coordination_snapshot_projects;
                 DROP TABLE coordination_snapshots;
                 DROP TABLE coordination_node_route_commands;
                 DROP TABLE coordination_node_prompt_commands;
                 DROP TABLE coordination_node_provision_commands;
                 DROP TABLE coordination_node_placement_commands;
                 DROP TABLE coordination_node_update_commands;
                 DROP TABLE coordination_node_create_commands;
                 DROP TABLE coordination_node_projects;
                 DROP TABLE coordination_node_placements;
                 DROP TABLE coordination_nodes;
                 DROP TABLE yard_orchestrator_route_commands;
                 DROP TABLE project_relationship_delete_commands;
                 DROP TABLE project_relationship_create_commands;
                 DROP TABLE project_relationships;
                 DROP TABLE yard_orchestrator_prompt_commands;
                 DROP TABLE yard_orchestrator_configure_commands;
                 DROP TABLE yard_orchestrator;
                 DROP TABLE orchestrator_prompt_commands;
                 DROP TABLE workspace_project_creation_commands;
                 PRAGMA user_version = 11;",
            )
            .unwrap();
        drop(connection);

        let migrated = SqliteProjectStore::open(&database_path).await.unwrap();
        let replayed = migrated
            .begin_profile_project_creation(command)
            .await
            .unwrap();
        let BeginProfileProjectCreation::Replayed(replayed) = replayed else {
            panic!("expected migrated profile project creation to replay");
        };
        let connection = Connection::open(database_path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let new_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM sqlite_schema
                     WHERE type = 'table'
                       AND name = 'workspace_project_creation_commands'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let project_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();

        assert_eq!(replayed.project.id, created.project.id);
        assert_eq!(schema_version, SCHEMA_VERSION);
        assert!(new_table_exists);
        assert_eq!(project_count, 1);
    }

    #[tokio::test]
    async fn adoption_is_atomic_when_workspace_is_already_bound() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (first_project, first_worker) = draft("workspace-1", "terminal-1");
        store
            .create_project(first_project, first_worker)
            .await
            .unwrap();
        let (second_project, second_worker) = draft("workspace-1", "terminal-2");

        let error = store
            .create_project(second_project, second_worker)
            .await
            .unwrap_err();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();

        assert!(matches!(
            error,
            ProjectStoreError::RuntimeWorkspaceAlreadyBound
        ));
        assert_eq!(worker_count, 1);
    }

    #[tokio::test]
    async fn observed_worker_can_only_back_one_yard_worker() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (first_project, first_worker) = draft("workspace-1", "terminal-1");
        store
            .create_project(first_project, first_worker)
            .await
            .unwrap();
        let (second_project, mut second_worker) = draft("workspace-2", "terminal-1");
        second_worker.workspace_id = "workspace-2".to_owned();

        let error = store
            .create_project(second_project, second_worker)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ProjectStoreError::RuntimeWorkerAlreadyBound
        ));
    }

    #[tokio::test]
    async fn placement_updates_use_independent_optimistic_versions() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project, worker) = draft("workspace-1", "terminal-1");
        let created = store.create_project(project, worker).await.unwrap();
        let update = UpdateProjectPlacement {
            placement: CanvasPlacement {
                x: 410.0,
                y: 120.0,
                width: 520.0,
                height: 360.0,
            },
            expected_version: created.placement.version,
        };

        let updated = store
            .update_project_placement(&created.id, update)
            .await
            .unwrap();
        let error = store
            .update_project_placement(&created.id, update)
            .await
            .unwrap_err();

        assert_eq!(updated.version, created.version);
        assert_eq!(updated.placement.version, 2);
        assert_eq!(updated.placement.geometry, update.placement);
        assert!(matches!(
            error,
            ProjectStoreError::VersionConflict { current_version: 2 }
        ));
    }

    #[tokio::test]
    async fn populated_v1_database_migrates_without_identity_loss() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection
            .execute_batch(
                "INSERT INTO workers
                    VALUES ('worker-1', 'orchestrator', 10, 10);
                 INSERT INTO projects
                    VALUES ('project-1', 'Migrated', 'worker-1', 1, 10, 10);
                 INSERT INTO project_workspace_bindings
                    VALUES ('project-1', 'herdr', 'default', 'workspace-1');
                 INSERT INTO project_placements
                    VALUES ('project-1', 80.0, 70.0, 322.0, 240.0, 1, 10);
                 INSERT INTO worker_runtime_bindings (
                    worker_id, adapter, runtime_session, runtime_workspace_id,
                    terminal_id, pane_id, provider_session_source,
                    provider_session_provider, provider_session_kind,
                    provider_session_value, last_observed_at_unix_ms
                 ) VALUES (
                    'worker-1', 'herdr', 'default', 'workspace-1',
                    'terminal-1', 'pane-1', NULL, NULL, NULL, NULL, 10
                 );",
            )
            .unwrap();
        drop(connection);

        let store = open_store(&temp).await;
        let project = store.get_project("project-1").await.unwrap();
        let assignments = store.list_project_assignments("project-1").await.unwrap();
        let connection = Connection::open(path).unwrap();
        let allocation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM worker_allocations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let foreign_key_errors: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(project.id, "project-1");
        assert_eq!(project.orchestrator.id, "worker-1");
        assert_eq!(project.orchestrator.runtime.unwrap().tab_id, None);
        assert!(assignments.assignments.is_empty());
        assert_eq!(allocation_count, 1);
        assert_eq!(foreign_key_errors, 0);
    }

    #[tokio::test]
    async fn populated_v2_database_migrates_active_assignments_without_identity_loss() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection
            .execute_batch(PROFILE_ASSIGNMENT_MIGRATION)
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO worker_profiles
                    VALUES ('profile-1', 'Implementer', 1, 10, 10);
                 INSERT INTO worker_profile_revisions
                    VALUES (
                        'profile-1', 1, 'Implementer', 'herdr', 'codex', NULL,
                        'implementer', NULL, 'runtime_default',
                        'project_workspace', 'runtime_default',
                        'manual_receipt', 10
                    );
                 INSERT INTO workers
                    VALUES ('orchestrator-1', NULL, NULL, 'running', 1, 10, 10);
                 INSERT INTO workers
                    VALUES ('worker-1', 'profile-1', 1, 'running', 1, 11, 11);
                 INSERT INTO projects
                    VALUES ('project-1', 'Migrated', 'orchestrator-1', 2, 10, 11);
                 INSERT INTO project_workspace_bindings
                    VALUES ('project-1', 'herdr', 'default', 'workspace-1');
                 INSERT INTO project_placements
                    VALUES ('project-1', 80.0, 70.0, 322.0, 240.0, 1, 10);
                 INSERT INTO worker_runtime_bindings
                    VALUES (
                        'orchestrator-1', 'herdr', 'default', 'workspace-1',
                        'terminal-1', 'tab-1', 'pane-1',
                        NULL, NULL, NULL, NULL, 0, 'observed', 1, 10, 10
                    );
                 INSERT INTO worker_runtime_bindings
                    VALUES (
                        'worker-1', 'herdr', 'default', 'workspace-1',
                        'terminal-2', 'tab-2', 'pane-2',
                        NULL, NULL, NULL, NULL, 1, 'observed', 1, 11, 11
                    );
                 INSERT INTO worker_allocations
                    VALUES (
                        'allocation-orchestrator', 'project-1',
                        'orchestrator-1', 'adopt_existing', NULL, 10, NULL
                    );
                 INSERT INTO worker_allocations
                    VALUES (
                        'allocation-1', 'project-1', 'worker-1',
                        'create_new', NULL, 11, NULL
                    );
                 INSERT INTO assignments
                    VALUES (
                        'assignment-1', 'project-1', 'allocation-1', 'worker-1',
                        'profile-1', 1, 'Implement migration.', 'implementer',
                        'project_workspace', 'active', 2, 11, 12
                    );
                 INSERT INTO assignment_attempts
                    VALUES (
                        'attempt-1', 'assignment-1', 1, 'active',
                        NULL, 2, 11, 12
                    );",
            )
            .unwrap();
        drop(connection);

        let store = open_store(&temp).await;
        let assignments = store.list_project_assignments("project-1").await.unwrap();
        let connection = Connection::open(path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let foreign_key_errors: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(assignments.assignments.len(), 1);
        assert_eq!(assignments.assignments[0].id, "assignment-1");
        assert_eq!(assignments.assignments[0].attempt.id, "attempt-1");
        assert_eq!(
            assignments.assignments[0].lifecycle,
            yard_domain::AssignmentLifecycle::Active
        );
        assert_eq!(schema_version, SCHEMA_VERSION);
        assert_eq!(foreign_key_errors, 0);
    }

    #[tokio::test]
    async fn reconciliation_adopts_unknown_worker_once_across_reopen() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let snapshot = inventory(
            20,
            vec![observed_worker(
                "terminal-unmanaged",
                "workspace-unmanaged",
                "tab-unmanaged",
                "pane-unmanaged",
                Some(provider_session("session-unmanaged")),
            )],
            Vec::new(),
        );

        let first = store
            .reconcile_runtime_inventory(snapshot.clone())
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let second = reopened
            .reconcile_runtime_inventory(RuntimeInventory {
                observed_at_unix_ms: 21,
                ..snapshot
            })
            .await
            .unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();
        let binding_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM worker_runtime_bindings
                  WHERE terminal_id = 'terminal-unmanaged'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(first.adopted_workers, 1);
        assert_eq!(second.adopted_workers, 0);
        assert_eq!(worker_count, 1);
        assert_eq!(binding_count, 1);
    }

    #[tokio::test]
    async fn project_creation_reuses_unallocated_adopted_worker() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-1",
                    "workspace-1",
                    "tab-1",
                    "pane-1",
                    Some(provider_session("session-1")),
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let adopted_worker_id: String = connection
            .query_row("SELECT id FROM workers", [], |row| row.get(0))
            .unwrap();
        drop(connection);
        let (project_draft, mut orchestrator) = draft("workspace-1", "terminal-1");
        orchestrator.last_observed_at_unix_ms = 21;

        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();
        let allocation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM worker_allocations", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(project.orchestrator.id, adopted_worker_id);
        assert_eq!(worker_count, 1);
        assert_eq!(allocation_count, 1);
    }

    #[tokio::test]
    async fn reconciliation_marks_duplicate_durable_provider_bindings_ambiguous() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let shared_provider = provider_session("shared-session");
        let (first_draft, mut first_runtime) = draft("workspace-1", "terminal-1");
        first_runtime.provider_session = Some(shared_provider.clone());
        let first = store
            .create_project(first_draft, first_runtime)
            .await
            .unwrap();
        let (second_draft, mut second_runtime) = draft("workspace-2", "terminal-2");
        second_runtime.provider_session = Some(shared_provider.clone());
        let second = store
            .create_project(second_draft, second_runtime)
            .await
            .unwrap();

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-new",
                    "workspace-1",
                    "tab-new",
                    "pane-new",
                    Some(shared_provider),
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let first = store.get_project(&first.id).await.unwrap();
        let second = store.get_project(&second.id).await.unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();

        assert_eq!(result.ambiguous_bindings, 2);
        assert_eq!(result.adopted_workers, 0);
        assert_eq!(
            first.orchestrator.runtime.unwrap().observation_state,
            RuntimeObservationState::Ambiguous
        );
        assert_eq!(
            second.orchestrator.runtime.unwrap().observation_state,
            RuntimeObservationState::Ambiguous
        );
        assert_eq!(worker_count, 2);
    }

    #[tokio::test]
    async fn reconciliation_ignores_snapshot_older_than_session_watermark() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                30,
                vec![observed_worker(
                    "terminal-1",
                    "workspace-1",
                    "tab-new",
                    "pane-new",
                    None,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();

        let stale = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-stale",
                    "workspace-stale",
                    "tab-stale",
                    "pane-stale",
                    None,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();

        assert_eq!(stale.updated_bindings, 0);
        assert_eq!(stale.missing_bindings, 0);
        assert_eq!(stale.adopted_workers, 0);
        assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
        assert_eq!(runtime.terminal_id, "terminal-1");
        assert_eq!(runtime.tab_id.as_deref(), Some("tab-new"));
        assert_eq!(runtime.last_observed_at_unix_ms, 30);
        assert_eq!(worker_count, 1);
    }

    #[tokio::test]
    async fn reconciliation_updates_exact_binding_status_and_topology() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-1",
                    "workspace-1",
                    "tab-moved",
                    "pane-moved",
                    Some(provider_session("session-1")),
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();

        assert_eq!(result.updated_bindings, 1);
        assert_eq!(runtime.tab_id.as_deref(), Some("tab-moved"));
        assert_eq!(runtime.pane_id, "pane-moved");
        assert_eq!(runtime.status, ObservedStatus::Working);
        assert_eq!(runtime.state_change_sequence, 7);
        assert_eq!(runtime.revision, 9);
        assert_eq!(runtime.version, 2);
        assert_eq!(runtime.last_observed_at_unix_ms, 20);
    }

    #[tokio::test]
    async fn reconciliation_repairs_topology_by_unique_provider_session() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, mut orchestrator) = draft("workspace-1", "terminal-old");
        orchestrator.provider_session = Some(provider_session("stable-session"));
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-new",
                    "workspace-1",
                    "tab-new",
                    "pane-new",
                    Some(provider_session("stable-session")),
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();

        assert_eq!(result.updated_bindings, 1);
        assert_eq!(result.adopted_workers, 0);
        assert_eq!(runtime.terminal_id, "terminal-new");
        assert_eq!(runtime.tab_id.as_deref(), Some("tab-new"));
        assert_eq!(runtime.pane_id, "pane-new");
        assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
        assert_eq!(runtime.process_state, RuntimeProcessState::Running);
    }

    #[tokio::test]
    async fn reconciliation_marks_active_cross_workspace_binding_ambiguous() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-1",
                    "workspace-2",
                    "tab-2",
                    "pane-2",
                    None,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();

        assert_eq!(result.ambiguous_bindings, 1);
        assert_eq!(result.adopted_workers, 0);
        assert_eq!(
            runtime.observation_state,
            RuntimeObservationState::Ambiguous
        );
        assert_eq!(runtime.process_state, RuntimeProcessState::Unknown);
        assert_eq!(runtime.workspace_id, "workspace-1");
        assert_eq!(runtime.terminal_id, "terminal-1");
    }

    #[tokio::test]
    async fn reconciliation_marks_absent_binding_missing() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();

        let result = store
            .reconcile_runtime_inventory(inventory(20, Vec::new(), Vec::new()))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();

        assert_eq!(result.missing_bindings, 1);
        assert_eq!(runtime.observation_state, RuntimeObservationState::Missing);
        assert_eq!(runtime.process_state, RuntimeProcessState::Unknown);
        assert_eq!(runtime.last_observed_at_unix_ms, 1);
    }

    #[tokio::test]
    async fn reconciliation_marks_bound_pane_without_agent_exited() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                Vec::new(),
                vec![observed_pane(
                    "terminal-1",
                    "workspace-1",
                    "tab-1",
                    "pane-1",
                    None,
                )],
            ))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();

        assert_eq!(result.exited_processes, 1);
        assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
        assert_eq!(runtime.process_state, RuntimeProcessState::Exited);
        assert_eq!(runtime.status, ObservedStatus::Unknown);
        assert_eq!(runtime.last_observed_at_unix_ms, 20);
    }

    #[tokio::test]
    async fn reconciliation_process_exit_does_not_complete_assignment() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-1",
                    "workspace-1",
                    "tab-1",
                    "pane-1",
                    None,
                )],
                vec![observed_pane(
                    "terminal-2",
                    "workspace-1",
                    "tab-2",
                    "pane-2",
                    None,
                )],
            ))
            .await
            .unwrap();
        let assignments = store.list_project_assignments(&project_id).await.unwrap();
        let assignment = &assignments.assignments[0];
        let runtime = assignment.worker.runtime.as_ref().unwrap();

        assert_eq!(result.exited_processes, 1);
        assert_eq!(assignment.id, active.id);
        assert_eq!(
            assignment.lifecycle,
            yard_domain::AssignmentLifecycle::Active
        );
        assert_eq!(assignment.attempt.lifecycle, AttemptLifecycle::Active);
        assert!(assignment.completion_receipt.is_none());
        assert_eq!(runtime.process_state, RuntimeProcessState::Exited);
    }

    #[tokio::test]
    async fn reconciliation_keeps_duplicate_provider_candidates_ambiguous() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, mut orchestrator) = draft("workspace-1", "terminal-old");
        orchestrator.provider_session = Some(provider_session("duplicate-session"));
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let duplicate_session = provider_session("duplicate-session");

        let result = store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![
                    observed_worker(
                        "terminal-a",
                        "workspace-1",
                        "tab-a",
                        "pane-a",
                        Some(duplicate_session.clone()),
                    ),
                    observed_worker(
                        "terminal-b",
                        "workspace-1",
                        "tab-b",
                        "pane-b",
                        Some(duplicate_session),
                    ),
                ],
                Vec::new(),
            ))
            .await
            .unwrap();
        let loaded = store.get_project(&project.id).await.unwrap();
        let runtime = loaded.orchestrator.runtime.unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();

        assert_eq!(result.ambiguous_bindings, 1);
        assert_eq!(result.adopted_workers, 0);
        assert_eq!(
            runtime.observation_state,
            RuntimeObservationState::Ambiguous
        );
        assert_eq!(runtime.terminal_id, "terminal-old");
        assert_eq!(worker_count, 1);
    }

    #[tokio::test]
    async fn v4_migration_rolls_back_when_foreign_keys_are_invalid() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection
            .execute_batch(PROFILE_ASSIGNMENT_MIGRATION)
            .unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .unwrap();
        connection
            .execute_batch(COMPLETION_RECEIPT_MIGRATION)
            .unwrap();
        connection
            .execute(
                "INSERT INTO worker_runtime_bindings (
                    worker_id, adapter, runtime_session, runtime_workspace_id,
                    terminal_id, tab_id, pane_id, provider_session_source,
                    provider_session_provider, provider_session_kind,
                    provider_session_value, owns_tab, observation_state, version,
                    last_observed_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    'missing-worker', 'herdr', 'default', 'workspace-1',
                    'terminal-1', 'tab-1', 'pane-1', NULL, NULL, NULL, NULL,
                    0, 'observed', 1, 10, 10
                 )",
                [],
            )
            .unwrap();
        drop(connection);

        let error = SqliteProjectStore::open(&path).await.unwrap_err();
        let connection = Connection::open(path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let prompt_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'assignment_prompt_commands'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(matches!(error, ProjectStoreError::ForeignKeyCheckFailed));
        assert_eq!(schema_version, 3);
        assert!(!prompt_table_exists);
    }

    #[tokio::test]
    async fn profile_revisions_are_immutable_and_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let created = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        let updated = store
            .update_worker_profile(
                &created.id,
                UpdateWorkerProfile {
                    spec: profile_spec("Reviewer"),
                    expected_version: 1,
                },
            )
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let loaded = reopened.get_worker_profile(&created.id).await.unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let revision_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM worker_profile_revisions
                  WHERE profile_id = ?1",
                [&created.id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(updated.version, 2);
        assert_eq!(loaded.spec.name, "Reviewer");
        assert_eq!(revision_count, 2);
    }

    #[tokio::test]
    async fn allocation_rejects_a_stale_profile_revision() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        store
            .update_worker_profile(
                &profile.id,
                UpdateWorkerProfile {
                    spec: profile_spec("Reviewer"),
                    expected_version: profile.version,
                },
            )
            .await
            .unwrap();
        let command = ConfirmProfileAllocation {
            command_id: "stale-profile-command".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_project_version: project.version,
            objective: "Review the implementation.".to_owned(),
            role: "reviewer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };

        let error = store
            .begin_profile_allocation(&project.id, command)
            .await
            .unwrap_err();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let command_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM profile_allocation_commands",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(matches!(
            error,
            ProjectStoreError::ProfileVersionConflict { current_version: 2 }
        ));
        assert_eq!(command_count, 0);
    }

    #[tokio::test]
    async fn confirmed_allocation_replays_after_reopen_without_duplicates() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        let command = ConfirmProfileAllocation {
            command_id: "command-1".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id.clone(),
            expected_profile_version: profile.version,
            expected_project_version: project.version,
            objective: "Implement persistence.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        assert!(matches!(
            store
                .begin_profile_allocation(&project.id, command.clone())
                .await
                .unwrap(),
            BeginProfileAllocation::Started(_)
        ));
        let (_, mut runtime) = draft("workspace-1", "terminal-2");
        runtime.tab_id = Some("tab-2".to_owned());
        runtime.owns_tab = true;
        store
            .persist_runtime_allocation(&command.command_id, runtime)
            .await
            .unwrap();
        let created = store
            .activate_profile_allocation(&command.command_id)
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .begin_profile_allocation(&project.id, command)
            .await
            .unwrap();
        let assignments = reopened
            .list_project_assignments(&project.id)
            .await
            .unwrap();

        let BeginProfileAllocation::Replayed(replayed) = replayed else {
            panic!("expected allocation replay");
        };
        assert_eq!(replayed.assignment.id, created.assignment.id);
        assert!(replayed.replayed);
        assert_eq!(assignments.assignments.len(), 1);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn completion_receipt_replays_after_reopen_without_duplicate_history() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        let allocation_command = ConfirmProfileAllocation {
            command_id: "allocation-command".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_project_version: project.version,
            objective: "Implement persistence.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        store
            .begin_profile_allocation(&project.id, allocation_command.clone())
            .await
            .unwrap();
        let (_, mut runtime) = draft("workspace-1", "terminal-2");
        runtime.tab_id = Some("tab-2".to_owned());
        runtime.owns_tab = true;
        store
            .persist_runtime_allocation(&allocation_command.command_id, runtime)
            .await
            .unwrap();
        let active = store
            .activate_profile_allocation(&allocation_command.command_id)
            .await
            .unwrap()
            .assignment;
        let completion_command = RecordCompletionReceipt {
            command_id: "completion-command".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            outcome: CompletionOutcome::Completed,
            summary: "Implemented and verified persistence.".to_owned(),
            artifact_refs: vec!["yard://artifacts/change-set".to_owned()],
            artifact_ids: Vec::new(),
            evidence_refs: vec!["test://cargo-test".to_owned()],
            unresolved_blockers: vec!["Deployment remains manual.".to_owned()],
        };
        let mut stale_command = completion_command.clone();
        stale_command.command_id = "stale-completion-command".to_owned();
        stale_command.expected_assignment_version -= 1;
        let stale_error = store
            .record_completion_receipt(&project.id, &active.id, stale_command)
            .await
            .unwrap_err();
        assert!(matches!(
            stale_error,
            ProjectStoreError::AssignmentVersionConflict { current_version: 2 }
        ));
        let completed = store
            .record_completion_receipt(&project.id, &active.id, completion_command.clone())
            .await
            .unwrap();
        let mut duplicate_command = completion_command.clone();
        duplicate_command.command_id = "second-completion-command".to_owned();
        let duplicate_error = store
            .record_completion_receipt(&project.id, &active.id, duplicate_command)
            .await
            .unwrap_err();
        assert!(matches!(
            duplicate_error,
            ProjectStoreError::AssignmentNotActive
        ));
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .record_completion_receipt(&project.id, &active.id, completion_command)
            .await
            .unwrap();
        let assignments = reopened
            .list_project_assignments(&project.id)
            .await
            .unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let receipt_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM completion_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        let completion_event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type = 'completion_receipt_recorded'",
                [&active.id],
                |row| row.get(0),
            )
            .unwrap();
        let allocation_end: Option<i64> = connection
            .query_row(
                "SELECT ended_at_unix_ms FROM worker_allocations WHERE id = ?1",
                [&active.allocation_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            completed.assignment.lifecycle,
            yard_domain::AssignmentLifecycle::Completed
        );
        assert_eq!(
            completed.assignment.attempt.lifecycle,
            yard_domain::AttemptLifecycle::Completed
        );
        assert_eq!(completed.receipt.evidence_refs, ["test://cargo-test"]);
        assert_eq!(replayed.receipt.id, completed.receipt.id);
        assert!(replayed.replayed);
        assert_eq!(
            assignments.assignments[0]
                .completion_receipt
                .as_ref()
                .unwrap()
                .summary,
            "Implemented and verified persistence."
        );
        assert_eq!(receipt_count, 1);
        assert_eq!(completion_event_count, 1);
        assert!(allocation_end.is_some());
    }

    #[tokio::test]
    async fn assignment_prompt_replays_after_reopen_without_resubmission() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        let command = SendAssignmentPrompt {
            command_id: "prompt-command".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            text: "Continue with the failing test.".to_owned(),
        };
        let started = store
            .begin_assignment_prompt(&project_id, &active.id, command.clone())
            .await
            .unwrap();
        assert!(matches!(started, BeginAssignmentPrompt::Started { .. }));
        let acknowledged = store
            .succeed_assignment_prompt(&command.command_id, "working")
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .begin_assignment_prompt(&project_id, &active.id, command.clone())
            .await
            .unwrap();
        let BeginAssignmentPrompt::Replayed(replayed) = replayed else {
            panic!("expected prompt replay");
        };
        let mut conflict = command;
        conflict.text = "Different input.".to_owned();
        let conflict = reopened
            .begin_assignment_prompt(&project_id, &active.id, conflict)
            .await
            .unwrap_err();
        let assignments = reopened
            .list_project_assignments(&project_id)
            .await
            .unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
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
                [&active.id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(replayed, acknowledged);
        assert!(matches!(conflict, ProjectStoreError::IdempotencyConflict));
        assert_eq!(assignments.assignments[0].version, active.version);
        assert_eq!(
            assignments.assignments[0].attempt.version,
            active.attempt.version
        );
        assert_eq!(prompt_count, 1);
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn orchestrator_prompt_replays_after_reopen_without_resubmission() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let command = SendOrchestratorPrompt {
            command_id: "orchestrator-prompt-command".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: project.version,
            orchestrator_worker_id: project.orchestrator.id.clone(),
            text: "Rebalance the active workers.".to_owned(),
        };
        let started = store
            .begin_orchestrator_prompt(&project.id, command.clone())
            .await
            .unwrap();
        assert!(matches!(started, BeginOrchestratorPrompt::Started { .. }));
        let acknowledged = store
            .succeed_orchestrator_prompt(&command.command_id, "working")
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let replayed = reopened
            .begin_orchestrator_prompt(&project.id, command.clone())
            .await
            .unwrap();
        let BeginOrchestratorPrompt::Replayed(replayed) = replayed else {
            panic!("expected orchestrator prompt replay");
        };
        let mut conflict = command;
        conflict.text = "Different input.".to_owned();
        let conflict = reopened
            .begin_orchestrator_prompt(&project.id, conflict)
            .await
            .unwrap_err();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let prompt_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM orchestrator_prompt_commands",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type = 'orchestrator_prompt_submitted'",
                [&project.id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(replayed, acknowledged);
        assert_eq!(replayed.worker_id, project.orchestrator.id);
        assert!(matches!(conflict, ProjectStoreError::IdempotencyConflict));
        assert_eq!(prompt_count, 1);
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn interrupted_prompts_become_ambiguous_and_release_replacement() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (source_project, source_assignment, target_project) =
            create_handoff_fixture(&store).await;
        let assignment_prompt = SendAssignmentPrompt {
            command_id: "interrupted-assignment-prompt".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: source_assignment.attempt.id.clone(),
            expected_assignment_version: source_assignment.version,
            expected_attempt_version: source_assignment.attempt.version,
            text: "Review the source assignment.".to_owned(),
        };
        let orchestrator_prompt = SendOrchestratorPrompt {
            command_id: "interrupted-orchestrator-prompt".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: target_project.version,
            orchestrator_worker_id: target_project.orchestrator.id.clone(),
            text: "Review the target project.".to_owned(),
        };
        store
            .begin_assignment_prompt(
                &source_project.id,
                &source_assignment.id,
                assignment_prompt.clone(),
            )
            .await
            .unwrap();
        store
            .begin_orchestrator_prompt(&target_project.id, orchestrator_prompt.clone())
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let assignment_replay = reopened
            .begin_assignment_prompt(
                &source_project.id,
                &source_assignment.id,
                assignment_prompt.clone(),
            )
            .await
            .unwrap_err();
        let orchestrator_replay = reopened
            .begin_orchestrator_prompt(&target_project.id, orchestrator_prompt.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            assignment_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert!(matches!(
            orchestrator_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));

        let handoff = handoff_command(
            "replacement-after-interrupted-prompts",
            &source_project,
            &source_assignment,
            &target_project,
            HandoffTargetRole::Orchestrator,
        );
        let replacement = reopened
            .begin_worker_handoff(&source_project.id, &source_assignment.id, handoff)
            .await
            .unwrap();
        assert!(matches!(replacement, BeginWorkerHandoff::Started(_)));

        drop(reopened);
        let reopened_again = open_store(&temp).await;
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        for command_id in [
            assignment_prompt.command_id.as_str(),
            orchestrator_prompt.command_id.as_str(),
        ] {
            let (status, message): (String, Option<String>) = connection
                .query_row(
                    "SELECT status, error_message
                       FROM command_acknowledgements
                      WHERE id = ?1",
                    [command_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(status, "ambiguous");
            assert!(
                message
                    .as_deref()
                    .is_some_and(|value| value.contains("Yard restarted"))
            );
        }
        drop(connection);
        drop(reopened_again);
    }

    #[tokio::test]
    async fn routine_reconciliation_does_not_stale_orchestrator_prompt_identity() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, mut orchestrator) = draft("workspace-1", "terminal-1");
        orchestrator.provider_session = Some(provider_session("orchestrator-session"));
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-1",
                    "workspace-1",
                    "tab-1",
                    "pane-1",
                    Some(provider_session("orchestrator-session")),
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        let reconciled = store.get_project(&project.id).await.unwrap();
        assert!(reconciled.orchestrator.version > project.orchestrator.version);

        let started = store
            .begin_orchestrator_prompt(
                &project.id,
                SendOrchestratorPrompt {
                    command_id: "post-reconciliation-prompt".to_owned(),
                    actor: "local-user".to_owned(),
                    expected_project_version: project.version,
                    orchestrator_worker_id: project.orchestrator.id,
                    text: "Review the reconciled worker state.".to_owned(),
                },
            )
            .await
            .unwrap();

        assert!(matches!(started, BeginOrchestratorPrompt::Started { .. }));
    }

    #[tokio::test]
    async fn populated_v12_database_migrates_prompt_history_without_identity_loss() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        let command = SendAssignmentPrompt {
            command_id: "legacy-v12-prompt".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            text: "Preserve this prompt acknowledgement.".to_owned(),
        };
        store
            .begin_assignment_prompt(&project_id, &active.id, command.clone())
            .await
            .unwrap();
        let acknowledged = store
            .succeed_assignment_prompt(&command.command_id, "working")
            .await
            .unwrap();
        drop(store);

        let database_path = temp.path().join("yard.sqlite3");
        let connection = Connection::open(&database_path).unwrap();
        drop_automation_schema_for_downgrade(&connection);
        connection
            .execute_batch(
                "DROP TABLE coordination_snapshot_projects;
                 DROP TABLE coordination_snapshots;
                 DROP TABLE coordination_node_route_commands;
                 DROP TABLE coordination_node_prompt_commands;
                 DROP TABLE coordination_node_provision_commands;
                 DROP TABLE coordination_node_placement_commands;
                 DROP TABLE coordination_node_update_commands;
                 DROP TABLE coordination_node_create_commands;
                 DROP TABLE coordination_node_projects;
                 DROP TABLE coordination_node_placements;
                 DROP TABLE coordination_nodes;
                 DROP TABLE yard_orchestrator_route_commands;
                 DROP TABLE project_relationship_delete_commands;
                 DROP TABLE project_relationship_create_commands;
                 DROP TABLE project_relationships;
                 DROP TABLE yard_orchestrator_prompt_commands;
                 DROP TABLE yard_orchestrator_configure_commands;
                 DROP TABLE yard_orchestrator;
                 DROP TABLE orchestrator_prompt_commands;
                 PRAGMA user_version = 12;",
            )
            .unwrap();
        drop(connection);

        let migrated = SqliteProjectStore::open(&database_path).await.unwrap();
        let replayed = migrated
            .begin_assignment_prompt(&project_id, &active.id, command)
            .await
            .unwrap();
        let BeginAssignmentPrompt::Replayed(replayed) = replayed else {
            panic!("expected migrated assignment prompt replay");
        };
        let connection = Connection::open(database_path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let foreign_key_errors: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(replayed, acknowledged);
        assert_eq!(schema_version, SCHEMA_VERSION);
        assert_eq!(foreign_key_errors, 0);
    }

    #[tokio::test]
    async fn typed_artifact_registration_and_receipt_linkage_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        let artifact_id = "019ff1a2-0000-7000-8000-000000000010";
        let registration = ArtifactRegistration {
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            kind: ArtifactKind::Markdown,
            display_name: "verification.md".to_owned(),
            byte_size: 13,
            sha256: "a".repeat(64),
        };

        let artifact = store
            .register_artifact(&project_id, &active.id, artifact_id, registration.clone())
            .await
            .unwrap();
        assert_eq!(artifact.assignment_id, active.id);
        assert_eq!(artifact.attempt_id, active.attempt.id);
        assert_eq!(artifact.worker_id, active.worker.id);
        assert_eq!(artifact.media_type, "text/markdown");
        assert_eq!(
            store
                .register_artifact(&project_id, &active.id, artifact_id, registration.clone())
                .await
                .unwrap(),
            artifact
        );

        let mut conflicting = registration;
        conflicting.sha256 = "b".repeat(64);
        assert!(matches!(
            store
                .register_artifact(&project_id, &active.id, artifact_id, conflicting)
                .await
                .unwrap_err(),
            ProjectStoreError::ArtifactIdConflict
        ));

        store
            .record_completion_receipt(
                &project_id,
                &active.id,
                RecordCompletionReceipt {
                    command_id: "typed-artifact-receipt".to_owned(),
                    actor: "local-user".to_owned(),
                    attempt_id: active.attempt.id.clone(),
                    expected_assignment_version: active.version,
                    expected_attempt_version: active.attempt.version,
                    outcome: CompletionOutcome::Completed,
                    summary: "Published verification notes.".to_owned(),
                    artifact_refs: Vec::new(),
                    artifact_ids: vec![artifact_id.to_owned()],
                    evidence_refs: Vec::new(),
                    unresolved_blockers: Vec::new(),
                },
            )
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let assignments = reopened
            .list_project_assignments(&project_id)
            .await
            .unwrap();
        let receipt = assignments.assignments[0]
            .completion_receipt
            .as_ref()
            .unwrap();
        assert!(receipt.artifact_refs.is_empty());
        assert_eq!(receipt.artifacts, [artifact]);
    }

    #[tokio::test]
    async fn populated_v10_receipts_migrate_without_synthetic_artifacts() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        store
            .record_completion_receipt(
                &project_id,
                &active.id,
                RecordCompletionReceipt {
                    command_id: "legacy-v10-receipt".to_owned(),
                    actor: "local-user".to_owned(),
                    attempt_id: active.attempt.id.clone(),
                    expected_assignment_version: active.version,
                    expected_attempt_version: active.attempt.version,
                    outcome: CompletionOutcome::Completed,
                    summary: "Preserve the opaque artifact reference.".to_owned(),
                    artifact_refs: vec!["artifact://legacy-output".to_owned()],
                    artifact_ids: Vec::new(),
                    evidence_refs: Vec::new(),
                    unresolved_blockers: Vec::new(),
                },
            )
            .await
            .unwrap();
        drop(store);

        let database_path = temp.path().join("yard.sqlite3");
        let connection = Connection::open(&database_path).unwrap();
        drop_automation_schema_for_downgrade(&connection);
        connection
            .execute_batch(
                "DROP TABLE coordination_snapshot_projects;
                 DROP TABLE coordination_snapshots;
                 DROP TABLE coordination_node_route_commands;
                 DROP TABLE coordination_node_prompt_commands;
                 DROP TABLE coordination_node_provision_commands;
                 DROP TABLE coordination_node_placement_commands;
                 DROP TABLE coordination_node_update_commands;
                 DROP TABLE coordination_node_create_commands;
                 DROP TABLE coordination_node_projects;
                 DROP TABLE coordination_node_placements;
                 DROP TABLE coordination_nodes;
                 DROP TABLE yard_orchestrator_route_commands;
                 DROP TABLE project_relationship_delete_commands;
                 DROP TABLE project_relationship_create_commands;
                 DROP TABLE project_relationships;
                 DROP TABLE yard_orchestrator_prompt_commands;
                 DROP TABLE yard_orchestrator_configure_commands;
                 DROP TABLE yard_orchestrator;
                 DROP TABLE orchestrator_prompt_commands;
                 DROP TABLE workspace_project_creation_commands;
                 DROP TABLE completion_receipt_artifact_links;
                 DROP TABLE artifacts;
                 DROP INDEX completion_receipt_assignment_attempt_identity;
                 PRAGMA user_version = 10;",
            )
            .unwrap();
        drop(connection);

        let migrated = SqliteProjectStore::open(&database_path).await.unwrap();
        let assignments = migrated
            .list_project_assignments(&project_id)
            .await
            .unwrap();
        let receipt = assignments.assignments[0]
            .completion_receipt
            .as_ref()
            .unwrap();
        assert_eq!(receipt.artifact_refs, ["artifact://legacy-output"]);
        assert!(receipt.artifacts.is_empty());

        let connection = Connection::open(database_path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn completion_waits_for_pending_assignment_prompt() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        let prompt = SendAssignmentPrompt {
            command_id: "pending-prompt-command".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            text: "Check the final test before completion.".to_owned(),
        };
        store
            .begin_assignment_prompt(&project_id, &active.id, prompt.clone())
            .await
            .unwrap();
        let completion = RecordCompletionReceipt {
            command_id: "completion-after-prompt".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            outcome: CompletionOutcome::Completed,
            summary: "Verified the final test.".to_owned(),
            artifact_refs: Vec::new(),
            artifact_ids: Vec::new(),
            evidence_refs: vec!["test://final".to_owned()],
            unresolved_blockers: Vec::new(),
        };

        let blocked = store
            .record_completion_receipt(&project_id, &active.id, completion.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            blocked,
            ProjectStoreError::AssignmentInterventionInProgress
        ));

        store
            .succeed_assignment_prompt(&prompt.command_id, "working")
            .await
            .unwrap();
        let completed = store
            .record_completion_receipt(&project_id, &active.id, completion)
            .await
            .unwrap();
        assert_eq!(
            completed.assignment.lifecycle,
            yard_domain::AssignmentLifecycle::Completed
        );
    }

    #[tokio::test]
    async fn rejected_and_ambiguous_prompts_never_reenter_submission() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        let command = |id: &str| SendAssignmentPrompt {
            command_id: id.to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: active.attempt.id.clone(),
            expected_assignment_version: active.version,
            expected_attempt_version: active.attempt.version,
            text: "Inspect the current failure.".to_owned(),
        };

        let rejected = command("rejected-prompt");
        store
            .begin_assignment_prompt(&project_id, &active.id, rejected.clone())
            .await
            .unwrap();
        store
            .fail_assignment_prompt(&rejected.command_id, "agent not ready", false)
            .await
            .unwrap();
        let rejected_replay = store
            .begin_assignment_prompt(&project_id, &active.id, rejected)
            .await
            .unwrap_err();

        let ambiguous = command("ambiguous-prompt");
        store
            .begin_assignment_prompt(&project_id, &active.id, ambiguous.clone())
            .await
            .unwrap();
        store
            .fail_assignment_prompt(&ambiguous.command_id, "socket timed out", true)
            .await
            .unwrap();
        let ambiguous_replay = store
            .begin_assignment_prompt(&project_id, &active.id, ambiguous)
            .await
            .unwrap_err();

        let pending = command("pending-prompt");
        store
            .begin_assignment_prompt(&project_id, &active.id, pending.clone())
            .await
            .unwrap();
        let pending_replay = store
            .begin_assignment_prompt(&project_id, &active.id, pending)
            .await
            .unwrap_err();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type = 'assignment_prompt_submitted'",
                [&active.id],
                |row| row.get(0),
            )
            .unwrap();

        assert!(matches!(
            rejected_replay,
            ProjectStoreError::CommandPreviouslyFailed(_)
        ));
        assert!(matches!(
            ambiguous_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert!(matches!(
            pending_replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert_eq!(event_count, 0);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn yard_orchestrator_claims_one_live_worker_and_protects_its_session() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let initial = store.get_yard_orchestrator().await.unwrap();
        assert!(initial.worker.is_none());
        assert_eq!(initial.version, 1);

        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-yard",
                    "workspace-yard",
                    "tab-yard",
                    "pane-yard",
                    Some(provider_session("session-yard")),
                )],
                Vec::new(),
            ))
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
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-yard")
            })
            .unwrap();
        let command = ConfigureYardOrchestrator {
            command_id: "configure-yard-orchestrator".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id.clone(),
            expected_worker_version: candidate.worker.version,
            expected_orchestrator_version: initial.version,
        };
        let configured = store
            .configure_yard_orchestrator(command.clone())
            .await
            .unwrap();
        let replayed = store.configure_yard_orchestrator(command).await.unwrap();
        assert_eq!(
            configured.orchestrator.worker.as_ref().unwrap().id,
            candidate.worker.id
        );
        assert_eq!(configured.orchestrator.version, 2);
        assert!(replayed.replayed);

        let protected = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .into_iter()
            .find(|worker| worker.worker.id == candidate.worker.id)
            .unwrap();
        assert_eq!(protected.availability, WorkerAvailability::YardOrchestrator);
        assert!(matches!(
            store
                .end_worker_session(
                    &protected.worker.id,
                    EndWorkerSession {
                        command_id: "end-yard-orchestrator".to_owned(),
                        actor: "local-user".to_owned(),
                        expected_worker_version: protected.worker.version,
                        expected_runtime_version: protected
                            .worker
                            .runtime
                            .as_ref()
                            .map(|runtime| runtime.version),
                    },
                )
                .await
                .unwrap_err(),
            ProjectStoreError::YardOrchestratorSessionEndForbidden
        ));

        let prompt = SendYardOrchestratorPrompt {
            command_id: "prompt-yard-orchestrator".to_owned(),
            actor: "local-user".to_owned(),
            expected_orchestrator_version: configured.orchestrator.version,
            orchestrator_worker_id: protected.worker.id.clone(),
            text: "Summarize project attention needs.".to_owned(),
        };
        let started = store
            .begin_yard_orchestrator_prompt(prompt.clone())
            .await
            .unwrap();
        assert!(matches!(
            started,
            BeginYardOrchestratorPrompt::Started { .. }
        ));
        let acknowledged = store
            .succeed_yard_orchestrator_prompt(&prompt.command_id, "accepted")
            .await
            .unwrap();
        let replayed = store.begin_yard_orchestrator_prompt(prompt).await.unwrap();
        let BeginYardOrchestratorPrompt::Replayed(replayed) = replayed else {
            panic!("expected Yard orchestrator prompt replay");
        };
        assert_eq!(replayed, acknowledged);
    }

    #[tokio::test]
    async fn live_worker_allocation_reuses_identity_and_replays() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-live",
                    "workspace-1",
                    "tab-live",
                    "pane-live",
                    Some(provider_session("session-live")),
                )],
                Vec::new(),
            ))
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
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-live")
            })
            .unwrap();
        assert_eq!(candidate.availability, WorkerAvailability::UnassignedLive);

        let command = ConfirmWorkerAllocation {
            command_id: "assign-live-worker".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id.clone(),
            expected_worker_version: candidate.worker.version,
            profile_id: None,
            expected_profile_version: None,
            expected_project_version: project.version,
            objective: "Implement the live worker path.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        let started = store
            .begin_worker_allocation(&project.id, command.clone())
            .await
            .unwrap();
        let BeginWorkerAllocation::Started(context) = started else {
            panic!("expected a new worker allocation");
        };
        assert!(!context.replace_runtime);
        assert_eq!(context.worker.id, candidate.worker.id);
        let activated = store
            .activate_worker_allocation(&command.command_id)
            .await
            .unwrap();
        let replayed = store
            .begin_worker_allocation(&project.id, command)
            .await
            .unwrap();
        let BeginWorkerAllocation::Replayed(replayed) = replayed else {
            panic!("expected allocation replay");
        };

        assert_eq!(activated.assignment.worker.id, candidate.worker.id);
        assert_eq!(activated.assignment.profile_id, BLANK_WORKER_PROFILE_ID);
        assert_eq!(activated.assignment.profile_name, BLANK_WORKER_PROFILE_NAME);
        assert!(
            store
                .list_worker_profiles()
                .await
                .unwrap()
                .profiles
                .is_empty()
        );
        assert_eq!(
            activated.allocation.mode,
            yard_domain::AllocationMode::AdoptExisting
        );
        assert_eq!(replayed.assignment.id, activated.assignment.id);
        assert!(replayed.replayed);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn worker_projection_classifies_all_six_availability_states() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, mut orchestrator) = draft("workspace-1", "terminal-orchestrator");
        orchestrator.provider_session = Some(provider_session("session-orchestrator"));
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![
                    observed_worker(
                        "terminal-orchestrator",
                        "workspace-1",
                        "tab-orchestrator",
                        "pane-orchestrator",
                        Some(provider_session("session-orchestrator")),
                    ),
                    observed_worker(
                        "terminal-assigned",
                        "workspace-1",
                        "tab-assigned",
                        "pane-assigned",
                        Some(provider_session("session-assigned")),
                    ),
                    observed_worker(
                        "terminal-live",
                        "workspace-live",
                        "tab-live",
                        "pane-live",
                        Some(provider_session("session-live")),
                    ),
                    observed_worker(
                        "terminal-resumable",
                        "workspace-resumable",
                        "tab-resumable",
                        "pane-resumable",
                        Some(provider_session("session-resumable")),
                    ),
                    observed_worker(
                        "terminal-unavailable",
                        "workspace-unavailable",
                        "tab-unavailable",
                        "pane-unavailable",
                        None,
                    ),
                    observed_worker(
                        "terminal-ambiguous",
                        "workspace-ambiguous",
                        "tab-ambiguous",
                        "pane-ambiguous",
                        Some(provider_session("session-ambiguous")),
                    ),
                ],
                Vec::new(),
            ))
            .await
            .unwrap();
        let initial = store.list_worker_candidates().await.unwrap();
        let worker_for_terminal = |terminal_id: &str| {
            initial
                .workers
                .iter()
                .find(|candidate| {
                    candidate
                        .worker
                        .runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.terminal_id == terminal_id)
                })
                .unwrap()
                .worker
                .clone()
        };
        let assigned = worker_for_terminal("terminal-assigned");
        let resumable = worker_for_terminal("terminal-resumable");
        let ambiguous = worker_for_terminal("terminal-ambiguous");
        store
            .begin_worker_allocation(
                &project.id,
                ConfirmWorkerAllocation {
                    command_id: "classify-assigned".to_owned(),
                    actor: "local-user".to_owned(),
                    worker_id: assigned.id,
                    expected_worker_version: assigned.version,
                    profile_id: Some(profile.id.clone()),
                    expected_profile_version: Some(profile.version),
                    expected_project_version: project.version,
                    objective: "Reserve this worker.".to_owned(),
                    role: "implementer".to_owned(),
                    isolation_policy: IsolationPolicy::ProjectWorkspace,
                },
            )
            .await
            .unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE workers
                    SET profile_id = ?1, profile_version = ?2
                  WHERE id = ?3",
                rusqlite::params![
                    profile.id,
                    i64::try_from(profile.version).unwrap(),
                    resumable.id
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE worker_runtime_bindings
                    SET process_state = 'exited', observed_status = 'unknown'
                  WHERE worker_id = ?1",
                [resumable.id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE worker_runtime_bindings
                    SET observation_state = 'ambiguous',
                        process_state = 'unknown', observed_status = 'unknown'
                  WHERE worker_id = ?1",
                [ambiguous.id],
            )
            .unwrap();
        drop(connection);

        let candidates = store.list_worker_candidates().await.unwrap();
        let states = candidates
            .workers
            .iter()
            .map(|candidate| candidate.availability)
            .collect::<HashSet<_>>();
        assert_eq!(
            states,
            HashSet::from([
                WorkerAvailability::Orchestrator,
                WorkerAvailability::Assigned,
                WorkerAvailability::UnassignedLive,
                WorkerAvailability::Resumable,
                WorkerAvailability::Unavailable,
                WorkerAvailability::Ambiguous,
            ])
        );
        let assigned = candidates
            .workers
            .iter()
            .find(|candidate| candidate.availability == WorkerAvailability::Assigned)
            .unwrap();
        assert_eq!(assigned.project_id.as_deref(), Some(project.id.as_str()));
        assert!(assigned.assignment_id.is_some());
    }

    #[tokio::test]
    async fn resumable_worker_replaces_runtime_without_changing_identity() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_id, active) = create_active_assignment(&store).await;
        store
            .record_completion_receipt(
                &project_id,
                &active.id,
                RecordCompletionReceipt {
                    command_id: "complete-before-resume".to_owned(),
                    actor: "local-user".to_owned(),
                    attempt_id: active.attempt.id.clone(),
                    expected_assignment_version: active.version,
                    expected_attempt_version: active.attempt.version,
                    outcome: CompletionOutcome::Completed,
                    summary: "The first assignment is complete.".to_owned(),
                    artifact_refs: Vec::new(),
                    artifact_ids: Vec::new(),
                    evidence_refs: vec!["test://first-assignment".to_owned()],
                    unresolved_blockers: Vec::new(),
                },
            )
            .await
            .unwrap();
        let candidate = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .into_iter()
            .find(|candidate| candidate.worker.id == active.worker.id)
            .unwrap();
        assert_eq!(candidate.availability, WorkerAvailability::Resumable);
        let project = store.get_project(&project_id).await.unwrap();
        let command = ConfirmWorkerAllocation {
            command_id: "resume-worker".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id.clone(),
            expected_worker_version: candidate.worker.version,
            profile_id: None,
            expected_profile_version: None,
            expected_project_version: project.version,
            objective: "Continue with a second assignment.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        let started = store
            .begin_worker_allocation(&project_id, command.clone())
            .await
            .unwrap();
        let BeginWorkerAllocation::Started(context) = started else {
            panic!("expected a resumable worker allocation");
        };
        assert!(context.replace_runtime);

        let (_, mut replacement) = draft("workspace-1", "terminal-replacement");
        replacement.tab_id = Some("tab-replacement".to_owned());
        replacement.pane_id = "pane-replacement".to_owned();
        replacement.provider_session = Some(provider_session("session-replacement"));
        replacement.owns_tab = true;
        let persisted = store
            .replace_worker_runtime(&command.command_id, replacement)
            .await
            .unwrap();
        let activated = store
            .activate_worker_allocation(&command.command_id)
            .await
            .unwrap();

        assert_eq!(persisted.assignment.worker.id, active.worker.id);
        assert_eq!(activated.assignment.worker.id, active.worker.id);
        assert_eq!(
            activated
                .assignment
                .worker
                .runtime
                .as_ref()
                .unwrap()
                .terminal_id,
            "terminal-replacement"
        );
    }

    #[tokio::test]
    async fn ambiguous_worker_delivery_retains_reservation_and_never_reenters() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-ambiguous-delivery",
                    "workspace-1",
                    "tab-ambiguous-delivery",
                    "pane-ambiguous-delivery",
                    Some(provider_session("session-ambiguous-delivery")),
                )],
                Vec::new(),
            ))
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
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-ambiguous-delivery")
            })
            .unwrap();
        let command = ConfirmWorkerAllocation {
            command_id: "ambiguous-worker-allocation".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id.clone(),
            expected_worker_version: candidate.worker.version,
            profile_id: Some(profile.id),
            expected_profile_version: Some(profile.version),
            expected_project_version: project.version,
            objective: "Deliver exactly once.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        store
            .begin_worker_allocation(&project.id, command.clone())
            .await
            .unwrap();
        store
            .fail_worker_allocation(&command.command_id, "socket closed after submission", true)
            .await
            .unwrap();
        let replay = store
            .begin_worker_allocation(&project.id, command)
            .await
            .unwrap_err();
        let candidate = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .into_iter()
            .find(|candidate_after| candidate_after.worker.id == candidate.worker.id)
            .unwrap();

        assert!(matches!(
            replay,
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        assert_eq!(candidate.availability, WorkerAvailability::Assigned);
        assert!(candidate.assignment_id.is_some());
    }

    #[tokio::test]
    async fn concurrent_worker_allocations_allow_exactly_one_reservation() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, orchestrator) = draft("workspace-1", "terminal-1");
        let project = store
            .create_project(project_draft, orchestrator)
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Implementer"),
            })
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                20,
                vec![observed_worker(
                    "terminal-concurrent",
                    "workspace-1",
                    "tab-concurrent",
                    "pane-concurrent",
                    Some(provider_session("session-concurrent")),
                )],
                Vec::new(),
            ))
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
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-concurrent")
            })
            .unwrap();
        let command = |command_id: &str| ConfirmWorkerAllocation {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id.clone(),
            expected_worker_version: candidate.worker.version,
            profile_id: Some(profile.id.clone()),
            expected_profile_version: Some(profile.version),
            expected_project_version: project.version,
            objective: "Reserve concurrently.".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        let first_store = store.clone();
        let second_store = store.clone();
        let first_project = project.id.clone();
        let second_project = project.id.clone();
        let (first, second) = tokio::join!(
            first_store.begin_worker_allocation(&first_project, command("concurrent-worker-first")),
            second_store
                .begin_worker_allocation(&second_project, command("concurrent-worker-second")),
        );
        let successes = usize::from(first.is_ok()) + usize::from(second.is_ok());
        let failures = [first.as_ref().err(), second.as_ref().err()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let assignments = store.list_project_assignments(&project.id).await.unwrap();

        assert_eq!(successes, 1);
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            failures[0],
            ProjectStoreError::ProjectVersionConflict { .. }
                | ProjectStoreError::WorkerVersionConflict { .. }
                | ProjectStoreError::WorkerNotAvailable { .. }
        ));
        assert_eq!(assignments.assignments.len(), 1);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn project_relationship_commands_replay_from_snapshots_after_delete() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (source_draft, source_runtime) = draft("relationship-source", "terminal-source");
        let source = store
            .create_project(source_draft, source_runtime)
            .await
            .unwrap();
        let (mut target_draft, target_runtime) = draft("relationship-target", "terminal-target");
        target_draft.name = "Target".to_owned();
        let target = store
            .create_project(target_draft, target_runtime)
            .await
            .unwrap();

        let command = CreateProjectRelationship {
            command_id: "create-relationship".to_owned(),
            actor: "local-user".to_owned(),
            relationship_id: "relationship-1".to_owned(),
            source_project_id: source.id.clone(),
            target_project_id: target.id.clone(),
            kind: ProjectRelationshipKind::DependsOn,
        };
        let mut missing_source = command.clone();
        missing_source.command_id = "missing-relationship-source".to_owned();
        missing_source.relationship_id = "relationship-missing-source".to_owned();
        missing_source.source_project_id = "missing-source".to_owned();
        assert!(matches!(
            store
                .create_project_relationship(missing_source)
                .await
                .unwrap_err(),
            ProjectStoreError::RelationshipSourceProjectNotFound
        ));
        let mut missing_target = command.clone();
        missing_target.command_id = "missing-relationship-target".to_owned();
        missing_target.relationship_id = "relationship-missing-target".to_owned();
        missing_target.target_project_id = "missing-target".to_owned();
        assert!(matches!(
            store
                .create_project_relationship(missing_target)
                .await
                .unwrap_err(),
            ProjectStoreError::RelationshipTargetProjectNotFound
        ));
        let created = store
            .create_project_relationship(command.clone())
            .await
            .unwrap();
        let replayed = store
            .create_project_relationship(command.clone())
            .await
            .unwrap();
        assert_eq!(created.relationship.version, 1);
        assert_eq!(created.relationship.created_by, "local-user");
        assert!(replayed.replayed);
        assert_eq!(replayed.relationship, created.relationship);
        assert_eq!(
            store
                .list_project_relationships()
                .await
                .unwrap()
                .relationships
                .as_slice(),
            std::slice::from_ref(&created.relationship)
        );

        let mut semantic_conflict = command.clone();
        semantic_conflict.command_id = "semantic-relationship-conflict".to_owned();
        semantic_conflict.relationship_id = "relationship-2".to_owned();
        assert!(matches!(
            store
                .create_project_relationship(semantic_conflict)
                .await
                .unwrap_err(),
            ProjectStoreError::ProjectRelationshipAlreadyExists
        ));
        let mut id_conflict = command.clone();
        id_conflict.command_id = "relationship-id-conflict".to_owned();
        id_conflict.source_project_id = target.id.clone();
        id_conflict.target_project_id = source.id.clone();
        assert!(matches!(
            store
                .create_project_relationship(id_conflict)
                .await
                .unwrap_err(),
            ProjectStoreError::ProjectRelationshipIdConflict
        ));

        let delete = DeleteProjectRelationship {
            command_id: "delete-relationship".to_owned(),
            actor: "local-user".to_owned(),
            expected_version: created.relationship.version,
        };
        let mut stale_delete = delete.clone();
        stale_delete.command_id = "stale-delete-relationship".to_owned();
        stale_delete.expected_version += 1;
        assert!(matches!(
            store
                .delete_project_relationship(&created.relationship.id, stale_delete)
                .await
                .unwrap_err(),
            ProjectStoreError::ProjectRelationshipVersionConflict { current_version: 1 }
        ));
        let deleted = store
            .delete_project_relationship(&created.relationship.id, delete.clone())
            .await
            .unwrap();
        let delete_replay = store
            .delete_project_relationship(&created.relationship.id, delete)
            .await
            .unwrap();
        let create_replay = store.create_project_relationship(command).await.unwrap();

        assert!(delete_replay.replayed);
        assert_eq!(delete_replay.deleted_at_unix_ms, deleted.deleted_at_unix_ms);
        assert!(create_replay.replayed);
        assert_eq!(create_replay.relationship, created.relationship);
        assert!(
            store
                .list_project_relationships()
                .await
                .unwrap()
                .relationships
                .is_empty()
        );

        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let lifecycle_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type IN (
                        'project_relationship_created',
                        'project_relationship_deleted'
                    )",
                [&created.relationship.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lifecycle_count, 2);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn yard_orchestrator_routes_submit_replay_list_and_reserve_target() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (target_draft, target_runtime) = draft("route-target", "terminal-route-target");
        let target = store
            .create_project(target_draft, target_runtime)
            .await
            .unwrap();
        let (source_project_id, source_assignment) = create_active_assignment(&store).await;
        let source_project = store.get_project(&source_project_id).await.unwrap();
        let orchestrator = configure_yard_orchestrator_for_routes(&store).await;
        let command = yard_route_command("route-command", &orchestrator, &target);

        let started = store
            .begin_yard_orchestrator_route(command.clone())
            .await
            .unwrap();
        let BeginYardOrchestratorRoute::Started {
            orchestrator: started_orchestrator,
            target_project,
            ..
        } = started
        else {
            panic!("expected a new route");
        };
        assert_eq!(started_orchestrator.version, orchestrator.version);
        assert_eq!(target_project.id, target.id);
        assert_eq!(
            store
                .list_yard_orchestrator_routes(100)
                .await
                .unwrap()
                .routes[0]
                .status,
            CoordinationCommandStatus::Pending
        );

        let direct_prompt = SendOrchestratorPrompt {
            command_id: "direct-while-routed".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: target.version,
            orchestrator_worker_id: target.orchestrator.id.clone(),
            text: "This must wait.".to_owned(),
        };
        assert!(matches!(
            store
                .begin_orchestrator_prompt(&target.id, direct_prompt)
                .await
                .unwrap_err(),
            ProjectStoreError::OrchestratorInterventionInProgress
        ));
        assert!(matches!(
            store
                .begin_yard_orchestrator_route(yard_route_command(
                    "second-pending-route",
                    &orchestrator,
                    &target,
                ))
                .await
                .unwrap_err(),
            ProjectStoreError::OrchestratorInterventionInProgress
        ));

        assert!(matches!(
            store
                .begin_worker_handoff(
                    &source_project.id,
                    &source_assignment.id,
                    handoff_command(
                        "replacement-while-routed",
                        &source_project,
                        &source_assignment,
                        &target,
                        HandoffTargetRole::Orchestrator,
                    ),
                )
                .await
                .unwrap_err(),
            ProjectStoreError::OrchestratorInterventionInProgress
        ));

        store
            .reconcile_runtime_inventory(inventory(
                30,
                vec![
                    observed_worker(
                        "terminal-yard-routes",
                        "workspace-yard-routes",
                        "tab-yard-routes",
                        "pane-yard-routes",
                        Some(provider_session("session-yard-routes")),
                    ),
                    observed_worker(
                        "terminal-yard-replacement",
                        "workspace-yard-replacement",
                        "tab-yard-replacement",
                        "pane-yard-replacement",
                        Some(provider_session("session-yard-replacement")),
                    ),
                ],
                Vec::new(),
            ))
            .await
            .unwrap();
        let replacement = store
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
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-yard-replacement")
            })
            .unwrap();
        assert!(matches!(
            store
                .configure_yard_orchestrator(ConfigureYardOrchestrator {
                    command_id: "replace-yard-while-routed".to_owned(),
                    actor: "local-user".to_owned(),
                    worker_id: replacement.worker.id,
                    expected_worker_version: replacement.worker.version,
                    expected_orchestrator_version: orchestrator.version,
                })
                .await
                .unwrap_err(),
            ProjectStoreError::YardOrchestratorInterventionInProgress
        ));

        let submitted = store
            .succeed_yard_orchestrator_route(&command.command_id, "accepted")
            .await
            .unwrap();
        assert_eq!(submitted.status, CoordinationCommandStatus::Submitted);
        assert_eq!(submitted.runtime_status.as_deref(), Some("accepted"));
        assert!(submitted.submitted_at_unix_ms.is_some());
        let replayed = store
            .begin_yard_orchestrator_route(command.clone())
            .await
            .unwrap();
        let BeginYardOrchestratorRoute::Replayed(replayed) = replayed else {
            panic!("expected route replay");
        };
        assert_eq!(replayed, submitted);

        let mut conflict = command;
        conflict.text = "Different route text.".to_owned();
        assert!(matches!(
            store
                .begin_yard_orchestrator_route(conflict)
                .await
                .unwrap_err(),
            ProjectStoreError::IdempotencyConflict
        ));
        assert!(matches!(
            store.list_yard_orchestrator_routes(0).await.unwrap_err(),
            ProjectStoreError::RouteListLimitInvalid { .. }
        ));

        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events
                  WHERE aggregate_id = ?1
                    AND event_type = 'yard_orchestrator_route_submitted'",
                [&target.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn failed_and_interrupted_routes_never_reenter_submission() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (target_draft, target_runtime) =
            draft("route-recovery-target", "terminal-route-recovery");
        let target = store
            .create_project(target_draft, target_runtime)
            .await
            .unwrap();
        let orchestrator = configure_yard_orchestrator_for_routes(&store).await;

        let failed = yard_route_command("failed-route", &orchestrator, &target);
        store
            .begin_yard_orchestrator_route(failed.clone())
            .await
            .unwrap();
        store
            .fail_yard_orchestrator_route(&failed.command_id, "transport rejected", false)
            .await
            .unwrap();
        assert!(matches!(
            store
                .begin_yard_orchestrator_route(failed)
                .await
                .unwrap_err(),
            ProjectStoreError::CommandPreviouslyFailed(_)
        ));

        let interrupted = yard_route_command("interrupted-route", &orchestrator, &target);
        store
            .begin_yard_orchestrator_route(interrupted.clone())
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        assert!(matches!(
            reopened
                .begin_yard_orchestrator_route(interrupted)
                .await
                .unwrap_err(),
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
        let routes = reopened.list_yard_orchestrator_routes(100).await.unwrap();
        assert_eq!(
            routes.routes[0].status,
            CoordinationCommandStatus::Ambiguous
        );
        assert!(
            routes.routes[0]
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("Yard restarted"))
        );
        assert_eq!(routes.routes[1].status, CoordinationCommandStatus::Failed);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn latest_delivered_orchestrator_commands_use_state_recency_and_direct_precedence() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (target_draft, target_runtime) = draft("status-target", "terminal-status-target");
        let target = store
            .create_project(target_draft, target_runtime)
            .await
            .unwrap();
        let orchestrator = configure_yard_orchestrator_for_routes(&store).await;

        let direct = SendOrchestratorPrompt {
            command_id: "project-direct".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: target.version,
            orchestrator_worker_id: target.orchestrator.id.clone(),
            text: "Direct project command.".to_owned(),
        };
        store
            .begin_orchestrator_prompt(&target.id, direct.clone())
            .await
            .unwrap();
        assert_eq!(
            store
                .latest_delivered_project_orchestrator_command_id(&target.id)
                .await
                .unwrap(),
            None
        );
        store
            .succeed_orchestrator_prompt(&direct.command_id, "working")
            .await
            .unwrap();

        let central = SendYardOrchestratorPrompt {
            command_id: "central-direct".to_owned(),
            actor: "local-user".to_owned(),
            expected_orchestrator_version: orchestrator.version,
            orchestrator_worker_id: orchestrator.worker.as_ref().unwrap().id.clone(),
            text: "Direct central command.".to_owned(),
        };
        store
            .begin_yard_orchestrator_prompt(central.clone())
            .await
            .unwrap();
        assert_eq!(
            store
                .latest_delivered_yard_orchestrator_command_id()
                .await
                .unwrap(),
            None
        );
        store
            .succeed_yard_orchestrator_prompt(&central.command_id, "working")
            .await
            .unwrap();

        let route = yard_route_command("project-route", &orchestrator, &target);
        store
            .begin_yard_orchestrator_route(route.clone())
            .await
            .unwrap();
        store
            .succeed_yard_orchestrator_route(&route.command_id, "working")
            .await
            .unwrap();

        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE orchestrator_prompt_commands
                    SET submitted_at_unix_ms = 100
                  WHERE command_id = 'project-direct'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE yard_orchestrator_route_commands
                    SET submitted_at_unix_ms = 200
                  WHERE command_id = 'project-route'",
                [],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            store
                .latest_delivered_project_orchestrator_command_id(&target.id)
                .await
                .unwrap()
                .as_deref(),
            Some("project-route")
        );

        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE orchestrator_prompt_commands
                    SET submitted_at_unix_ms = 300
                  WHERE command_id = 'project-direct'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE yard_orchestrator_route_commands
                    SET submitted_at_unix_ms = 300
                  WHERE command_id = 'project-route'",
                [],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            store
                .latest_delivered_project_orchestrator_command_id(&target.id)
                .await
                .unwrap()
                .as_deref(),
            Some("project-direct")
        );
        assert_eq!(
            store
                .latest_delivered_yard_orchestrator_command_id()
                .await
                .unwrap()
                .as_deref(),
            Some("central-direct")
        );

        let failed_central = SendYardOrchestratorPrompt {
            command_id: "central-failed".to_owned(),
            text: "This command failed delivery.".to_owned(),
            ..central
        };
        store
            .begin_yard_orchestrator_prompt(failed_central.clone())
            .await
            .unwrap();
        store
            .fail_yard_orchestrator_prompt(
                &failed_central.command_id,
                "runtime rejected delivery",
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .latest_delivered_yard_orchestrator_command_id()
                .await
                .unwrap()
                .as_deref(),
            Some("central-direct")
        );
    }

    fn node_placement() -> CanvasPlacement {
        CanvasPlacement {
            x: 120.0,
            y: 90.0,
            width: 360.0,
            height: 220.0,
        }
    }

    fn create_node_command(
        command_id: &str,
        kind: CoordinationNodeKind,
        project_ids: Vec<String>,
    ) -> CreateCoordinationNode {
        CreateCoordinationNode {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            name: "Release coordination".to_owned(),
            kind,
            placement: node_placement(),
            attached_project_ids: project_ids,
        }
    }

    #[tokio::test]
    async fn coordination_node_commands_are_versioned_idempotent_and_scoped() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, runtime) = draft("node-project", "node-project-terminal");
        let project = store.create_project(project_draft, runtime).await.unwrap();
        let node_id = Uuid::now_v7().to_string();
        let create = create_node_command(
            "create-workstream-node",
            CoordinationNodeKind::Workstream,
            vec![project.id.clone()],
        );
        let created = store
            .create_coordination_node(
                &node_id,
                Some(
                    temp.path()
                        .join("coordination")
                        .join(&node_id)
                        .to_string_lossy()
                        .into_owned(),
                ),
                None,
                create.clone(),
            )
            .await
            .unwrap();
        let replayed = store
            .create_coordination_node(
                &Uuid::now_v7().to_string(),
                Some("/ignored/on/replay".to_owned()),
                None,
                create,
            )
            .await
            .unwrap();
        assert_eq!(replayed.node.id, created.node.id);
        assert!(replayed.replayed);

        let update = UpdateCoordinationNode {
            command_id: "update-workstream-node".to_owned(),
            actor: "local-user".to_owned(),
            expected_version: created.node.version,
            name: "Release train".to_owned(),
            attached_project_ids: vec![project.id.clone()],
        };
        let updated = store
            .update_coordination_node(&node_id, update.clone())
            .await
            .unwrap();
        assert_eq!(updated.node.version, 2);
        assert_eq!(updated.node.name, "Release train");
        assert!(
            store
                .update_coordination_node(
                    &node_id,
                    UpdateCoordinationNode {
                        command_id: "stale-node-update".to_owned(),
                        ..update
                    },
                )
                .await
                .is_err()
        );

        let placement = UpdateCoordinationNodePlacement {
            command_id: "move-workstream-node".to_owned(),
            actor: "local-user".to_owned(),
            expected_version: updated.node.placement.version,
            placement: CanvasPlacement {
                x: 500.0,
                ..node_placement()
            },
        };
        let moved = store
            .update_coordination_node_placement(&node_id, placement.clone())
            .await
            .unwrap();
        assert_eq!(moved.node.version, updated.node.version);
        assert_eq!(moved.node.placement.version, 2);
        assert!(
            store
                .update_coordination_node_placement(&node_id, placement)
                .await
                .unwrap()
                .replayed
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn workstream_worker_is_protected_and_routes_only_to_attachments() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, runtime) = draft("route-project", "route-project-terminal");
        let project = store.create_project(project_draft, runtime).await.unwrap();
        let (other_draft, other_runtime) = draft("other-project", "other-project-terminal");
        let other = store
            .create_project(other_draft, other_runtime)
            .await
            .unwrap();
        let node_id = Uuid::now_v7().to_string();
        let node = store
            .create_coordination_node(
                &node_id,
                Some(
                    temp.path()
                        .join("coordination")
                        .join(&node_id)
                        .to_string_lossy()
                        .into_owned(),
                ),
                None,
                create_node_command(
                    "create-routed-node",
                    CoordinationNodeKind::Workstream,
                    vec![project.id.clone()],
                ),
            )
            .await
            .unwrap()
            .node;
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: profile_spec("Coordination"),
            })
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                50,
                vec![observed_worker(
                    "coordination-terminal",
                    "coordination-workspace",
                    "coordination-tab",
                    "coordination-pane",
                    Some(provider_session("coordination-session")),
                )],
                Vec::new(),
            ))
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
                    .is_some_and(|runtime| runtime.terminal_id == "coordination-terminal")
            })
            .unwrap();
        let pinned = store
            .pin_worker_profile(&candidate.worker.id, &profile.id, profile.version)
            .await
            .unwrap();
        let provision = ProvisionCoordinationNode {
            command_id: "provision-routed-node".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_node_version: node.version,
        };
        let provisioned = store
            .configure_coordination_node(&node_id, provision, &pinned.id, pinned.version)
            .await
            .unwrap()
            .node;
        let protected = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .into_iter()
            .find(|candidate| candidate.worker.id == pinned.id)
            .unwrap();
        assert_eq!(protected.availability, WorkerAvailability::CoordinationNode);
        assert!(matches!(
            store
                .end_worker_session(
                    &pinned.id,
                    EndWorkerSession {
                        command_id: "end-coordination-worker".to_owned(),
                        actor: "local-user".to_owned(),
                        expected_worker_version: protected.worker.version,
                        expected_runtime_version: protected
                            .worker
                            .runtime
                            .as_ref()
                            .map(|runtime| runtime.version),
                    },
                )
                .await
                .unwrap_err(),
            ProjectStoreError::CoordinationNodeSessionEndForbidden
        ));

        let prompt = SendCoordinationNodePrompt {
            command_id: "prompt-coordination-node".to_owned(),
            actor: "local-user".to_owned(),
            expected_node_version: provisioned.version,
            worker_id: pinned.id.clone(),
            text: "Report release blockers.".to_owned(),
        };
        assert!(matches!(
            store
                .begin_coordination_node_prompt(&node_id, prompt.clone())
                .await
                .unwrap(),
            BeginCoordinationNodePrompt::Started { .. }
        ));
        store
            .succeed_coordination_node_prompt(&prompt.command_id, "accepted")
            .await
            .unwrap();
        assert!(matches!(
            store
                .begin_coordination_node_prompt(&node_id, prompt)
                .await
                .unwrap(),
            BeginCoordinationNodePrompt::Replayed(_)
        ));

        let route = SendCoordinationNodeRoute {
            command_id: "route-attached-project".to_owned(),
            actor: "local-user".to_owned(),
            expected_node_version: provisioned.version,
            worker_id: pinned.id.clone(),
            target_project_id: project.id.clone(),
            expected_project_version: project.version,
            target_orchestrator_worker_id: project.orchestrator.id.clone(),
            text: "Write the integration status.".to_owned(),
        };
        assert!(matches!(
            store
                .begin_coordination_node_route(&node_id, route.clone())
                .await
                .unwrap(),
            BeginCoordinationNodeRoute::Started { .. }
        ));
        let unscoped = SendCoordinationNodeRoute {
            command_id: "route-unattached-project".to_owned(),
            target_project_id: other.id,
            expected_project_version: other.version,
            target_orchestrator_worker_id: other.orchestrator.id,
            expected_node_version: provisioned.version,
            worker_id: pinned.id,
            actor: "local-user".to_owned(),
            text: "This must be rejected.".to_owned(),
        };
        assert!(matches!(
            store
                .begin_coordination_node_route(&node_id, unscoped)
                .await
                .unwrap_err(),
            ProjectStoreError::CoordinationNodeProjectNotAttached
        ));
        drop(store);
        let reopened = open_store(&temp).await;
        assert!(matches!(
            reopened
                .begin_coordination_node_route(&node_id, route)
                .await
                .unwrap_err(),
            ProjectStoreError::CommandOutcomeAmbiguous(_)
        ));
    }

    #[tokio::test]
    async fn knowledge_snapshots_capture_revisions_and_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (project_draft, runtime) = draft("knowledge-project", "knowledge-terminal");
        let project = store.create_project(project_draft, runtime).await.unwrap();
        let node_id = Uuid::now_v7().to_string();
        let node_folder = temp.path().join("knowledge").join(&node_id);
        let node = store
            .create_coordination_node(
                &node_id,
                None,
                Some(node_folder.to_string_lossy().into_owned()),
                create_node_command(
                    "create-knowledge-node",
                    CoordinationNodeKind::KnowledgeStore,
                    vec![project.id.clone()],
                ),
            )
            .await
            .unwrap()
            .node;
        let snapshot_id = Uuid::now_v7().to_string();
        let snapshot_folder = node_folder.join("snapshots").join(&snapshot_id);
        let project_folder = snapshot_folder.join("projects").join(&project.id);
        let request = RequestCoordinationSnapshot {
            command_id: "request-knowledge-snapshot".to_owned(),
            actor: "local-user".to_owned(),
            expected_node_version: node.version,
        };
        let snapshot = store
            .create_coordination_snapshot(
                &node_id,
                &snapshot_id,
                snapshot_folder.to_string_lossy().into_owned(),
                vec![SnapshotProjectFolder {
                    project_id: project.id.clone(),
                    folder_path: project_folder.to_string_lossy().into_owned(),
                }],
                request.clone(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.projects[0].project_version, project.version);
        assert_eq!(snapshot.progress.completed, 0);
        assert_eq!(snapshot.progress.total, 1);
        store
            .record_snapshot_project_delivery(
                &snapshot_id,
                &project.id,
                SnapshotDeliveryResult::Submitted {
                    runtime_status: "accepted".to_owned(),
                },
            )
            .await
            .unwrap();
        store
            .record_snapshot_project_collected(&snapshot_id, &project.id)
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let preserved = reopened
            .get_coordination_snapshot(&node_id, &snapshot_id)
            .await
            .unwrap();
        let replayed = reopened
            .create_coordination_snapshot(
                &node_id,
                &Uuid::now_v7().to_string(),
                "/ignored/on/replay".to_owned(),
                Vec::new(),
                request,
            )
            .await
            .unwrap();
        assert_eq!(preserved.progress.completed, 1);
        assert_eq!(
            preserved.projects[0].collection_status,
            SnapshotCollectionStatus::Collected
        );
        assert_eq!(replayed.id, snapshot_id);
        assert!(replayed.replayed);
    }

    #[tokio::test]
    async fn v18_migration_preserves_v16_projects_and_reopens() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = open_store(&temp).await;
        let (project_draft, runtime) = draft("migration-project", "migration-terminal");
        let project = store.create_project(project_draft, runtime).await.unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        drop_automation_schema_for_downgrade(&connection);
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE coordination_snapshot_projects;
                 DROP TABLE coordination_snapshots;
                 DROP TABLE coordination_node_route_commands;
                 DROP TABLE coordination_node_prompt_commands;
                 DROP TABLE coordination_node_provision_commands;
                 DROP TABLE coordination_node_placement_commands;
                 DROP TABLE coordination_node_update_commands;
                 DROP TABLE coordination_node_create_commands;
                 DROP TABLE coordination_node_projects;
                 DROP TABLE coordination_node_placements;
                 DROP TABLE coordination_nodes;
                 PRAGMA user_version = 16;
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();
        drop(connection);

        let migrated = SqliteProjectStore::open(&path).await.unwrap();
        assert_eq!(
            migrated.get_project(&project.id).await.unwrap().id,
            project.id
        );
        drop(migrated);
        let reopened = SqliteProjectStore::open(&path).await.unwrap();
        assert_eq!(
            reopened.get_project(&project.id).await.unwrap().id,
            project.id
        );
        drop(reopened);
        let connection = Connection::open(path).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 18);
    }
}

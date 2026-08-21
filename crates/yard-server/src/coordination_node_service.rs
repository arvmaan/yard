use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use thiserror::Error;
use tokio::{
    sync::Mutex,
    time::{Instant, sleep},
};
use uuid::Uuid;
use yard_domain::{
    CoordinationNode, CoordinationNodeCommandResult, CoordinationNodeKind,
    CoordinationNodePromptAcknowledgement, CoordinationNodeRoute, CoordinationNodeRoutes,
    CoordinationNodeTerminalOutput, CoordinationNodes, CoordinationSnapshot, CoordinationSnapshots,
    CreateCoordinationNode, OrchestratorStatusReport, ProvisionCoordinationNode,
    RequestCoordinationSnapshot, SendCoordinationNodePrompt, SendCoordinationNodeRoute,
    SnapshotCollectionStatus, UpdateCoordinationNode, UpdateCoordinationNodePlacement, Worker,
    WorkerAvailability, WorkerRuntimeBinding,
};
use yard_store::{
    BeginCoordinationNodePrompt, BeginCoordinationNodeRoute, ProjectStoreError,
    SnapshotDeliveryResult, SnapshotProjectFolder, TokenSpendCommandSource, YardStore,
};

use crate::{
    allocation_service::{
        AllocationServiceError, RuntimeControl, RuntimeProvisionError, RuntimeSessionRequest,
        RuntimeWorkspaceProvisionRequest, assignment_prompt, provider_args,
        validate_supported_profile,
    },
    intervention_service::{
        RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest, RuntimePromptRequest,
    },
    inventory_service::{InventoryServiceError, InventorySource},
    reconciliation_service::{ReconciliationService, ReconciliationServiceError},
    status_protocol::{
        validate_executable_orchestrator_workflow, with_orchestrator_status_contract,
        with_orchestrator_workflow,
    },
};

pub const COORDINATION_SESSION: &str = "yard-coordination";

const RUNTIME_IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);
const REQUIRED_SNAPSHOT_FILES: [&str; 8] = [
    "overview.md",
    "objectives.md",
    "changes.md",
    "decisions.md",
    "interfaces.md",
    "risks.md",
    "next-actions.md",
    "sources.md",
];

#[derive(Clone)]
pub struct CoordinationNodeService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    interventions: Arc<dyn RuntimeIntervention>,
    store: Arc<dyn YardStore>,
    reconciliation: ReconciliationService,
    coordination_root: PathBuf,
    knowledge_root: PathBuf,
    operation: Arc<Mutex<()>>,
}

impl CoordinationNodeService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        interventions: Arc<dyn RuntimeIntervention>,
        store: Arc<dyn YardStore>,
        reconciliation: ReconciliationService,
        coordination_root: PathBuf,
        knowledge_root: PathBuf,
    ) -> Self {
        Self {
            source,
            runtime,
            interventions,
            store,
            reconciliation,
            coordination_root,
            knowledge_root,
            operation: Arc::default(),
        }
    }

    pub(crate) async fn list(&self) -> Result<CoordinationNodes, CoordinationNodeServiceError> {
        self.store
            .list_coordination_nodes()
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn get(
        &self,
        node_id: &str,
    ) -> Result<CoordinationNode, CoordinationNodeServiceError> {
        self.store
            .get_coordination_node(node_id)
            .await
            .map_err(Into::into)
    }

    /// Create a node with a backend-owned folder.
    ///
    /// The request has no path field. A failed or replayed command removes
    /// only the empty UUID directory allocated by this attempt.
    pub(crate) async fn create(
        &self,
        command: CreateCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, CoordinationNodeServiceError> {
        let command = command.normalize()?;
        let node_id = Uuid::now_v7().to_string();
        let (cwd, folder_path, allocated) = match command.kind {
            CoordinationNodeKind::Workstream => {
                let path =
                    create_managed_path(&self.coordination_root, &["nodes", node_id.as_str()])?;
                (Some(path_string(&path)?), None, path)
            }
            CoordinationNodeKind::KnowledgeStore => {
                let path = create_managed_path(&self.knowledge_root, &["nodes", node_id.as_str()])?;
                (None, Some(path_string(&path)?), path)
            }
        };
        let result = self
            .store
            .create_coordination_node(&node_id, cwd, folder_path, command)
            .await;
        match result {
            Ok(result) => {
                if result.node.id != node_id {
                    remove_empty_managed_path(&allocated);
                }
                Ok(result)
            }
            Err(error) => {
                remove_empty_managed_path(&allocated);
                Err(error.into())
            }
        }
    }

    pub(crate) async fn update(
        &self,
        node_id: &str,
        command: UpdateCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, CoordinationNodeServiceError> {
        self.store
            .update_coordination_node(node_id, command)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn update_placement(
        &self,
        node_id: &str,
        command: UpdateCoordinationNodePlacement,
    ) -> Result<CoordinationNodeCommandResult, CoordinationNodeServiceError> {
        self.store
            .update_coordination_node_placement(node_id, command)
            .await
            .map_err(Into::into)
    }

    /// Provision one profile-backed worker in the shared coordination session.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn provision(
        &self,
        node_id: &str,
        command: ProvisionCoordinationNode,
    ) -> Result<CoordinationNodeCommandResult, CoordinationNodeServiceError> {
        let command = command.normalize()?;
        let _operation = self.operation.lock().await;
        let node = self.store.get_coordination_node(node_id).await?;
        if node.kind != CoordinationNodeKind::Workstream {
            return Err(CoordinationNodeServiceError::WrongNodeKind);
        }
        let cwd = node
            .cwd
            .as_deref()
            .ok_or(CoordinationNodeServiceError::ManagedPathMissing)?;
        validate_recorded_managed_path(&self.coordination_root, &node.id, cwd)?;
        if let Some(worker) = node.worker.as_ref() {
            return self
                .store
                .configure_coordination_node(&node.id, command, &worker.id, worker.version)
                .await
                .map_err(Into::into);
        }
        let profile = self.store.get_worker_profile(&command.profile_id).await?;
        if profile.version != command.expected_profile_version {
            return Err(ProjectStoreError::ProfileVersionConflict {
                current_version: profile.version,
            }
            .into());
        }
        validate_supported_profile(&profile)
            .map_err(CoordinationNodeServiceError::UnsupportedProfile)?;
        let args = provider_args(&profile).map_err(|error| match error {
            AllocationServiceError::UnsupportedProfile(message) => {
                CoordinationNodeServiceError::UnsupportedProfile(message)
            }
            other => CoordinationNodeServiceError::UnsupportedProfile(other.to_string()),
        })?;
        let root = ensure_managed_root(&self.coordination_root)?;
        self.runtime
            .ensure_session(RuntimeSessionRequest {
                session: COORDINATION_SESSION.to_owned(),
                startup_cwd: path_string(&root)?,
            })
            .await
            .map_err(runtime_error)?;
        let (inventory, _) = self.reconciliation.refresh(COORDINATION_SESSION).await?;
        let mut observed = dedicated_worker(&inventory, &node);
        self.store
            .begin_coordination_node_runtime_provision(&node.id, command.clone())
            .await?;
        if observed.is_none() {
            let provision = RuntimeWorkspaceProvisionRequest {
                command_id: command.command_id.clone(),
                session: COORDINATION_SESSION.to_owned(),
                workspace_label: workspace_label(&node.id),
                cwd: cwd.to_owned(),
                agent_name: agent_name(&node.id),
                kind: profile.spec.provider.clone(),
                args,
                prompt: with_orchestrator_status_contract(
                    &assignment_prompt(
                        &format!(
                            "Coordinate the '{}' workstream across only its attached Yard \
                             projects. Track objectives, dependencies, interfaces, decisions, \
                             risks, and next actions without claiming project completion.",
                            node.name
                        ),
                        "coordination workstream",
                        &profile,
                    ),
                    &command.command_id,
                ),
            };
            let prepared = match self
                .runtime
                .prepare_workspace_worker(provision.clone())
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    return self
                        .fail_dedicated_preparation(&command.command_id, error)
                        .await;
                }
            };
            if let Err(error) = self
                .store
                .claim_provisioning_runtime(&command.command_id, prepared.clone())
                .await
            {
                self.store
                    .quarantine_provisioning_runtime(&command.command_id, prepared)
                    .await?;
                self.store
                    .fail_dedicated_runtime_provision(&command.command_id, &error.to_string(), true)
                    .await?;
                return Err(CoordinationNodeServiceError::RuntimeProvisionAmbiguous(
                    error.to_string(),
                ));
            }
            let runtime = match self
                .runtime
                .start_prepared_workspace_worker(provision, prepared)
                .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    return self.fail_dedicated_start(&command.command_id, error).await;
                }
            };
            if let Err(error) = self
                .store
                .confirm_dedicated_runtime_provision(&command.command_id, runtime.clone())
                .await
            {
                self.store
                    .quarantine_provisioning_runtime(&command.command_id, runtime)
                    .await?;
                self.store
                    .fail_dedicated_runtime_provision(&command.command_id, &error.to_string(), true)
                    .await?;
                return Err(CoordinationNodeServiceError::RuntimeProvisionAmbiguous(
                    error.to_string(),
                ));
            }
            observed = Some(
                match self
                    .reconcile_provisioned_worker(&node, &runtime.terminal_id)
                    .await
                {
                    Ok(observed) => observed,
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command.command_id, runtime.clone())
                            .await?;
                        self.store
                            .fail_dedicated_runtime_provision(
                                &command.command_id,
                                &error.to_string(),
                                true,
                            )
                            .await?;
                        return Err(error);
                    }
                },
            );
        }
        let observed = observed.ok_or(CoordinationNodeServiceError::RuntimeBindingUnverified)?;
        let candidate = self
            .store
            .list_worker_candidates()
            .await?
            .workers
            .into_iter()
            .find(|candidate| {
                candidate.worker.runtime.as_ref().is_some_and(|runtime| {
                    runtime.adapter == "herdr"
                        && runtime.session == COORDINATION_SESSION
                        && runtime.terminal_id == observed.terminal_id
                })
            })
            .ok_or(CoordinationNodeServiceError::ReconciledWorkerMissing)?;
        if !matches!(
            candidate.availability,
            WorkerAvailability::UnassignedLive | WorkerAvailability::CoordinationNode
        ) {
            return Err(ProjectStoreError::WorkerNotAvailable {
                availability: candidate.availability,
            }
            .into());
        }
        let worker = self
            .store
            .pin_worker_profile(
                &candidate.worker.id,
                &profile.id,
                command.expected_profile_version,
            )
            .await?;
        self.store
            .configure_coordination_node(&node.id, command, &worker.id, worker.version)
            .await
            .map_err(Into::into)
    }

    async fn fail_dedicated_preparation(
        &self,
        command_id: &str,
        error: RuntimeProvisionError,
    ) -> Result<CoordinationNodeCommandResult, CoordinationNodeServiceError> {
        let (message, ambiguous, runtime, objective_delivery) = provisioning_failure(error);
        if let Some(runtime) = runtime {
            self.store
                .quarantine_provisioning_runtime(command_id, runtime)
                .await?;
        }
        self.store
            .fail_dedicated_runtime_provision(command_id, &message, ambiguous)
            .await?;
        Err(coordination_provision_error(
            message,
            ambiguous,
            objective_delivery,
        ))
    }

    async fn fail_dedicated_start(
        &self,
        command_id: &str,
        error: RuntimeProvisionError,
    ) -> Result<CoordinationNodeCommandResult, CoordinationNodeServiceError> {
        let (message, mut ambiguous, runtime, objective_delivery) = provisioning_failure(error);
        if let Some(runtime) = runtime {
            ambiguous = true;
            self.store
                .quarantine_provisioning_runtime(command_id, runtime)
                .await?;
        } else if !ambiguous {
            self.store
                .release_provisioning_runtime_claim(command_id)
                .await?;
        }
        self.store
            .fail_dedicated_runtime_provision(command_id, &message, ambiguous)
            .await?;
        Err(coordination_provision_error(
            message,
            ambiguous,
            objective_delivery,
        ))
    }

    pub(crate) async fn prompt(
        &self,
        node_id: &str,
        command: SendCoordinationNodePrompt,
    ) -> Result<CoordinationNodePromptAcknowledgement, CoordinationNodeServiceError> {
        self.prompt_with_scheduled_policy(node_id, command, false)
            .await
    }

    pub(crate) async fn prompt_scheduled_summary(
        &self,
        node_id: &str,
        command: SendCoordinationNodePrompt,
    ) -> Result<CoordinationNodePromptAcknowledgement, CoordinationNodeServiceError> {
        self.prompt_with_scheduled_policy(node_id, command, true)
            .await
    }

    async fn prompt_with_scheduled_policy(
        &self,
        node_id: &str,
        command: SendCoordinationNodePrompt,
        scheduled: bool,
    ) -> Result<CoordinationNodePromptAcknowledgement, CoordinationNodeServiceError> {
        if scheduled {
            self.ensure_scheduled_summaries_enabled().await?;
        }
        let (command, node) = match self
            .store
            .begin_coordination_node_prompt(
                node_id,
                command,
                if scheduled {
                    TokenSpendCommandSource::ScheduledSummary
                } else {
                    TokenSpendCommandSource::Manual
                },
            )
            .await?
        {
            BeginCoordinationNodePrompt::Replayed(acknowledgement) => return Ok(acknowledgement),
            BeginCoordinationNodePrompt::Started { command, node } => (command, *node),
        };
        let runtime = node_runtime(&node)?;
        if let Err(error) = self.validate_node_binding(&node).await {
            self.store
                .fail_coordination_node_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if scheduled && let Err(error) = self.ensure_scheduled_summaries_enabled().await {
            self.store
                .fail_coordination_node_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        let result = self
            .interventions
            .prompt(RuntimePromptRequest {
                command_id: command.command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: with_orchestrator_status_contract(&command.text, &command.command_id),
            })
            .await;
        match result {
            Ok(result) => {
                if let Err(error) = self.validate_node_binding(&node).await {
                    let error = RuntimeInterventionError::Ambiguous(format!(
                        "Herdr acknowledged the prompt, but the coordination node binding \
                         changed: {error}"
                    ));
                    self.store
                        .fail_coordination_node_prompt(
                            &command.command_id,
                            &error.to_string(),
                            true,
                        )
                        .await?;
                    return Err(error.into());
                }
                self.store
                    .succeed_coordination_node_prompt(&command.command_id, &result.status)
                    .await
                    .map_err(Into::into)
            }
            Err(error) => {
                let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                self.store
                    .fail_coordination_node_prompt(
                        &command.command_id,
                        &error.to_string(),
                        ambiguous,
                    )
                    .await?;
                Err(error.into())
            }
        }
    }

    async fn ensure_scheduled_summaries_enabled(&self) -> Result<(), CoordinationNodeServiceError> {
        if self
            .store
            .get_token_spend_settings()
            .await?
            .scheduled_automatic_summaries
        {
            Ok(())
        } else {
            Err(CoordinationNodeServiceError::AutomaticTokenSpendDisabled)
        }
    }

    pub(crate) async fn route(
        &self,
        node_id: &str,
        command: SendCoordinationNodeRoute,
    ) -> Result<CoordinationNodeRoute, CoordinationNodeServiceError> {
        let initial_project = self.store.get_project(&command.target_project_id).await?;
        let initial_workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                &initial_project.workflow_profile.profile_id,
                initial_project.workflow_profile.profile_version,
            )
            .await?;
        validate_executable_orchestrator_workflow(&initial_workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let (command, node, project, workflow_profile) = match self
            .store
            .begin_coordination_node_route(node_id, command)
            .await?
        {
            BeginCoordinationNodeRoute::Replayed(route) => return Ok(route),
            BeginCoordinationNodeRoute::Started {
                command,
                node,
                target_project,
                workflow_profile,
            } => (command, *node, *target_project, *workflow_profile),
        };
        let runtime = project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(CoordinationNodeServiceError::RuntimeBindingMissing)?;
        if let Err(error) = self.validate_route_bindings(&node, &project).await {
            self.store
                .fail_coordination_node_route(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        let prompt = with_orchestrator_workflow(&command.text, &workflow_profile)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let result = self
            .interventions
            .prompt(RuntimePromptRequest {
                command_id: command.command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: with_orchestrator_status_contract(&prompt, &command.command_id),
            })
            .await;
        match result {
            Ok(result) => {
                if let Err(error) = self.validate_route_bindings(&node, &project).await {
                    let error = RuntimeInterventionError::Ambiguous(format!(
                        "Herdr acknowledged the route, but a coordination binding changed: {error}"
                    ));
                    self.store
                        .fail_coordination_node_route(&command.command_id, &error.to_string(), true)
                        .await?;
                    return Err(error.into());
                }
                self.store
                    .succeed_coordination_node_route(&command.command_id, &result.status)
                    .await
                    .map_err(Into::into)
            }
            Err(error) => {
                let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                self.store
                    .fail_coordination_node_route(
                        &command.command_id,
                        &error.to_string(),
                        ambiguous,
                    )
                    .await?;
                Err(error.into())
            }
        }
    }

    pub(crate) async fn list_routes(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<CoordinationNodeRoutes, CoordinationNodeServiceError> {
        self.store
            .list_coordination_node_routes(node_id, limit)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn read_output(
        &self,
        node_id: &str,
        lines: u32,
    ) -> Result<CoordinationNodeTerminalOutput, CoordinationNodeServiceError> {
        if !(1..=1_000).contains(&lines) {
            return Err(CoordinationNodeServiceError::InvalidLineCount);
        }
        let node = self.store.get_coordination_node(node_id).await?;
        self.validate_node_binding(&node).await?;
        let worker = node
            .worker
            .as_ref()
            .ok_or(CoordinationNodeServiceError::NodeNotProvisioned)?;
        let runtime = node_runtime(&node)?;
        let result = self
            .interventions
            .read_output(RuntimeOutputRequest {
                request_id: Uuid::now_v7().to_string(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                lines,
            })
            .await?;
        validate_output_identity(runtime, &result)?;
        self.validate_node_binding(&node).await?;
        let expected_command_id = self
            .store
            .latest_delivered_coordination_node_command_id(&node.id)
            .await?;
        let status_report = expected_command_id.as_deref().and_then(|command_id| {
            OrchestratorStatusReport::scan_terminal_output_for_command(&result.text, command_id)
        });
        Ok(CoordinationNodeTerminalOutput {
            node_id: node.id,
            worker_id: worker.id.clone(),
            pane_id: result.pane_id,
            source: result.source,
            format: result.format,
            text: result.text,
            revision: result.revision,
            truncated: result.truncated,
            status_report,
        })
    }

    pub(crate) async fn request_snapshot(
        &self,
        node_id: &str,
        command: RequestCoordinationSnapshot,
    ) -> Result<CoordinationSnapshot, CoordinationNodeServiceError> {
        let command = command.normalize()?;
        let node = self.store.get_coordination_node(node_id).await?;
        if node.kind != CoordinationNodeKind::KnowledgeStore {
            return Err(CoordinationNodeServiceError::WrongNodeKind);
        }
        let recorded_folder = node
            .folder_path
            .as_deref()
            .ok_or(CoordinationNodeServiceError::ManagedPathMissing)?;
        validate_recorded_managed_path(&self.knowledge_root, &node.id, recorded_folder)?;
        let snapshot_id = Uuid::now_v7().to_string();
        let snapshot_folder = create_managed_path(
            &self.knowledge_root,
            &["nodes", &node.id, "snapshots", &snapshot_id],
        )?;
        let mut project_folders = Vec::with_capacity(node.attached_project_ids.len());
        for project_id in &node.attached_project_ids {
            let folder = create_managed_path(
                &self.knowledge_root,
                &[
                    "nodes",
                    &node.id,
                    "snapshots",
                    &snapshot_id,
                    "projects",
                    project_id,
                ],
            )?;
            project_folders.push(SnapshotProjectFolder {
                project_id: project_id.clone(),
                folder_path: path_string(&folder)?,
            });
        }
        let snapshot = self
            .store
            .create_coordination_snapshot(
                &node.id,
                &snapshot_id,
                path_string(&snapshot_folder)?,
                project_folders,
                command,
            )
            .await;
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                remove_empty_tree(&snapshot_folder);
                return Err(error.into());
            }
        };
        if snapshot.replayed {
            if snapshot.id != snapshot_id {
                remove_empty_tree(&snapshot_folder);
            }
            return self.refresh_snapshot(snapshot).await;
        }

        for project_collection in &snapshot.projects {
            let delivery = self
                .deliver_snapshot_prompt(&snapshot, project_collection)
                .await;
            let result = match delivery {
                Ok(runtime_status) => SnapshotDeliveryResult::Submitted { runtime_status },
                Err(error) => SnapshotDeliveryResult::Failed {
                    ambiguous: matches!(
                        error,
                        CoordinationNodeServiceError::Runtime(RuntimeInterventionError::Ambiguous(
                            _
                        ))
                    ),
                    message: error.to_string(),
                },
            };
            self.store
                .record_snapshot_project_delivery(
                    &snapshot.id,
                    &project_collection.project_id,
                    result,
                )
                .await?;
        }
        self.refresh_snapshot(
            self.store
                .get_coordination_snapshot(&node.id, &snapshot.id)
                .await?,
        )
        .await
    }

    pub(crate) async fn list_snapshots(
        &self,
        node_id: &str,
    ) -> Result<CoordinationSnapshots, CoordinationNodeServiceError> {
        let snapshots = self.store.list_coordination_snapshots(node_id).await?;
        let mut refreshed = Vec::with_capacity(snapshots.snapshots.len());
        for snapshot in snapshots.snapshots {
            refreshed.push(self.refresh_snapshot(snapshot).await?);
        }
        Ok(CoordinationSnapshots {
            snapshots: refreshed,
        })
    }

    pub(crate) async fn get_snapshot(
        &self,
        node_id: &str,
        snapshot_id: &str,
    ) -> Result<CoordinationSnapshot, CoordinationNodeServiceError> {
        let snapshot = self
            .store
            .get_coordination_snapshot(node_id, snapshot_id)
            .await?;
        self.refresh_snapshot(snapshot).await
    }

    pub(crate) async fn validate_node_binding(
        &self,
        expected: &CoordinationNode,
    ) -> Result<CoordinationNode, CoordinationNodeServiceError> {
        let current = self.store.get_coordination_node(&expected.id).await?;
        if current.version != expected.version
            || current.worker.as_ref().map(|worker| &worker.id)
                != expected.worker.as_ref().map(|worker| &worker.id)
        {
            return Err(CoordinationNodeServiceError::NodeChanged);
        }
        let expected_worker = expected
            .worker
            .as_ref()
            .ok_or(CoordinationNodeServiceError::NodeNotProvisioned)?;
        let current_worker = current
            .worker
            .as_ref()
            .ok_or(CoordinationNodeServiceError::NodeNotProvisioned)?;
        if !same_runtime_identity(expected_worker, current_worker) {
            return Err(CoordinationNodeServiceError::RuntimeBindingStale);
        }
        self.validate_worker_runtime(current_worker).await?;
        Ok(current)
    }

    async fn validate_route_bindings(
        &self,
        node: &CoordinationNode,
        expected_project: &yard_domain::Project,
    ) -> Result<(), CoordinationNodeServiceError> {
        self.validate_node_binding(node).await?;
        let current = self.store.get_project(&expected_project.id).await?;
        if current.version != expected_project.version
            || current.orchestrator.id != expected_project.orchestrator.id
            || !same_runtime_identity(&current.orchestrator, &expected_project.orchestrator)
        {
            return Err(CoordinationNodeServiceError::ProjectOrchestratorChanged);
        }
        self.validate_worker_runtime(&current.orchestrator).await
    }

    async fn validate_worker_runtime(
        &self,
        worker: &Worker,
    ) -> Result<(), CoordinationNodeServiceError> {
        let runtime = worker
            .runtime
            .as_ref()
            .ok_or(CoordinationNodeServiceError::RuntimeBindingMissing)?;
        let inventory = self.source.inventory(&runtime.session).await?;
        if inventory.adapter != runtime.adapter || inventory.session != runtime.session {
            return Err(CoordinationNodeServiceError::RuntimeBindingStale);
        }
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == runtime.terminal_id)
            .ok_or(CoordinationNodeServiceError::RuntimeBindingStale)?;
        if observed.workspace_id != runtime.workspace_id
            || observed.pane_id != runtime.pane_id
            || observed.tab_id != runtime.tab_id.as_deref().unwrap_or_default()
            || runtime
                .provider_session
                .as_ref()
                .is_some_and(|expected| observed.provider_session.as_ref() != Some(expected))
        {
            return Err(CoordinationNodeServiceError::RuntimeBindingStale);
        }
        Ok(())
    }

    async fn reconcile_provisioned_worker(
        &self,
        node: &CoordinationNode,
        terminal_id: &str,
    ) -> Result<yard_domain::ObservedWorker, CoordinationNodeServiceError> {
        let deadline = Instant::now() + RUNTIME_IDENTITY_TIMEOUT;
        loop {
            let (inventory, _) = self.reconciliation.refresh(COORDINATION_SESSION).await?;
            if let Some(worker) = dedicated_worker(&inventory, node)
                .filter(|worker| worker.terminal_id == terminal_id)
            {
                return Ok(worker);
            }
            if Instant::now() >= deadline {
                return Err(CoordinationNodeServiceError::RuntimeBindingUnverified);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn deliver_snapshot_prompt(
        &self,
        snapshot: &CoordinationSnapshot,
        collection: &yard_domain::SnapshotProjectCollection,
    ) -> Result<String, CoordinationNodeServiceError> {
        let project = self.store.get_project(&collection.project_id).await?;
        if project.version != collection.project_version
            || project.orchestrator.id != collection.orchestrator_worker_id
        {
            return Err(CoordinationNodeServiceError::ProjectOrchestratorChanged);
        }
        self.validate_worker_runtime(&project.orchestrator).await?;
        let runtime = project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(CoordinationNodeServiceError::RuntimeBindingMissing)?;
        let command_id = format!("snapshot:{}:{}", snapshot.id, project.id);
        let prompt = snapshot_prompt(
            &snapshot.id,
            &project.id,
            collection.project_version,
            &collection.folder_path,
        );
        let workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                &project.workflow_profile.profile_id,
                project.workflow_profile.profile_version,
            )
            .await?;
        let prompt = with_orchestrator_workflow(&prompt, &workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let result = self
            .interventions
            .prompt(RuntimePromptRequest {
                command_id: command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: with_orchestrator_status_contract(&prompt, &command_id),
            })
            .await?;
        let current = self
            .store
            .get_project(&collection.project_id)
            .await
            .map_err(|error| {
                CoordinationNodeServiceError::Runtime(RuntimeInterventionError::Ambiguous(format!(
                    "Herdr acknowledged the snapshot prompt, but the project orchestrator \
                         could not be revalidated: {error}"
                )))
            })?;
        if current.version != collection.project_version
            || current.orchestrator.id != collection.orchestrator_worker_id
            || !same_runtime_identity(&current.orchestrator, &project.orchestrator)
        {
            return Err(CoordinationNodeServiceError::Runtime(
                RuntimeInterventionError::Ambiguous(
                    "Herdr acknowledged the snapshot prompt, but the project orchestrator \
                     changed"
                        .to_owned(),
                ),
            ));
        }
        self.validate_worker_runtime(&current.orchestrator)
            .await
            .map_err(|error| {
                CoordinationNodeServiceError::Runtime(RuntimeInterventionError::Ambiguous(format!(
                    "Herdr acknowledged the snapshot prompt, but the project orchestrator \
                     binding changed: {error}"
                )))
            })?;
        Ok(result.status)
    }

    async fn refresh_snapshot(
        &self,
        snapshot: CoordinationSnapshot,
    ) -> Result<CoordinationSnapshot, CoordinationNodeServiceError> {
        for project in &snapshot.projects {
            if project.collection_status == SnapshotCollectionStatus::Pending
                && snapshot_project_is_collected(Path::new(&project.folder_path))?
            {
                self.store
                    .record_snapshot_project_collected(&snapshot.id, &project.project_id)
                    .await?;
            }
        }
        self.store
            .get_coordination_snapshot(&snapshot.node_id, &snapshot.id)
            .await
            .map_err(Into::into)
    }
}

fn dedicated_worker(
    inventory: &yard_domain::RuntimeInventory,
    node: &CoordinationNode,
) -> Option<yard_domain::ObservedWorker> {
    let workspaces = inventory
        .workspaces
        .iter()
        .filter(|workspace| workspace.label == workspace_label(&node.id))
        .map(|workspace| workspace.runtime_id.as_str())
        .collect::<HashSet<_>>();
    let current_terminal = node
        .worker
        .as_ref()
        .and_then(|worker| worker.runtime.as_ref())
        .map(|runtime| runtime.terminal_id.as_str());
    let mut workers = inventory
        .workers
        .iter()
        .filter(|worker| {
            worker.name.as_deref() == Some(agent_name(&node.id).as_str())
                && workspaces.contains(worker.workspace_id.as_str())
                && worker.interactive_ready
                && worker.provider_session.is_some()
        })
        .cloned()
        .collect::<Vec<_>>();
    workers.sort_by_key(|worker| {
        (
            current_terminal != Some(worker.terminal_id.as_str()),
            worker.workspace_id.clone(),
            worker.terminal_id.clone(),
        )
    });
    workers.into_iter().next()
}

fn workspace_label(node_id: &str) -> String {
    format!("Yard coordination {node_id}")
}

fn agent_name(node_id: &str) -> String {
    format!("coordination-{node_id}")
}

fn node_runtime(
    node: &CoordinationNode,
) -> Result<&yard_domain::WorkerRuntimeBinding, CoordinationNodeServiceError> {
    if node.kind != CoordinationNodeKind::Workstream {
        return Err(CoordinationNodeServiceError::WrongNodeKind);
    }
    node.worker
        .as_ref()
        .ok_or(CoordinationNodeServiceError::NodeNotProvisioned)?
        .runtime
        .as_ref()
        .ok_or(CoordinationNodeServiceError::RuntimeBindingMissing)
}

fn same_runtime_identity(left: &Worker, right: &Worker) -> bool {
    match (left.runtime.as_ref(), right.runtime.as_ref()) {
        (Some(left), Some(right)) => {
            left.adapter == right.adapter
                && left.session == right.session
                && left.workspace_id == right.workspace_id
                && left.terminal_id == right.terminal_id
                && left.tab_id == right.tab_id
                && left.pane_id == right.pane_id
                && left.provider_session == right.provider_session
        }
        (None, None) => true,
        _ => false,
    }
}

fn validate_output_identity(
    runtime: &yard_domain::WorkerRuntimeBinding,
    result: &crate::intervention_service::RuntimeOutputResult,
) -> Result<(), CoordinationNodeServiceError> {
    if result.pane_id != runtime.pane_id
        || result.workspace_id != runtime.workspace_id
        || result.tab_id != runtime.tab_id.as_deref().unwrap_or_default()
    {
        Err(CoordinationNodeServiceError::RuntimeBindingStale)
    } else {
        Ok(())
    }
}

fn snapshot_prompt(
    snapshot_id: &str,
    project_id: &str,
    project_version: u64,
    folder_path: &str,
) -> String {
    format!(
        "Yard knowledge snapshot {snapshot_id} captured project {project_id} at durable revision \
         {project_version}. Write the following UTF-8 Markdown files directly into the exact \
         server-managed folder `{folder_path}`: overview.md, objectives.md, changes.md, \
         decisions.md, interfaces.md, risks.md, next-actions.md, and sources.md. Include concrete \
         source references in sources.md and cross-reference them from factual claims. Do not \
         write outside that folder, follow symlinks, mutate an earlier snapshot, claim project \
         completion, or create a Yard completion receipt. This request collects project knowledge \
         only."
    )
}

fn snapshot_project_is_collected(path: &Path) -> Result<bool, CoordinationNodeServiceError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CoordinationNodeServiceError::UnsafeManagedPath);
    }
    for file in REQUIRED_SNAPSHOT_FILES {
        let candidate = path.join(file);
        let Ok(metadata) = fs::symlink_metadata(candidate) else {
            return Ok(false);
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CoordinationNodeServiceError::UnsafeManagedPath);
        }
    }
    Ok(true)
}

fn validate_recorded_managed_path(
    root: &Path,
    node_id: &str,
    recorded: &str,
) -> Result<(), CoordinationNodeServiceError> {
    let expected = create_managed_path(root, &["nodes", node_id])?;
    let recorded = fs::canonicalize(recorded)?;
    if expected != recorded {
        return Err(CoordinationNodeServiceError::UnsafeManagedPath);
    }
    Ok(())
}

fn create_managed_path(
    root: &Path,
    components: &[&str],
) -> Result<PathBuf, CoordinationNodeServiceError> {
    let root = ensure_managed_root(root)?;
    let mut current = root.clone();
    for component in components {
        validate_path_component(component)?;
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(CoordinationNodeServiceError::UnsafeManagedPath);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let canonical = fs::canonicalize(&current)?;
    if !canonical.starts_with(&root) {
        return Err(CoordinationNodeServiceError::UnsafeManagedPath);
    }
    Ok(canonical)
}

fn ensure_managed_root(root: &Path) -> Result<PathBuf, CoordinationNodeServiceError> {
    if !root.is_absolute() {
        return Err(CoordinationNodeServiceError::ManagedRootNotAbsolute);
    }
    let root = normalize_managed_root(root)?;
    fs::create_dir_all(&root)?;
    let metadata = fs::symlink_metadata(&root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CoordinationNodeServiceError::UnsafeManagedPath);
    }
    // The configured root may pass through a system-managed alias such as
    // /home/user -> /local/home/user. Descendants are created from the
    // canonical root and checked individually by create_managed_path.
    fs::canonicalize(root).map_err(Into::into)
}

fn normalize_managed_root(root: &Path) -> Result<PathBuf, CoordinationNodeServiceError> {
    let mut normalized = PathBuf::new();
    for component in root.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(CoordinationNodeServiceError::UnsafeManagedPath);
            }
        }
    }
    Ok(normalized)
}

fn validate_path_component(component: &str) -> Result<(), CoordinationNodeServiceError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains('/')
        || component.contains('\\')
    {
        return Err(CoordinationNodeServiceError::UnsafeManagedPath);
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<String, CoordinationNodeServiceError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(CoordinationNodeServiceError::ManagedPathNotUtf8)
}

fn remove_empty_managed_path(path: &Path) {
    let _ = fs::remove_dir(path);
}

fn remove_empty_tree(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn runtime_error(error: RuntimeProvisionError) -> CoordinationNodeServiceError {
    match error {
        RuntimeProvisionError::PromptDelivery { message, .. } => {
            CoordinationNodeServiceError::ObjectiveDeliveryFailed(message)
        }
        RuntimeProvisionError::BeforeWorker(message)
        | RuntimeProvisionError::AfterPreparation { message, .. } => {
            CoordinationNodeServiceError::RuntimeProvision(message)
        }
    }
}

fn provisioning_failure(
    error: RuntimeProvisionError,
) -> (String, bool, Option<WorkerRuntimeBinding>, bool) {
    match error {
        RuntimeProvisionError::BeforeWorker(message) => (message, false, None, false),
        RuntimeProvisionError::PromptDelivery { runtime, message } => {
            (message, true, Some(*runtime), true)
        }
        RuntimeProvisionError::AfterPreparation {
            message,
            ambiguous,
            started_runtime,
        } => {
            let runtime = started_runtime.map(|runtime| *runtime);
            (message, ambiguous || runtime.is_some(), runtime, false)
        }
    }
}

fn coordination_provision_error(
    message: String,
    ambiguous: bool,
    objective_delivery: bool,
) -> CoordinationNodeServiceError {
    if objective_delivery {
        CoordinationNodeServiceError::ObjectiveDeliveryFailed(message)
    } else if ambiguous {
        CoordinationNodeServiceError::RuntimeProvisionAmbiguous(message)
    } else {
        CoordinationNodeServiceError::RuntimeProvision(message)
    }
}

#[derive(Debug, Error)]
pub enum CoordinationNodeServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::CoordinationNodeValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Reconciliation(#[from] ReconciliationServiceError),
    #[error(transparent)]
    Runtime(#[from] RuntimeInterventionError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("this operation is not supported by the coordination node kind")]
    WrongNodeKind,
    #[error("the workstream node has not been provisioned")]
    NodeNotProvisioned,
    #[error("the coordination node changed while the operation was in progress")]
    NodeChanged,
    #[error("the target project orchestrator changed while the operation was in progress")]
    ProjectOrchestratorChanged,
    #[error("worker has no runtime binding")]
    RuntimeBindingMissing,
    #[error("the Herdr runtime no longer matches the durable worker binding")]
    RuntimeBindingStale,
    #[error("scheduled automatic summaries are disabled")]
    AutomaticTokenSpendDisabled,
    #[error("lines must be between 1 and 1000")]
    InvalidLineCount,
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("Herdr coordination-node provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("Herdr coordination-node provisioning outcome is ambiguous: {0}")]
    RuntimeProvisionAmbiguous(String),
    #[error("the provisioned Herdr worker did not appear in a fresh inventory")]
    RuntimeBindingUnverified,
    #[error("the reconciled Herdr worker is missing from Yard's durable worker inventory")]
    ReconciledWorkerMissing,
    #[error("the worker was created but the coordination objective prompt failed: {0}")]
    ObjectiveDeliveryFailed(String),
    #[error("the server-managed node path is missing")]
    ManagedPathMissing,
    #[error("managed roots must be absolute paths")]
    ManagedRootNotAbsolute,
    #[error("the server-managed path is not valid UTF-8")]
    ManagedPathNotUtf8,
    #[error("the server-managed path is unsafe or contains a symlink")]
    UnsafeManagedPath,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
    };

    use async_trait::async_trait;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, CoordinationDeliveryStatus, CoordinationNodeKind, CreateCoordinationNode,
        CreateProject, CreateWorkerProfile, FocusObservation, ObservedStatus, ObservedWorker,
        ProjectRuntimeBinding, ProviderSessionRef, ProvisionCoordinationNode,
        RequestCoordinationSnapshot, RuntimeInventory, RuntimeObservationState,
        RuntimeProcessState, RuntimeSession, RuntimeSessions, SnapshotCollectionStatus,
        TransferProjectOrchestrator, WorkerProfileSpec, WorkerRuntimeBinding, WorkspaceObservation,
    };
    use yard_store::{SqliteProjectStore, YardStore};

    use crate::{
        allocation_service::{
            RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeSessionRequest,
            RuntimeWorkspaceProvisionRequest,
        },
        intervention_service::{
            RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest,
            RuntimeOutputResult, RuntimePromptRequest, RuntimePromptResult,
        },
        inventory_service::{InventoryServiceError, InventorySource},
        reconciliation_service::ReconciliationService,
    };

    use super::{
        COORDINATION_SESSION, CoordinationNodeService, CoordinationNodeServiceError,
        REQUIRED_SNAPSHOT_FILES, create_managed_path,
    };

    #[derive(Default)]
    struct FakeRuntime {
        live: AtomicBool,
        observed_at: AtomicU64,
        session_requests: Mutex<Vec<RuntimeSessionRequest>>,
        bootstrap: Mutex<Option<RuntimeWorkspaceProvisionRequest>>,
        provision_failure: Mutex<Option<RuntimeProvisionError>>,
        prompts: Mutex<Vec<RuntimePromptRequest>>,
        project_mutation: Mutex<Option<(PathBuf, String, String)>>,
        project_read_failure: Mutex<Option<PathBuf>>,
        prompt_gate: Mutex<Option<PromptGate>>,
    }

    #[derive(Clone)]
    struct PromptGate {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl FakeRuntime {
        fn coordination_binding() -> WorkerRuntimeBinding {
            binding(
                COORDINATION_SESSION,
                "coordination-workspace",
                "coordination-terminal",
                "coordination-tab",
                "coordination-pane",
                "coordination-provider-session",
            )
        }
    }

    #[async_trait]
    impl InventorySource for FakeRuntime {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![
                    RuntimeSession {
                        name: "default".to_owned(),
                        is_default: true,
                        running: true,
                    },
                    RuntimeSession {
                        name: COORDINATION_SESSION.to_owned(),
                        is_default: false,
                        running: true,
                    },
                ],
            })
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let observed_at_unix_ms = self.observed_at.fetch_add(1, Ordering::SeqCst) + 1;
            if session_name == "default" {
                return Ok(inventory(
                    "default",
                    observed_at_unix_ms,
                    vec![workspace("project-workspace", "Project")],
                    vec![observed_worker(
                        "project-terminal",
                        "project-workspace",
                        "project-tab",
                        "project-pane",
                        "project-orchestrator",
                        "project-provider-session",
                    )],
                ));
            }
            assert_eq!(session_name, COORDINATION_SESSION);
            let request = self.bootstrap.lock().unwrap().clone();
            let live = self.live.load(Ordering::SeqCst);
            Ok(inventory(
                COORDINATION_SESSION,
                observed_at_unix_ms,
                request
                    .as_ref()
                    .filter(|_| live)
                    .map(|request| {
                        vec![workspace(
                            "coordination-workspace",
                            &request.workspace_label,
                        )]
                    })
                    .unwrap_or_default(),
                request
                    .as_ref()
                    .filter(|_| live)
                    .map(|request| {
                        vec![observed_worker(
                            "coordination-terminal",
                            "coordination-workspace",
                            "coordination-tab",
                            "coordination-pane",
                            &request.agent_name,
                            "coordination-provider-session",
                        )]
                    })
                    .unwrap_or_default(),
            ))
        }
    }

    #[async_trait]
    impl RuntimeControl for FakeRuntime {
        async fn ensure_session(
            &self,
            request: RuntimeSessionRequest,
        ) -> Result<(), RuntimeProvisionError> {
            self.session_requests.lock().unwrap().push(request);
            Ok(())
        }

        async fn bootstrap_worker(
            &self,
            request: RuntimeWorkspaceProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            *self.bootstrap.lock().unwrap() = Some(request);
            if let Some(error) = self.provision_failure.lock().unwrap().take() {
                return Err(error);
            }
            self.live.store(true, Ordering::SeqCst);
            Ok(Self::coordination_binding())
        }

        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            unreachable!("coordination nodes bootstrap their own workspace")
        }
    }

    #[async_trait]
    impl RuntimeIntervention for FakeRuntime {
        async fn prompt(
            &self,
            request: RuntimePromptRequest,
        ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
            self.prompts.lock().unwrap().push(request);
            let prompt_gate = self.prompt_gate.lock().unwrap().clone();
            if let Some(prompt_gate) = prompt_gate {
                prompt_gate.entered.notify_one();
                prompt_gate.release.notified().await;
            }
            if let Some((database_path, project_id, worker_id)) =
                self.project_mutation.lock().unwrap().take()
            {
                let connection = rusqlite::Connection::open(database_path).unwrap();
                connection
                    .execute(
                        "UPDATE projects
                            SET orchestrator_worker_id = ?1,
                                version = version + 1,
                                updated_at_unix_ms = updated_at_unix_ms + 1
                          WHERE id = ?2",
                        rusqlite::params![worker_id, project_id],
                    )
                    .unwrap();
            }
            if let Some(database_path) = self.project_read_failure.lock().unwrap().take() {
                let connection = rusqlite::Connection::open(database_path).unwrap();
                connection
                    .execute_batch("ALTER TABLE projects RENAME TO unavailable_projects;")
                    .unwrap();
            }
            Ok(RuntimePromptResult {
                status: "accepted".to_owned(),
            })
        }

        async fn read_output(
            &self,
            request: RuntimeOutputRequest,
        ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
            Ok(RuntimeOutputResult {
                pane_id: request.pane_id,
                workspace_id: "coordination-workspace".to_owned(),
                tab_id: "coordination-tab".to_owned(),
                source: "screen".to_owned(),
                format: "text".to_owned(),
                text: "idle".to_owned(),
                revision: 1,
                truncated: false,
            })
        }
    }

    async fn setup() -> (
        CoordinationNodeService,
        Arc<SqliteProjectStore>,
        Arc<FakeRuntime>,
        TempDir,
    ) {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let runtime = Arc::new(FakeRuntime::default());
        let source: Arc<dyn InventorySource> = runtime.clone();
        let control: Arc<dyn RuntimeControl> = runtime.clone();
        let interventions: Arc<dyn RuntimeIntervention> = runtime.clone();
        let reconciliation = ReconciliationService::new(source.clone(), store.clone());
        (
            CoordinationNodeService::new(
                source,
                control,
                interventions,
                store.clone(),
                reconciliation,
                temp.path().join("coordination"),
                temp.path().join("knowledge"),
            ),
            store,
            runtime,
            temp,
        )
    }

    #[tokio::test]
    async fn provisions_shared_session_worker_with_managed_cwd_and_status_contract() {
        let (service, store, runtime, temp) = setup().await;
        let created = service
            .create(create_command(
                CoordinationNodeKind::Workstream,
                Vec::new(),
                "create-workstream",
            ))
            .await
            .unwrap();
        let profile = store.create_worker_profile(profile()).await.unwrap();
        let provision_command = ProvisionCoordinationNode {
            command_id: "provision-workstream".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_node_version: created.node.version,
        };
        let provisioned = service
            .provision(&created.node.id, provision_command.clone())
            .await
            .unwrap();
        let replayed = service
            .provision(&created.node.id, provision_command)
            .await
            .unwrap();

        assert!(provisioned.node.worker.is_some());
        assert!(replayed.replayed);
        let cwd = created.node.cwd.unwrap();
        assert!(Path::new(&cwd).starts_with(temp.path().join("coordination")));
        assert!(Path::new(&cwd).is_dir());
        let sessions = runtime.session_requests.lock().unwrap();
        assert_eq!(sessions[0].session, COORDINATION_SESSION);
        let bootstrap = runtime.bootstrap.lock().unwrap();
        let request = bootstrap.as_ref().unwrap();
        assert_eq!(request.session, COORDINATION_SESSION);
        assert_eq!(request.cwd, cwd);
        assert!(request.workspace_label.contains(&created.node.id));
        assert!(request.agent_name.contains(&created.node.id));
        assert!(request.prompt.contains("There is no completed state"));
        assert!(request.prompt.contains("command \"provision-workstream\""));
        assert!(
            yard_domain::OrchestratorStatusReport::scan_terminal_output(&request.prompt).is_none()
        );
    }

    #[tokio::test]
    async fn prompt_delivery_runtime_is_quarantined_for_coordination_bootstrap() {
        let (service, store, runtime, temp) = setup().await;
        let created = service
            .create(create_command(
                CoordinationNodeKind::Workstream,
                Vec::new(),
                "create-ambiguous-workstream",
            ))
            .await
            .unwrap();
        let profile = store.create_worker_profile(profile()).await.unwrap();
        *runtime.provision_failure.lock().unwrap() = Some(RuntimeProvisionError::PromptDelivery {
            runtime: Box::new(FakeRuntime::coordination_binding()),
            message: "initial coordination prompt acknowledgement was lost".to_owned(),
        });
        let command = ProvisionCoordinationNode {
            command_id: "provision-ambiguous-workstream".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_node_version: created.node.version,
        };

        let error = service
            .provision(&created.node.id, command.clone())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CoordinationNodeServiceError::ObjectiveDeliveryFailed(_)
        ));
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let captured: (String, String, Option<String>) = connection
            .query_row(
                "SELECT command.status, quarantine.terminal_id,
                        quarantine.provider_session_value
                   FROM command_acknowledgements command
                   JOIN quarantined_provisioning_runtime_bindings quarantine
                     ON quarantine.command_id = command.id
                  WHERE command.id = ?1",
                [&command.command_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            captured,
            (
                "ambiguous".to_owned(),
                "coordination-terminal".to_owned(),
                Some("coordination-provider-session".to_owned())
            )
        );
    }

    #[tokio::test]
    async fn snapshot_prompts_exact_managed_path_and_never_create_receipts() {
        let (service, store, runtime, temp) = setup().await;
        let project = store
            .create_project(
                CreateProject {
                    name: "Project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "project-workspace".to_owned(),
                    },
                    orchestrator_observed_worker_id: "project-terminal".to_owned(),
                    placement: placement(),
                },
                binding(
                    "default",
                    "project-workspace",
                    "project-terminal",
                    "project-tab",
                    "project-pane",
                    "project-provider-session",
                ),
            )
            .await
            .unwrap();
        let node = service
            .create(create_command(
                CoordinationNodeKind::KnowledgeStore,
                vec![project.id.clone()],
                "create-knowledge",
            ))
            .await
            .unwrap()
            .node;
        let snapshot = service
            .request_snapshot(
                &node.id,
                RequestCoordinationSnapshot {
                    command_id: "request-snapshot".to_owned(),
                    actor: "local-user".to_owned(),
                    expected_node_version: node.version,
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot.progress.completed, 0);
        assert_eq!(snapshot.progress.total, 1);
        let project_folder = PathBuf::from(&snapshot.projects[0].folder_path);
        assert!(project_folder.starts_with(temp.path().join("knowledge")));
        {
            let prompts = runtime.prompts.lock().unwrap();
            assert_eq!(prompts.len(), 1);
            assert!(prompts[0].text.contains(project_folder.to_str().unwrap()));
            for file in REQUIRED_SNAPSHOT_FILES {
                assert!(prompts[0].text.contains(file));
            }
            assert!(prompts[0].text.contains("source references"));
            assert!(prompts[0].text.contains("Do not"));
            assert!(
                prompts[0]
                    .text
                    .contains("Yard orchestrator workflow profile yard:standard-orchestrator")
            );
            assert!(prompts[0].text.contains("Allocate independent workers"));
            assert!(prompts[0].text.contains("There is no completed state"));
        }

        for file in REQUIRED_SNAPSHOT_FILES {
            fs::write(project_folder.join(file), format!("# {file}\n")).unwrap();
        }
        let collected = service.get_snapshot(&node.id, &snapshot.id).await.unwrap();
        assert_eq!(collected.progress.completed, 1);
        assert_eq!(
            collected.projects[0].collection_status,
            SnapshotCollectionStatus::Collected
        );
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
    async fn snapshot_delivery_blocks_concurrent_orchestrator_transfer() {
        let (service, store, runtime, _temp) = setup().await;
        let project = store
            .create_project(
                CreateProject {
                    name: "Project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "project-workspace".to_owned(),
                    },
                    orchestrator_observed_worker_id: "project-terminal".to_owned(),
                    placement: placement(),
                },
                binding(
                    "default",
                    "project-workspace",
                    "project-terminal",
                    "project-tab",
                    "project-pane",
                    "project-provider-session",
                ),
            )
            .await
            .unwrap();
        store
            .reconcile_runtime_inventory(inventory(
                "default",
                20,
                vec![workspace("project-workspace", "Project")],
                vec![
                    observed_worker(
                        "project-terminal",
                        "project-workspace",
                        "project-tab",
                        "project-pane",
                        "project-orchestrator",
                        "project-provider-session",
                    ),
                    observed_worker(
                        "transfer-terminal",
                        "project-workspace",
                        "transfer-tab",
                        "transfer-pane",
                        "transfer-candidate",
                        "transfer-provider-session",
                    ),
                ],
            ))
            .await
            .unwrap();
        let current_project = store.get_project(&project.id).await.unwrap();
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
                    .is_some_and(|binding| binding.terminal_id == "transfer-terminal")
            })
            .unwrap();
        let transfer = TransferProjectOrchestrator {
            command_id: "transfer-during-snapshot-delivery".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id.clone(),
            expected_worker_version: candidate.worker.version,
            expected_worker_runtime: candidate.worker.runtime.clone().unwrap(),
            expected_project_version: current_project.version,
            expected_orchestrator_worker_id: current_project.orchestrator.id.clone(),
            expected_orchestrator_worker_version: current_project.orchestrator.version,
            expected_orchestrator_runtime: current_project.orchestrator.runtime.clone().unwrap(),
        };
        let node = service
            .create(create_command(
                CoordinationNodeKind::KnowledgeStore,
                vec![project.id.clone()],
                "create-transfer-racing-knowledge",
            ))
            .await
            .unwrap()
            .node;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *runtime.prompt_gate.lock().unwrap() = Some(PromptGate {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let service = Arc::new(service);
        let request_service = Arc::clone(&service);
        let node_id = node.id.clone();
        let snapshot_task = tokio::spawn(async move {
            request_service
                .request_snapshot(
                    &node_id,
                    RequestCoordinationSnapshot {
                        command_id: "request-transfer-racing-snapshot".to_owned(),
                        actor: "local-user".to_owned(),
                        expected_node_version: node.version,
                    },
                )
                .await
        });
        entered.notified().await;

        assert!(matches!(
            store
                .transfer_project_orchestrator(&project.id, transfer.clone())
                .await
                .unwrap_err(),
            yard_store::ProjectStoreError::OrchestratorInterventionInProgress
        ));
        release.notify_one();
        let snapshot = snapshot_task.await.unwrap().unwrap();
        assert_eq!(
            snapshot.projects[0].delivery_status,
            CoordinationDeliveryStatus::Submitted
        );

        let transferred = store
            .transfer_project_orchestrator(&project.id, transfer)
            .await
            .unwrap();
        assert_eq!(transferred.project.orchestrator.id, candidate.worker.id);
    }

    #[tokio::test]
    async fn snapshot_acknowledgement_is_ambiguous_if_orchestrator_changes_after_send() {
        let (service, store, runtime, temp) = setup().await;
        let project = store
            .create_project(
                CreateProject {
                    name: "Project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "project-workspace".to_owned(),
                    },
                    orchestrator_observed_worker_id: "project-terminal".to_owned(),
                    placement: placement(),
                },
                binding(
                    "default",
                    "project-workspace",
                    "project-terminal",
                    "project-tab",
                    "project-pane",
                    "project-provider-session",
                ),
            )
            .await
            .unwrap();
        let node = service
            .create(create_command(
                CoordinationNodeKind::KnowledgeStore,
                vec![project.id.clone()],
                "create-racing-knowledge",
            ))
            .await
            .unwrap()
            .node;
        let replacement_worker_id = uuid::Uuid::now_v7().to_string();
        let database_path = temp.path().join("yard.sqlite3");
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute(
                "INSERT INTO workers (
                    id, profile_id, profile_version, desired_state, version,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, NULL, NULL, 'running', 1, 1, 1)",
                [&replacement_worker_id],
            )
            .unwrap();
        drop(connection);
        *runtime.project_mutation.lock().unwrap() = Some((
            database_path,
            project.id.clone(),
            replacement_worker_id.clone(),
        ));

        let snapshot = service
            .request_snapshot(
                &node.id,
                RequestCoordinationSnapshot {
                    command_id: "request-racing-snapshot".to_owned(),
                    actor: "local-user".to_owned(),
                    expected_node_version: node.version,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            snapshot.projects[0].delivery_status,
            CoordinationDeliveryStatus::Ambiguous
        );
        assert_eq!(
            store
                .get_project(&project.id)
                .await
                .unwrap()
                .orchestrator
                .id,
            replacement_worker_id
        );
    }

    #[tokio::test]
    async fn snapshot_acknowledgement_is_ambiguous_if_post_send_owner_read_fails() {
        let (service, store, runtime, temp) = setup().await;
        let project = store
            .create_project(
                CreateProject {
                    name: "Project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "project-workspace".to_owned(),
                    },
                    orchestrator_observed_worker_id: "project-terminal".to_owned(),
                    placement: placement(),
                },
                binding(
                    "default",
                    "project-workspace",
                    "project-terminal",
                    "project-tab",
                    "project-pane",
                    "project-provider-session",
                ),
            )
            .await
            .unwrap();
        let node = service
            .create(create_command(
                CoordinationNodeKind::KnowledgeStore,
                vec![project.id],
                "create-read-failure-knowledge",
            ))
            .await
            .unwrap()
            .node;
        *runtime.project_read_failure.lock().unwrap() = Some(temp.path().join("yard.sqlite3"));

        let snapshot = service
            .request_snapshot(
                &node.id,
                RequestCoordinationSnapshot {
                    command_id: "request-read-failure-snapshot".to_owned(),
                    actor: "local-user".to_owned(),
                    expected_node_version: node.version,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            snapshot.projects[0].delivery_status,
            CoordinationDeliveryStatus::Ambiguous
        );
    }

    #[cfg(unix)]
    #[test]
    fn allows_symlinked_ancestor_outside_managed_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let physical_home = temp.path().join("physical-home");
        let home_alias = temp.path().join("home-alias");
        fs::create_dir_all(&physical_home).unwrap();
        symlink(&physical_home, &home_alias).unwrap();

        let created = create_managed_path(
            &home_alias.join("knowledge"),
            &["nodes", "0198a81c-3773-7c60-b7d2-ff795ad88ad1"],
        )
        .unwrap();

        assert_eq!(
            created,
            physical_home.join("knowledge/nodes/0198a81c-3773-7c60-b7d2-ff795ad88ad1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_managed_root_symlinks_with_path_suffixes() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let outside = temp.path().join("outside");
        let root_link = temp.path().join("knowledge-link");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &root_link).unwrap();

        for root in [
            root_link.clone(),
            PathBuf::from(format!("{}/", root_link.display())),
            root_link.join("."),
        ] {
            let error =
                create_managed_path(&root, &["nodes", "0198a81c-3773-7c60-b7d2-ff795ad88ad1"])
                    .unwrap_err();
            assert!(matches!(
                error,
                CoordinationNodeServiceError::UnsafeManagedPath
            ));
        }
        assert!(!outside.join("nodes").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_components_in_managed_paths() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("knowledge");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("nodes")).unwrap();

        let error = create_managed_path(&root, &["nodes", "0198a81c-3773-7c60-b7d2-ff795ad88ad1"])
            .unwrap_err();
        assert!(matches!(
            error,
            CoordinationNodeServiceError::UnsafeManagedPath
        ));
    }

    fn create_command(
        kind: CoordinationNodeKind,
        attached_project_ids: Vec<String>,
        command_id: &str,
    ) -> CreateCoordinationNode {
        CreateCoordinationNode {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            name: "Release coordination".to_owned(),
            kind,
            placement: placement(),
            attached_project_ids,
        }
    }

    fn placement() -> CanvasPlacement {
        CanvasPlacement {
            x: 20.0,
            y: 30.0,
            width: 360.0,
            height: 220.0,
        }
    }

    fn profile() -> CreateWorkerProfile {
        CreateWorkerProfile {
            spec: WorkerProfileSpec {
                name: "Coordination worker".to_owned(),
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
                permission_policy: "yolo".to_owned(),
                completion_contract: "manual_receipt".to_owned(),
            },
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

    fn binding(
        session: &str,
        workspace_id: &str,
        terminal_id: &str,
        tab_id: &str,
        pane_id: &str,
        provider: &str,
    ) -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: session.to_owned(),
            workspace_id: workspace_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            tab_id: Some(tab_id.to_owned()),
            pane_id: pane_id.to_owned(),
            provider_session: Some(provider_session(provider)),
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Idle,
            state_change_sequence: 1,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: 1,
        }
    }

    fn workspace(id: &str, label: &str) -> WorkspaceObservation {
        WorkspaceObservation {
            runtime_id: id.to_owned(),
            order: 1,
            label: label.to_owned(),
            focused: false,
            active_tab_id: String::new(),
            pane_count: 1,
            tab_count: 1,
            status: ObservedStatus::Idle,
            tokens: BTreeMap::new(),
            worktree: None,
        }
    }

    fn observed_worker(
        terminal_id: &str,
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        name: &str,
        provider: &str,
    ) -> ObservedWorker {
        ObservedWorker {
            runtime_id: terminal_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            tab_id: tab_id.to_owned(),
            pane_id: pane_id.to_owned(),
            name: Some(name.to_owned()),
            provider: Some("codex".to_owned()),
            display_provider: Some("Codex".to_owned()),
            status: ObservedStatus::Idle,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 1,
            cwd: None,
            foreground_cwd: None,
            tokens: BTreeMap::new(),
            provider_session: Some(provider_session(provider)),
            revision: 1,
        }
    }

    fn inventory(
        session: &str,
        observed_at_unix_ms: u64,
        workspaces: Vec<WorkspaceObservation>,
        workers: Vec<ObservedWorker>,
    ) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: session.to_owned(),
            runtime_version: "test".to_owned(),
            protocol: 19,
            observed_at_unix_ms,
            focus: FocusObservation::default(),
            workspaces,
            tabs: Vec::new(),
            panes: Vec::new(),
            workers,
            child_agents: Vec::new(),
        }
    }
}

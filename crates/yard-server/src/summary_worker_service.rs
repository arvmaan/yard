use std::sync::Arc;

use thiserror::Error;
use yard_domain::{
    ConfirmProfileAllocation, IsolationPolicy, Project, ReceiveSummaryWorker,
    ReceivedSummaryWorker, RequestSummaryWorker, RuntimeInventory, RuntimeObservationState,
    RuntimeProcessState, SendAssignmentPrompt, SummaryParentRuntimeCapture, SummaryWorker,
    SummaryWorkers,
};
use yard_store::{BeginSummaryWorker, ProjectStoreError, YardStore};

use crate::{
    allocation_service::{AllocationService, AllocationServiceError},
    artifact_service::{ArtifactService, ArtifactServiceError},
    intervention_service::{InterventionService, InterventionServiceError},
    inventory_service::{InventoryServiceError, InventorySource},
    worker_cleanup_service::WorkerCleanupService,
};

#[derive(Clone)]
pub struct SummaryWorkerService {
    source: Arc<dyn InventorySource>,
    store: Arc<dyn YardStore>,
    allocations: AllocationService,
    interventions: InterventionService,
    artifacts: ArtifactService,
    cleanup: WorkerCleanupService,
}

impl SummaryWorkerService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        store: Arc<dyn YardStore>,
        allocations: AllocationService,
        interventions: InterventionService,
        artifacts: ArtifactService,
        cleanup: WorkerCleanupService,
    ) -> Self {
        Self {
            source,
            store,
            allocations,
            interventions,
            artifacts,
            cleanup,
        }
    }

    /// Start one explicitly requested summary worker.
    ///
    /// # Errors
    ///
    /// Returns an error when parent observation, allocation, contract delivery,
    /// or durable persistence fails.
    pub async fn request(
        &self,
        project_id: &str,
        command: RequestSummaryWorker,
    ) -> Result<SummaryWorker, SummaryWorkerServiceError> {
        let command = command.normalize()?;
        let capture = self.capture_parent(project_id, &command).await?;
        match self
            .store
            .begin_summary_worker(project_id, command.clone(), capture)
            .await?
        {
            BeginSummaryWorker::Replayed(summary) => return Ok(*summary),
            BeginSummaryWorker::Started => {}
        }
        let allocation = self
            .allocations
            .confirm(
                project_id,
                ConfirmProfileAllocation {
                    command_id: command.command_id.clone(),
                    actor: command.actor.clone(),
                    profile_id: command.profile_id.clone(),
                    expected_profile_version: command.expected_profile_version,
                    expected_project_version: command.expected_project_version,
                    objective: command.objective.clone(),
                    role: "summary_worker".to_owned(),
                    isolation_policy: IsolationPolicy::ProjectWorkspace,
                },
            )
            .await;
        let allocation = match allocation {
            Ok(allocation) => allocation,
            Err(error) => {
                self.store
                    .fail_summary_worker(&command.command_id, &error.to_string())
                    .await?;
                return Err(error.into());
            }
        };
        let summary = self
            .store
            .complete_summary_worker(&command.command_id, allocation)
            .await?;
        let assignment = summary
            .assignment
            .as_ref()
            .ok_or(ProjectStoreError::SummaryWorkerAllocationMismatch)?;
        let prompt = SendAssignmentPrompt {
            command_id: derived_command_id("summary-contract", &command.command_id),
            actor: command.actor,
            attempt_id: assignment.attempt.id.clone(),
            expected_assignment_version: assignment.version,
            expected_attempt_version: assignment.attempt.version,
            text: contract_prompt(&summary),
        };
        if let Err(error) = self
            .interventions
            .prompt(project_id, &assignment.id, prompt)
            .await
        {
            self.store
                .fail_summary_worker(&command.command_id, &error.to_string())
                .await?;
            return Err(error.into());
        }
        self.store
            .get_summary_worker(project_id, &summary.parent_worker_id, &assignment.id)
            .await
            .map_err(Into::into)
    }

    /// List durable summary-worker lifecycle records for one parent.
    ///
    /// # Errors
    ///
    /// Returns an error when durable state cannot be read.
    pub async fn list(
        &self,
        project_id: &str,
        parent_worker_id: &str,
    ) -> Result<SummaryWorkers, SummaryWorkerServiceError> {
        self.store
            .list_summary_workers(project_id, parent_worker_id)
            .await
            .map_err(Into::into)
    }

    /// Retrieve the committed summary artifact, then request guarded retirement.
    ///
    /// # Errors
    ///
    /// Returns an error when the artifact contract is incomplete, content
    /// verification fails, or cleanup cannot be durably scheduled.
    pub async fn receive(
        &self,
        project_id: &str,
        parent_worker_id: &str,
        assignment_id: &str,
        command: ReceiveSummaryWorker,
    ) -> Result<ReceivedSummaryWorker, SummaryWorkerServiceError> {
        let summary = self
            .store
            .get_summary_worker(project_id, parent_worker_id, assignment_id)
            .await?;
        let artifact = summary
            .artifact
            .as_ref()
            .ok_or(ProjectStoreError::SummaryWorkerHandoffNotReady)?;
        let content = self
            .artifacts
            .content(project_id, assignment_id, &artifact.id)
            .await?;
        let pane_instance_id = self.observed_child_instance(&summary).await;
        let prepared = self
            .store
            .start_summary_worker_handoff(
                project_id,
                parent_worker_id,
                assignment_id,
                command,
                pane_instance_id,
            )
            .await?;
        let cleanup_run_id = prepared
            .summary
            .cleanup_run_id
            .as_deref()
            .ok_or(ProjectStoreError::SummaryWorkerHandoffNotReady)?;
        self.cleanup.process_run(cleanup_run_id).await?;
        let summary = self
            .store
            .get_summary_worker(project_id, parent_worker_id, assignment_id)
            .await?;
        Ok(ReceivedSummaryWorker {
            summary,
            artifact: content,
            replayed: prepared.replayed,
        })
    }

    async fn capture_parent(
        &self,
        project_id: &str,
        command: &RequestSummaryWorker,
    ) -> Result<SummaryParentRuntimeCapture, SummaryWorkerServiceError> {
        let project = self.store.get_project(project_id).await?;
        if project.orchestrator.id != command.parent_worker_id {
            return Err(ProjectStoreError::SummaryParentNotCurrent.into());
        }
        let runtime = project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(ProjectStoreError::SummaryParentRuntimeUnverified)?;
        if project.runtime.adapter != runtime.adapter
            || project.runtime.session != runtime.session
            || project.runtime.workspace_id != runtime.workspace_id
        {
            return Err(ProjectStoreError::SummaryWorkerAllocationMismatch.into());
        }
        let inventory = self.source.inventory(&runtime.session).await?;
        capture_parent_runtime(&project, command, &inventory).map_err(Into::into)
    }

    async fn observed_child_instance(&self, summary: &SummaryWorker) -> Option<String> {
        let runtime = summary.assignment.as_ref()?.worker.runtime.as_ref()?;
        let inventory = self.source.inventory(&runtime.session).await.ok()?;
        inventory
            .panes
            .iter()
            .find(|pane| {
                pane.runtime_id == runtime.pane_id
                    && pane.terminal_id == runtime.terminal_id
                    && pane.workspace_id == runtime.workspace_id
                    && Some(pane.tab_id.as_str()) == runtime.tab_id.as_deref()
                    && pane.provider_session == runtime.provider_session
            })
            .and_then(|pane| pane.pane_instance_id.clone())
    }
}

fn capture_parent_runtime(
    project: &Project,
    command: &RequestSummaryWorker,
    inventory: &RuntimeInventory,
) -> Result<SummaryParentRuntimeCapture, ProjectStoreError> {
    if project.orchestrator.id != command.parent_worker_id {
        return Err(ProjectStoreError::SummaryParentNotCurrent);
    }
    let runtime = project
        .orchestrator
        .runtime
        .as_ref()
        .ok_or(ProjectStoreError::SummaryParentRuntimeUnverified)?;
    if project.runtime.adapter != runtime.adapter
        || project.runtime.session != runtime.session
        || project.runtime.workspace_id != runtime.workspace_id
    {
        return Err(ProjectStoreError::SummaryWorkerAllocationMismatch);
    }
    if runtime.observation_state != RuntimeObservationState::Observed
        || runtime.process_state != RuntimeProcessState::Running
        || inventory.adapter != runtime.adapter
        || inventory.session != runtime.session
        || inventory.observed_at_unix_ms < runtime.last_observed_at_unix_ms
    {
        return Err(ProjectStoreError::SummaryParentRuntimeUnverified);
    }
    let pane = inventory
        .panes
        .iter()
        .find(|pane| pane.runtime_id == runtime.pane_id && pane.terminal_id == runtime.terminal_id)
        .filter(|pane| {
            pane.workspace_id == runtime.workspace_id
                && Some(pane.tab_id.as_str()) == runtime.tab_id.as_deref()
                && pane.provider_session == runtime.provider_session
                && pane.revision >= runtime.revision
        })
        .ok_or(ProjectStoreError::SummaryParentRuntimeUnverified)?;
    Ok(SummaryParentRuntimeCapture {
        adapter: runtime.adapter.clone(),
        session: runtime.session.clone(),
        workspace_id: pane.workspace_id.clone(),
        terminal_id: pane.terminal_id.clone(),
        tab_id: pane.tab_id.clone(),
        pane_id: pane.runtime_id.clone(),
        pane_instance_id: pane.pane_instance_id.clone(),
        provider_session: pane.provider_session.clone(),
        observed_at_unix_ms: inventory.observed_at_unix_ms,
    })
}

fn contract_prompt(summary: &SummaryWorker) -> String {
    let assignment = summary
        .assignment
        .as_ref()
        .expect("active summary assignment");
    let receipt_command_id = derived_command_id("summary-receipt", &summary.command_id);
    format!(
        "Complete the summary objective now. Persist exactly one Markdown artifact before \
         recording completion. PUT the artifact to \
         /api/v1/projects/{project}/assignments/{assignment}/artifacts/{artifact} as JSON with \
         actor=\"summary-worker\", attempt_id=\"{attempt}\", \
         expected_assignment_version=\"{assignment_version}\", \
         expected_attempt_version=\"{attempt_version}\", kind=\"markdown\", \
         display_name=\"summary.md\", and content containing the result. Only after that PUT \
         succeeds, POST /api/v1/projects/{project}/assignments/{assignment}/completion-receipts \
         with command_id=\"{receipt_command_id}\", actor=\"summary-worker\", the same attempt \
         and versions, outcome=\"completed\", a concise summary, \
         artifact_ids=[\"{artifact}\"], and unresolved_blockers=[]. The artifact and receipt \
         must retain this exact project, assignment, attempt, and worker provenance. Terminal \
         output and process exit are not completion proof.",
        project = summary.project_id,
        assignment = assignment.id,
        artifact = summary.expected_artifact_id,
        attempt = assignment.attempt.id,
        assignment_version = assignment.version,
        attempt_version = assignment.attempt.version,
    )
}

fn derived_command_id(prefix: &str, command_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(command_id.as_bytes()));
    format!("{prefix}-{}", &digest[..32])
}

#[derive(Debug, Error)]
pub enum SummaryWorkerServiceError {
    #[error(transparent)]
    Invalid(#[from] yard_domain::SummaryWorkerValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Allocation(#[from] AllocationServiceError),
    #[error(transparent)]
    Intervention(#[from] InterventionServiceError),
    #[error(transparent)]
    Artifact(#[from] ArtifactServiceError),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yard_domain::{
        CanvasPlacement, FocusObservation, ObservedStatus, PaneObservation, ProjectPlacement,
        ProjectRuntimeBinding, ProjectWorkflowProfilePin, TabObservation, Worker,
        WorkerDesiredState, WorkerRuntimeBinding, WorkspaceObservation, WorktreeObservation,
    };

    use super::*;

    fn project() -> Project {
        Project {
            id: "project-1".to_owned(),
            name: "Project".to_owned(),
            runtime: ProjectRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "alpha".to_owned(),
                workspace_id: "workspace-parent".to_owned(),
            },
            orchestrator: Worker {
                id: "parent".to_owned(),
                profile_id: None,
                profile_version: None,
                desired_state: WorkerDesiredState::Running,
                runtime: Some(WorkerRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "alpha".to_owned(),
                    workspace_id: "workspace-parent".to_owned(),
                    terminal_id: "terminal-parent".to_owned(),
                    tab_id: Some("tab-parent".to_owned()),
                    pane_id: "pane-parent".to_owned(),
                    provider_session: None,
                    owns_tab: false,
                    observation_state: RuntimeObservationState::Observed,
                    process_state: RuntimeProcessState::Running,
                    status: ObservedStatus::Working,
                    state_change_sequence: 1,
                    revision: 5,
                    version: 1,
                    last_observed_at_unix_ms: 50,
                }),
                version: 1,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            },
            placement: ProjectPlacement {
                geometry: CanvasPlacement {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0,
                },
                version: 1,
                updated_at_unix_ms: 1,
            },
            workflow_profile: ProjectWorkflowProfilePin {
                profile_id: "workflow".to_owned(),
                profile_version: 1,
                pinned_by: "test".to_owned(),
                pinned_at_unix_ms: 1,
            },
            version: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn command() -> RequestSummaryWorker {
        RequestSummaryWorker {
            command_id: "summary-1".to_owned(),
            actor: "local-user".to_owned(),
            parent_worker_id: "parent".to_owned(),
            expected_parent_worker_version: 1,
            expected_project_version: 1,
            profile_id: "profile".to_owned(),
            expected_profile_version: 1,
            artifact_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            objective: "Summarize.".to_owned(),
        }
    }

    fn pane(
        pane_id: &str,
        terminal_id: &str,
        workspace_id: &str,
        tab_id: &str,
        revision: u64,
    ) -> PaneObservation {
        PaneObservation {
            runtime_id: pane_id.to_owned(),
            pane_instance_id: Some(format!("instance-{pane_id}")),
            terminal_id: terminal_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            tab_id: tab_id.to_owned(),
            focused: false,
            cwd: None,
            foreground_cwd: None,
            label: None,
            provider: None,
            display_provider: None,
            status: ObservedStatus::Working,
            tokens: BTreeMap::new(),
            provider_session: None,
            revision,
        }
    }

    fn workspace(runtime_id: &str, focused: bool) -> WorkspaceObservation {
        WorkspaceObservation {
            runtime_id: runtime_id.to_owned(),
            order: usize::from(!focused),
            label: "same-repository".to_owned(),
            focused,
            active_tab_id: format!("tab-{runtime_id}"),
            pane_count: 1,
            tab_count: 1,
            status: ObservedStatus::Working,
            tokens: BTreeMap::new(),
            worktree: Some(WorktreeObservation {
                repository_key: format!("key-{runtime_id}"),
                repository_name: "same-repository".to_owned(),
                repository_root: format!("/repos/{runtime_id}"),
                checkout_path: format!("/checkouts/{runtime_id}"),
                is_linked: true,
            }),
        }
    }

    fn inventory() -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "alpha".to_owned(),
            runtime_version: "test".to_owned(),
            protocol: 1,
            observed_at_unix_ms: 60,
            focus: FocusObservation {
                workspace_id: Some("workspace-other".to_owned()),
                tab_id: Some("tab-other".to_owned()),
                pane_id: Some("pane-other".to_owned()),
            },
            workspaces: vec![
                workspace("workspace-parent", false),
                workspace("workspace-other", true),
            ],
            tabs: vec![
                TabObservation {
                    runtime_id: "tab-parent".to_owned(),
                    workspace_id: "workspace-parent".to_owned(),
                    order: 0,
                    label: "Parent".to_owned(),
                    focused: false,
                    pane_count: 1,
                    status: ObservedStatus::Working,
                },
                TabObservation {
                    runtime_id: "tab-other".to_owned(),
                    workspace_id: "workspace-other".to_owned(),
                    order: 0,
                    label: "Other".to_owned(),
                    focused: true,
                    pane_count: 1,
                    status: ObservedStatus::Working,
                },
            ],
            panes: vec![
                pane(
                    "pane-parent",
                    "terminal-parent",
                    "workspace-parent",
                    "tab-parent",
                    5,
                ),
                pane(
                    "pane-other",
                    "terminal-other",
                    "workspace-other",
                    "tab-other",
                    9,
                ),
            ],
            workers: Vec::new(),
            child_agents: Vec::new(),
        }
    }

    #[test]
    fn capture_rejects_missing_and_stale_parent_pane() {
        let project = project();
        let command = command();
        let mut missing = inventory();
        missing.panes.remove(0);
        assert!(matches!(
            capture_parent_runtime(&project, &command, &missing),
            Err(ProjectStoreError::SummaryParentRuntimeUnverified)
        ));

        let mut stale_observation = inventory();
        stale_observation.observed_at_unix_ms = 49;
        assert!(matches!(
            capture_parent_runtime(&project, &command, &stale_observation),
            Err(ProjectStoreError::SummaryParentRuntimeUnverified)
        ));

        let mut stale_revision = inventory();
        stale_revision.panes[0].revision = 4;
        assert!(matches!(
            capture_parent_runtime(&project, &command, &stale_revision),
            Err(ProjectStoreError::SummaryParentRuntimeUnverified)
        ));
    }

    #[test]
    fn capture_uses_identity_not_focus_or_repository_name() {
        let capture = capture_parent_runtime(&project(), &command(), &inventory()).unwrap();

        assert_eq!(capture.workspace_id, "workspace-parent");
        assert_eq!(capture.pane_id, "pane-parent");
        assert_eq!(
            capture.pane_instance_id.as_deref(),
            Some("instance-pane-parent")
        );
    }

    #[test]
    fn capture_rejects_allocation_workspace_mismatch_before_provisioning() {
        let mut project = project();
        project.runtime.workspace_id = "workspace-allocation-target".to_owned();

        assert!(matches!(
            capture_parent_runtime(&project, &command(), &inventory()),
            Err(ProjectStoreError::SummaryWorkerAllocationMismatch)
        ));
    }
}

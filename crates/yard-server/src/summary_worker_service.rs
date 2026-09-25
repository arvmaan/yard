use std::sync::Arc;

use thiserror::Error;
use yard_domain::{
    ConfirmProfileAllocation, IsolationPolicy, ReceiveSummaryWorker, ReceivedSummaryWorker,
    RequestSummaryWorker, RuntimeObservationState, RuntimeProcessState, SendAssignmentPrompt,
    SummaryParentRuntimeCapture, SummaryWorker, SummaryWorkers,
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
        if runtime.observation_state != RuntimeObservationState::Observed
            || runtime.process_state != RuntimeProcessState::Running
        {
            return Err(ProjectStoreError::SummaryParentRuntimeUnverified.into());
        }
        let inventory = self.source.inventory(&runtime.session).await?;
        if inventory.adapter != runtime.adapter
            || inventory.session != runtime.session
            || inventory.observed_at_unix_ms < runtime.last_observed_at_unix_ms
        {
            return Err(ProjectStoreError::SummaryParentRuntimeUnverified.into());
        }
        let pane = inventory
            .panes
            .iter()
            .find(|pane| {
                pane.runtime_id == runtime.pane_id && pane.terminal_id == runtime.terminal_id
            })
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

use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio::time::{Instant, sleep};
use yard_domain::{
    ConfirmedProjectCreation, CreateProject, CreateProjectFromProfile,
    CreateWorkspaceProjectFromProfile, Project, Projects, RuntimeObservationState,
    RuntimeProcessState, UpdateProjectPlacement, UpdateProjectWorkflowProfile,
    WorkerRuntimeBinding,
};
use yard_store::{
    BeginProfileProjectCreation, BeginWorkspaceProjectCreation, ProjectStoreError, YardStore,
};

use crate::allocation_service::{
    AllocationServiceError, RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest,
    RuntimeWorkspaceProvisionRequest, agent_name, assignment_prompt, provider_args,
    validate_supported_profile,
};
use crate::inventory_service::{InventoryServiceError, InventorySource};
use crate::status_protocol::{with_orchestrator_status_contract, with_orchestrator_workflow};

const RUNTIME_IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ProjectService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    store: Arc<dyn YardStore>,
}

impl ProjectService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        Self {
            source,
            runtime,
            store,
        }
    }

    /// List durable Yard projects.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] when the project store cannot be read.
    pub async fn list(&self) -> Result<Projects, ProjectServiceError> {
        self.store.list_projects().await.map_err(Into::into)
    }

    /// Read one durable Yard project.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] when the project does not exist or the
    /// project store cannot be read.
    pub async fn get(&self, project_id: &str) -> Result<Project, ProjectServiceError> {
        self.store.get_project(project_id).await.map_err(Into::into)
    }

    /// Pin a project to one immutable workflow-profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid input, stale project state,
    /// a missing workflow revision, or persistence failure.
    pub async fn update_workflow_profile(
        &self,
        project_id: &str,
        command: UpdateProjectWorkflowProfile,
    ) -> Result<Project, ProjectServiceError> {
        let command = command.normalize()?;
        self.store
            .update_project_workflow_profile(project_id, command)
            .await
            .map_err(Into::into)
    }

    /// Create a project after proving its Herdr workspace and orchestrator
    /// exist in the same fresh runtime snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid input, unavailable runtime
    /// state, missing binding targets, or persistence failures.
    pub async fn create(&self, project: CreateProject) -> Result<Project, ProjectServiceError> {
        let project = project.normalize()?;
        if project.runtime.adapter != "herdr" {
            return Err(ProjectServiceError::UnsupportedRuntimeAdapter(
                project.runtime.adapter,
            ));
        }

        let inventory = self.source.inventory(&project.runtime.session).await?;
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == project.runtime.workspace_id)
            .ok_or_else(|| ProjectServiceError::RuntimeWorkspaceNotFound {
                session: project.runtime.session.clone(),
                workspace_id: project.runtime.workspace_id.clone(),
            })?;
        let orchestrator = inventory
            .workers
            .iter()
            .find(|worker| worker.runtime_id == project.orchestrator_observed_worker_id)
            .ok_or_else(|| ProjectServiceError::RuntimeWorkerNotFound {
                worker_id: project.orchestrator_observed_worker_id.clone(),
            })?;
        if orchestrator.workspace_id != workspace.runtime_id {
            return Err(ProjectServiceError::OrchestratorOutsideWorkspace {
                worker_id: orchestrator.runtime_id.clone(),
                workspace_id: workspace.runtime_id.clone(),
            });
        }
        let orchestrator_runtime = WorkerRuntimeBinding {
            adapter: project.runtime.adapter.clone(),
            session: project.runtime.session.clone(),
            workspace_id: orchestrator.workspace_id.clone(),
            terminal_id: orchestrator.terminal_id.clone(),
            tab_id: Some(orchestrator.tab_id.clone()),
            pane_id: orchestrator.pane_id.clone(),
            provider_session: orchestrator.provider_session.clone(),
            owns_tab: false,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: orchestrator.status,
            state_change_sequence: orchestrator.state_change_sequence,
            revision: orchestrator.revision,
            version: 1,
            last_observed_at_unix_ms: inventory.observed_at_unix_ms,
        };

        self.store
            .create_project(project, orchestrator_runtime)
            .await
            .map_err(Into::into)
    }

    /// Create a project and its required orchestrator from an immutable worker
    /// profile revision.
    ///
    /// The workspace is reserved durably before Herdr is mutated. A replay of a
    /// pending or ambiguous command never provisions a second worker.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid or stale commands,
    /// unsupported profile capabilities, missing runtime state, provisioning
    /// failures, unverifiable worker identity, or persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn create_from_profile(
        &self,
        command: CreateProjectFromProfile,
    ) -> Result<ConfirmedProjectCreation, ProjectServiceError> {
        let command = command.normalize()?;
        let context = match self.store.begin_profile_project_creation(command).await? {
            BeginProfileProjectCreation::Replayed(project) => return Ok(*project),
            BeginProfileProjectCreation::Started(context) => *context,
        };
        let command_id = context.command.command_id.clone();
        if context.command.runtime.adapter != "herdr" {
            let error = ProjectServiceError::UnsupportedRuntimeAdapter(
                context.command.runtime.adapter.clone(),
            );
            self.store
                .fail_profile_project_creation(&command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_profile_project_creation(&command_id, &message, false)
                .await?;
            return Err(ProjectServiceError::UnsupportedProfile(message));
        }
        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(AllocationServiceError::UnsupportedProfile(message)) => {
                self.store
                    .fail_profile_project_creation(&command_id, &message, false)
                    .await?;
                return Err(ProjectServiceError::UnsupportedProfile(message));
            }
            Err(error) => {
                let message = error.to_string();
                self.store
                    .fail_profile_project_creation(&command_id, &message, false)
                    .await?;
                return Err(ProjectServiceError::UnsupportedProfile(message));
            }
        };

        let inventory = match self
            .source
            .inventory(&context.command.runtime.session)
            .await
        {
            Ok(inventory) => inventory,
            Err(error) => {
                self.store
                    .fail_profile_project_creation(&command_id, &error.to_string(), false)
                    .await?;
                return Err(error.into());
            }
        };
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == context.command.runtime.workspace_id)
            .ok_or_else(|| ProjectServiceError::RuntimeWorkspaceNotFound {
                session: context.command.runtime.session.clone(),
                workspace_id: context.command.runtime.workspace_id.clone(),
            });
        let workspace = match workspace {
            Ok(workspace) => workspace,
            Err(error) => {
                self.store
                    .fail_profile_project_creation(&command_id, &error.to_string(), false)
                    .await?;
                return Err(error);
            }
        };
        let cwd = workspace
            .worktree
            .as_ref()
            .map(|worktree| worktree.checkout_path.clone())
            .or_else(|| {
                inventory
                    .workers
                    .iter()
                    .find(|worker| worker.workspace_id == workspace.runtime_id)
                    .and_then(|worker| worker.foreground_cwd.clone().or_else(|| worker.cwd.clone()))
            })
            .or_else(|| {
                inventory
                    .panes
                    .iter()
                    .find(|pane| pane.workspace_id == workspace.runtime_id)
                    .and_then(|pane| pane.foreground_cwd.clone().or_else(|| pane.cwd.clone()))
            });
        let Some(cwd) = cwd else {
            let error = ProjectServiceError::RuntimeCwdUnavailable;
            self.store
                .fail_profile_project_creation(&command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        };
        let objective = assignment_prompt(
            &context.command.orchestrator_objective,
            "orchestrator",
            &context.profile,
        );
        let objective = with_orchestrator_workflow(&objective, &context.workflow_profile)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let provision = RuntimeProvisionRequest {
            command_id: command_id.clone(),
            session: context.command.runtime.session.clone(),
            workspace_id: context.command.runtime.workspace_id.clone(),
            cwd,
            tab_label: format!("{} orchestrator", context.command.name),
            agent_name: agent_name(&command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt: with_orchestrator_status_contract(&objective, &command_id),
        };

        let prepared = match self.runtime.prepare_worker(provision.clone()).await {
            Ok(prepared) => prepared,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_profile_project_creation(&command_id, &message, false)
                    .await?;
                return Err(ProjectServiceError::RuntimeProvision(message));
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&command_id, *runtime)
                    .await?;
                self.store
                    .fail_profile_project_creation(&command_id, &message, true)
                    .await?;
                return Err(ProjectServiceError::ObjectiveDeliveryFailed(message));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                self.store
                    .fail_profile_project_creation(&command_id, &message, ambiguous)
                    .await?;
                return if ambiguous {
                    Err(ProjectServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(ProjectServiceError::RuntimeProvision(message))
                };
            }
        };
        if let Err(error) = self
            .store
            .claim_provisioning_runtime(&command_id, prepared.clone())
            .await
        {
            self.store
                .fail_profile_project_creation(&command_id, &error.to_string(), true)
                .await?;
            return Err(error.into());
        }

        match self
            .runtime
            .start_prepared_worker(provision, prepared)
            .await
        {
            Ok(runtime) => {
                let unverified_runtime = runtime.clone();
                let runtime = match self.verify_runtime_identity(runtime).await {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command_id, unverified_runtime)
                            .await?;
                        self.store
                            .fail_profile_project_creation(&command_id, &error.to_string(), true)
                            .await?;
                        return Err(error);
                    }
                };
                match self
                    .store
                    .finalize_profile_project_creation(&command_id, runtime.clone())
                    .await
                {
                    Ok(project) => Ok(project),
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command_id, runtime)
                            .await?;
                        self.store
                            .fail_profile_project_creation(&command_id, &error.to_string(), true)
                            .await?;
                        Err(ProjectServiceError::RuntimeProvisionAmbiguous(
                            error.to_string(),
                        ))
                    }
                }
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&command_id, *runtime)
                    .await?;
                self.store
                    .fail_profile_project_creation(&command_id, &message, true)
                    .await?;
                Err(ProjectServiceError::ObjectiveDeliveryFailed(message))
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_profile_project_creation(&command_id, &message, true)
                    .await?;
                Err(ProjectServiceError::RuntimeProvisionAmbiguous(message))
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                if !ambiguous {
                    self.store
                        .release_provisioning_runtime_claim(&command_id)
                        .await?;
                }
                self.store
                    .fail_profile_project_creation(&command_id, &message, ambiguous)
                    .await?;
                if ambiguous {
                    Err(ProjectServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(ProjectServiceError::RuntimeProvision(message))
                }
            }
        }
    }

    /// Create a Herdr workspace and its required orchestrator from an immutable
    /// worker profile revision.
    ///
    /// Yard records the command before mutating Herdr. Because Herdr request
    /// IDs do not deduplicate workspace creation, pending and ambiguous
    /// commands are never automatically replayed.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid or stale commands,
    /// unsupported profiles, runtime failures, unverifiable identities, or
    /// persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn create_with_workspace(
        &self,
        command: CreateWorkspaceProjectFromProfile,
    ) -> Result<ConfirmedProjectCreation, ProjectServiceError> {
        let command = command.normalize()?;
        let context = match self.store.begin_workspace_project_creation(command).await? {
            BeginWorkspaceProjectCreation::Replayed(project) => return Ok(*project),
            BeginWorkspaceProjectCreation::Started(context) => *context,
        };
        let command_id = context.command.command_id.clone();
        if context.command.runtime_adapter != "herdr" {
            let error = ProjectServiceError::UnsupportedRuntimeAdapter(
                context.command.runtime_adapter.clone(),
            );
            self.store
                .fail_workspace_project_creation(&command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_workspace_project_creation(&command_id, &message, false)
                .await?;
            return Err(ProjectServiceError::UnsupportedProfile(message));
        }
        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(AllocationServiceError::UnsupportedProfile(message)) => {
                self.store
                    .fail_workspace_project_creation(&command_id, &message, false)
                    .await?;
                return Err(ProjectServiceError::UnsupportedProfile(message));
            }
            Err(error) => {
                let message = error.to_string();
                self.store
                    .fail_workspace_project_creation(&command_id, &message, false)
                    .await?;
                return Err(ProjectServiceError::UnsupportedProfile(message));
            }
        };
        let objective = assignment_prompt(
            &context.command.orchestrator_objective,
            "orchestrator",
            &context.profile,
        );
        let objective = with_orchestrator_workflow(&objective, &context.workflow_profile)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let provision = RuntimeWorkspaceProvisionRequest {
            command_id: command_id.clone(),
            session: context.command.runtime_session.clone(),
            workspace_label: context.command.workspace_label,
            cwd: context.command.cwd,
            agent_name: agent_name(&command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt: with_orchestrator_status_contract(&objective, &command_id),
        };

        let prepared = match self
            .runtime
            .prepare_workspace_worker(provision.clone())
            .await
        {
            Ok(prepared) => prepared,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_workspace_project_creation(&command_id, &message, false)
                    .await?;
                return Err(ProjectServiceError::RuntimeProvision(message));
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&command_id, *runtime)
                    .await?;
                self.store
                    .fail_workspace_project_creation(&command_id, &message, true)
                    .await?;
                return Err(ProjectServiceError::ObjectiveDeliveryFailed(message));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                self.store
                    .fail_workspace_project_creation(&command_id, &message, ambiguous)
                    .await?;
                return if ambiguous {
                    Err(ProjectServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(ProjectServiceError::RuntimeProvision(message))
                };
            }
        };
        if let Err(error) = self
            .store
            .claim_provisioning_runtime(&command_id, prepared.clone())
            .await
        {
            self.store
                .fail_workspace_project_creation(&command_id, &error.to_string(), true)
                .await?;
            return Err(error.into());
        }

        match self
            .runtime
            .start_prepared_workspace_worker(provision, prepared)
            .await
        {
            Ok(runtime) => {
                let unverified_runtime = runtime.clone();
                let runtime = match self.verify_runtime_identity(runtime).await {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command_id, unverified_runtime)
                            .await?;
                        self.store
                            .fail_workspace_project_creation(&command_id, &error.to_string(), true)
                            .await?;
                        return Err(error);
                    }
                };
                match self
                    .store
                    .finalize_workspace_project_creation(&command_id, runtime.clone())
                    .await
                {
                    Ok(project) => Ok(project),
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command_id, runtime)
                            .await?;
                        self.store
                            .fail_workspace_project_creation(&command_id, &error.to_string(), true)
                            .await?;
                        Err(ProjectServiceError::RuntimeProvisionAmbiguous(
                            error.to_string(),
                        ))
                    }
                }
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                let unverified_runtime = (*runtime).clone();
                let runtime = match self.verify_runtime_identity(*runtime).await {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command_id, unverified_runtime)
                            .await?;
                        self.store
                            .fail_workspace_project_creation(&command_id, &error.to_string(), true)
                            .await?;
                        return Err(error);
                    }
                };
                match self
                    .store
                    .finalize_workspace_project_creation(&command_id, runtime.clone())
                    .await
                {
                    Ok(_) => Err(ProjectServiceError::ObjectiveDeliveryFailed(message)),
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&command_id, runtime)
                            .await?;
                        self.store
                            .fail_workspace_project_creation(&command_id, &error.to_string(), true)
                            .await?;
                        Err(ProjectServiceError::RuntimeProvisionAmbiguous(
                            error.to_string(),
                        ))
                    }
                }
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_workspace_project_creation(&command_id, &message, true)
                    .await?;
                Err(ProjectServiceError::RuntimeProvisionAmbiguous(message))
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                if !ambiguous {
                    self.store
                        .release_provisioning_runtime_claim(&command_id)
                        .await?;
                }
                self.store
                    .fail_workspace_project_creation(&command_id, &message, ambiguous)
                    .await?;
                if ambiguous {
                    Err(ProjectServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(ProjectServiceError::RuntimeProvision(message))
                }
            }
        }
    }

    async fn verify_runtime_identity(
        &self,
        mut runtime: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, ProjectServiceError> {
        let deadline = Instant::now() + RUNTIME_IDENTITY_TIMEOUT;
        loop {
            let inventory = self.source.inventory(&runtime.session).await?;
            let Some(observed) = inventory
                .workers
                .iter()
                .find(|worker| worker.terminal_id == runtime.terminal_id)
            else {
                if Instant::now() < deadline {
                    sleep(Duration::from_millis(100)).await;
                    continue;
                }
                return Err(ProjectServiceError::RuntimeBindingUnverified(
                    "provisioned Herdr terminal is not present in the fresh inventory".to_owned(),
                ));
            };
            if observed.workspace_id != runtime.workspace_id
                || observed.pane_id != runtime.pane_id
                || Some(observed.tab_id.as_str()) != runtime.tab_id.as_deref()
            {
                return Err(ProjectServiceError::RuntimeBindingUnverified(
                    "provisioned Herdr topology changed before Yard could persist it".to_owned(),
                ));
            }
            if let Some(provider_session) = observed.provider_session.clone() {
                if runtime
                    .provider_session
                    .as_ref()
                    .is_some_and(|expected| expected != &provider_session)
                {
                    return Err(ProjectServiceError::RuntimeBindingUnverified(
                        "Herdr reported a different provider session after provisioning".to_owned(),
                    ));
                }
                runtime.provider_session = Some(provider_session);
            }
            runtime.observation_state = RuntimeObservationState::Observed;
            runtime.process_state = RuntimeProcessState::Running;
            runtime.status = observed.status;
            runtime.state_change_sequence = observed.state_change_sequence;
            runtime.revision = observed.revision;
            runtime.last_observed_at_unix_ms = inventory.observed_at_unix_ms;
            return Ok(runtime);
        }
    }

    /// Persist a project region after checking its optimistic version.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid geometry, a missing project,
    /// a concurrent update, or another persistence failure.
    pub async fn update_placement(
        &self,
        project_id: &str,
        update: UpdateProjectPlacement,
    ) -> Result<Project, ProjectServiceError> {
        update.validate()?;
        self.store
            .update_project_placement(project_id, update)
            .await
            .map_err(Into::into)
    }
}

#[derive(Debug, Error)]
pub enum ProjectServiceError {
    #[error(transparent)]
    InvalidProject(#[from] yard_domain::ProjectValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("runtime adapter '{0}' is not supported")]
    UnsupportedRuntimeAdapter(String),
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("Herdr workspace '{workspace_id}' was not found in session '{session}'")]
    RuntimeWorkspaceNotFound {
        session: String,
        workspace_id: String,
    },
    #[error("observed worker '{worker_id}' was not found")]
    RuntimeWorkerNotFound { worker_id: String },
    #[error("observed worker '{worker_id}' does not belong to workspace '{workspace_id}'")]
    OrchestratorOutsideWorkspace {
        worker_id: String,
        workspace_id: String,
    },
    #[error("Herdr did not report a working directory for the project workspace")]
    RuntimeCwdUnavailable,
    #[error("Herdr worker provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("Herdr workspace creation outcome is ambiguous: {0}")]
    RuntimeProvisionAmbiguous(String),
    #[error("Herdr worker binding could not be verified: {0}")]
    RuntimeBindingUnverified(String),
    #[error("worker was created but orchestrator objective delivery failed: {0}")]
    ObjectiveDeliveryFailed(String),
}

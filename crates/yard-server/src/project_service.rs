use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use thiserror::Error;
use tokio::{
    fs,
    process::Command,
    time::{Instant, sleep, timeout},
};
use tracing::warn;
use yard_domain::{
    ArchiveProject, ArchivedProject, ConfirmedProjectCreation, CreateProject,
    CreateProjectFromProfile, CreateWorkspaceProjectFromProfile, DeleteProject, DeletedProject,
    Project, ProjectRepositories, ProjectRepository, Projects, RuntimeObservationState,
    RuntimeProcessState, SetProjectRepository, UpdateProjectPlacement,
    UpdateProjectWorkflowProfile, WorkerRuntimeBinding,
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
use crate::runtime_cleanup_service::RuntimeCleanupService;
use crate::status_protocol::{with_orchestrator_status_contract, with_orchestrator_workflow};

const RUNTIME_IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);
const REPOSITORY_IDENTITY_TIMEOUT: Duration = Duration::from_secs(2);
const REPOSITORY_IDENTITY_OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone)]
pub struct ProjectService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    store: Arc<dyn YardStore>,
    cleanup: RuntimeCleanupService,
}

impl ProjectService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        let cleanup = RuntimeCleanupService::new(Arc::clone(&runtime), Arc::clone(&store));
        Self {
            source,
            runtime,
            store,
            cleanup,
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

    /// List repository checkouts linked to an active project.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] when the project is unavailable.
    pub async fn list_repositories(
        &self,
        project_id: &str,
    ) -> Result<ProjectRepositories, ProjectServiceError> {
        self.store
            .list_project_repositories(project_id)
            .await
            .map_err(Into::into)
    }

    /// Read one repository checkout linked to an active project.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for missing projects or repositories.
    pub async fn get_repository(
        &self,
        project_id: &str,
        repository_id: &str,
    ) -> Result<ProjectRepository, ProjectServiceError> {
        self.store
            .get_project_repository(project_id, repository_id)
            .await
            .map_err(Into::into)
    }

    /// Link a validated Git checkout to an active project.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid paths, non-Git directories,
    /// duplicate links, missing projects, or unavailable identity checks.
    pub async fn link_repository(
        &self,
        project_id: &str,
        repository_id: &str,
        repository: SetProjectRepository,
    ) -> Result<ProjectRepository, ProjectServiceError> {
        self.store.get_project(project_id).await?;
        let identity = resolve_repository_identity(repository).await?;
        self.store
            .create_project_repository(
                project_id,
                repository_id,
                &identity.root_path,
                &identity.git_common_dir,
            )
            .await
            .map_err(Into::into)
    }

    /// Relink a repository association to another checkout of the same Git
    /// common directory.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for invalid paths, missing ownership, or
    /// an identity-changing relink.
    pub async fn relink_repository(
        &self,
        project_id: &str,
        repository_id: &str,
        repository: SetProjectRepository,
    ) -> Result<ProjectRepository, ProjectServiceError> {
        let current = self
            .store
            .get_project_repository(project_id, repository_id)
            .await?;
        let identity = resolve_repository_identity(repository).await?;
        if identity.git_common_dir != current.git_common_dir {
            return Err(ProjectServiceError::RepositoryIdentityChanged);
        }
        self.store
            .update_project_repository(
                project_id,
                repository_id,
                &identity.root_path,
                &identity.git_common_dir,
            )
            .await
            .map_err(Into::into)
    }

    /// Remove a repository association from an active project.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for missing ownership or storage errors.
    pub async fn unlink_repository(
        &self,
        project_id: &str,
        repository_id: &str,
    ) -> Result<ProjectRepository, ProjectServiceError> {
        self.store
            .delete_project_repository(project_id, repository_id)
            .await
            .map_err(Into::into)
    }

    /// Archive a durable project and immediately attempt cleanup of its
    /// orchestrator runtime.
    ///
    /// The project and its historical work remain in storage, while active
    /// project queries stop returning it. Runtime cleanup failures remain
    /// durably queued for the background retry loop.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] for stale input, active project work,
    /// pending orchestrator interventions, or persistence failure.
    pub async fn archive(
        &self,
        project_id: &str,
        command: ArchiveProject,
    ) -> Result<ArchivedProject, ProjectServiceError> {
        let command_id = command.command_id.clone();
        let mut archived = self.store.archive_project(project_id, command).await?;
        if !archived.cleanup_pending {
            return Ok(archived);
        }

        match self.cleanup.process_command(&command_id).await {
            Ok(report) if report.attempted > 0 => {
                archived.cleanup_pending = report.failed > 0;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    command_id,
                    project_id,
                    error = %error,
                    "Immediate archived-project runtime cleanup failed; durable retry remains pending"
                );
            }
        }
        Ok(archived)
    }

    /// Permanently remove an archived project and its orchestrator from normal
    /// Yard UI queries while retaining durable audit and cleanup records.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectServiceError`] when the project is not archived, was
    /// already deleted, or persistence fails.
    pub async fn delete(
        &self,
        project_id: &str,
        command: DeleteProject,
    ) -> Result<DeletedProject, ProjectServiceError> {
        self.store
            .delete_project(project_id, command)
            .await
            .map_err(Into::into)
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

struct RepositoryIdentity {
    root_path: String,
    git_common_dir: String,
}

async fn resolve_repository_identity(
    repository: SetProjectRepository,
) -> Result<RepositoryIdentity, ProjectServiceError> {
    let repository = repository.normalize()?;
    let requested = PathBuf::from(&repository.root_path);
    let canonical_requested = fs::canonicalize(&requested).await.map_err(|_| {
        ProjectServiceError::RepositoryRootUnavailable(repository.root_path.clone())
    })?;
    if !fs::metadata(&canonical_requested)
        .await
        .map_err(|_| ProjectServiceError::RepositoryRootUnavailable(repository.root_path.clone()))?
        .is_dir()
    {
        return Err(ProjectServiceError::RepositoryRootUnavailable(
            repository.root_path,
        ));
    }

    let root = canonical_git_directory(
        git_repository_path(&canonical_requested, "--show-toplevel").await?,
    )
    .await?;
    if !canonical_requested.starts_with(&root) {
        return Err(ProjectServiceError::RepositoryNotGitWorktree(
            canonical_requested.display().to_string(),
        ));
    }
    let git_common_dir = canonical_git_directory(
        git_repository_path(&canonical_requested, "--git-common-dir").await?,
    )
    .await?;

    Ok(RepositoryIdentity {
        root_path: path_string(root)?,
        git_common_dir: path_string(git_common_dir)?,
    })
}

async fn git_repository_path(cwd: &Path, argument: &str) -> Result<PathBuf, ProjectServiceError> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "-c",
            "color.ui=false",
            "-c",
            "core.fsmonitor=false",
            "rev-parse",
            "--path-format=absolute",
            argument,
        ])
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = timeout(REPOSITORY_IDENTITY_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            ProjectServiceError::RepositoryIdentityUnavailable(
                "Git identity check timed out".to_owned(),
            )
        })?
        .map_err(|error| ProjectServiceError::RepositoryIdentityUnavailable(error.to_string()))?;
    if output.stdout.len() > REPOSITORY_IDENTITY_OUTPUT_LIMIT
        || output.stderr.len() > REPOSITORY_IDENTITY_OUTPUT_LIMIT
    {
        return Err(ProjectServiceError::RepositoryIdentityUnavailable(
            "Git identity output exceeded the limit".to_owned(),
        ));
    }
    if !output.status.success() {
        return Err(ProjectServiceError::RepositoryNotGitWorktree(
            cwd.display().to_string(),
        ));
    }
    let output = String::from_utf8(output.stdout).map_err(|_| {
        ProjectServiceError::RepositoryIdentityUnavailable(
            "Git returned a non-UTF-8 repository path".to_owned(),
        )
    })?;
    let output = output.trim_end_matches(['\r', '\n']);
    if output.is_empty() || output.contains('\r') || output.contains('\n') {
        return Err(ProjectServiceError::RepositoryIdentityUnavailable(
            "Git returned an invalid repository path".to_owned(),
        ));
    }
    Ok(PathBuf::from(output))
}

async fn canonical_git_directory(path: PathBuf) -> Result<PathBuf, ProjectServiceError> {
    let canonical = fs::canonicalize(&path)
        .await
        .map_err(|error| ProjectServiceError::RepositoryIdentityUnavailable(error.to_string()))?;
    if !fs::metadata(&canonical)
        .await
        .map_err(|error| ProjectServiceError::RepositoryIdentityUnavailable(error.to_string()))?
        .is_dir()
    {
        return Err(ProjectServiceError::RepositoryIdentityUnavailable(
            "Git identity is not a directory".to_owned(),
        ));
    }
    Ok(canonical)
}

fn path_string(path: PathBuf) -> Result<String, ProjectServiceError> {
    path.into_os_string().into_string().map_err(|_| {
        ProjectServiceError::RepositoryIdentityUnavailable(
            "Repository identity is not valid UTF-8".to_owned(),
        )
    })
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
    #[error("repository root is unavailable: {0}")]
    RepositoryRootUnavailable(String),
    #[error("path is not a Git worktree: {0}")]
    RepositoryNotGitWorktree(String),
    #[error("repository identity is unavailable: {0}")]
    RepositoryIdentityUnavailable(String),
    #[error("repository relink would change Git identity")]
    RepositoryIdentityChanged,
}

#[cfg(test)]
mod tests {
    use std::{fs as std_fs, process::Command as StdCommand};

    use async_trait::async_trait;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, ObservedStatus, ProjectRuntimeBinding, RuntimeInventory, RuntimeSessions,
    };
    use yard_store::SqliteProjectStore;

    use super::*;

    struct UnusedInventory;

    #[async_trait]
    impl InventorySource for UnusedInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            unreachable!("repository tests do not inspect runtime inventory")
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            unreachable!("repository tests do not inspect runtime inventory")
        }
    }

    struct UnusedRuntime;

    #[async_trait]
    impl RuntimeControl for UnusedRuntime {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            unreachable!("repository tests do not provision workers")
        }
    }

    async fn service(
        temp: &TempDir,
        suffix: &str,
    ) -> (ProjectService, Arc<SqliteProjectStore>, Project) {
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let project = store
            .create_project(
                CreateProject {
                    name: format!("Repository {suffix}"),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: format!("workspace-{suffix}"),
                    },
                    orchestrator_observed_worker_id: format!("terminal-{suffix}"),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                WorkerRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "default".to_owned(),
                    workspace_id: format!("workspace-{suffix}"),
                    terminal_id: format!("terminal-{suffix}"),
                    tab_id: Some(format!("tab-{suffix}")),
                    pane_id: format!("pane-{suffix}"),
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
            .await
            .unwrap();
        let service = ProjectService::new(
            Arc::new(UnusedInventory),
            Arc::new(UnusedRuntime),
            store.clone(),
        );
        (service, store, project)
    }

    fn init_repository(path: &Path) {
        std_fs::create_dir_all(path).unwrap();
        assert!(
            StdCommand::new("git")
                .args(["init", "--quiet"])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            StdCommand::new("git")
                .current_dir(path)
                .args([
                    "-c",
                    "user.name=Yard Tests",
                    "-c",
                    "user.email=yard@example.invalid",
                    "commit",
                    "--allow-empty",
                    "--quiet",
                    "-m",
                    "initial",
                ])
                .status()
                .unwrap()
                .success()
        );
    }

    fn add_worktree(repository: &Path, worktree: &Path) {
        assert!(
            StdCommand::new("git")
                .current_dir(repository)
                .args(["worktree", "add", "--detach", "--quiet"])
                .arg(worktree)
                .arg("HEAD")
                .status()
                .unwrap()
                .success()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_identity_is_derived_and_preserved_across_relinks() {
        let temp = TempDir::new().unwrap();
        let (service, store, project) = service(&temp, "identity").await;
        let repository = temp.path().join("repository");
        init_repository(&repository);
        std_fs::create_dir(repository.join("nested")).unwrap();
        let alias = temp.path().join("repository-alias");
        std::os::unix::fs::symlink(&repository, &alias).unwrap();

        let linked = service
            .link_repository(
                &project.id,
                "repository-1",
                SetProjectRepository {
                    root_path: alias.join("nested").to_string_lossy().into_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            linked.root_path,
            std_fs::canonicalize(&repository).unwrap().to_string_lossy()
        );
        assert_eq!(
            linked.git_common_dir,
            std_fs::canonicalize(repository.join(".git"))
                .unwrap()
                .to_string_lossy()
        );
        assert!(matches!(
            service
                .link_repository(
                    &project.id,
                    "repository-alias",
                    SetProjectRepository {
                        root_path: repository.to_string_lossy().into_owned(),
                    },
                )
                .await,
            Err(ProjectServiceError::Store(
                ProjectStoreError::ProjectRepositoryAlreadyLinked
            )),
        ));

        let worktree = temp.path().join("worktree");
        add_worktree(&repository, &worktree);
        let relinked = service
            .relink_repository(
                &project.id,
                &linked.id,
                SetProjectRepository {
                    root_path: worktree.to_string_lossy().into_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(relinked.git_common_dir, linked.git_common_dir);

        let other = temp.path().join("other-repository");
        init_repository(&other);
        assert!(matches!(
            service
                .relink_repository(
                    &project.id,
                    &linked.id,
                    SetProjectRepository {
                        root_path: other.to_string_lossy().into_owned(),
                    },
                )
                .await,
            Err(ProjectServiceError::RepositoryIdentityChanged),
        ));

        store
            .create_project_repository(
                &project.id,
                "repository-forged",
                &std_fs::canonicalize(&other).unwrap().to_string_lossy(),
                "/forged/common-dir",
            )
            .await
            .unwrap();
        assert!(matches!(
            service
                .relink_repository(
                    &project.id,
                    "repository-forged",
                    SetProjectRepository {
                        root_path: other.to_string_lossy().into_owned(),
                    },
                )
                .await,
            Err(ProjectServiceError::RepositoryIdentityChanged),
        ));
    }

    #[tokio::test]
    async fn repository_identity_rejects_unusable_roots() {
        let temp = TempDir::new().unwrap();
        let (service, _store, project) = service(&temp, "invalid").await;

        let relative = service
            .link_repository(
                &project.id,
                "relative",
                SetProjectRepository {
                    root_path: "relative".to_owned(),
                },
            )
            .await;
        assert!(matches!(
            relative,
            Err(ProjectServiceError::InvalidProject(
                yard_domain::ProjectValidationError::InvalidRepositoryRoot
            )),
        ));

        let missing = temp.path().join("missing");
        assert!(matches!(
            service
                .link_repository(
                    &project.id,
                    "missing",
                    SetProjectRepository {
                        root_path: missing.to_string_lossy().into_owned(),
                    },
                )
                .await,
            Err(ProjectServiceError::RepositoryRootUnavailable(_)),
        ));

        let file = temp.path().join("file");
        std_fs::write(&file, "not a directory").unwrap();
        assert!(matches!(
            service
                .link_repository(
                    &project.id,
                    "file",
                    SetProjectRepository {
                        root_path: file.to_string_lossy().into_owned(),
                    },
                )
                .await,
            Err(ProjectServiceError::RepositoryRootUnavailable(_)),
        ));

        let directory = temp.path().join("not-git");
        std_fs::create_dir(&directory).unwrap();
        assert!(matches!(
            service
                .link_repository(
                    &project.id,
                    "not-git",
                    SetProjectRepository {
                        root_path: directory.to_string_lossy().into_owned(),
                    },
                )
                .await,
            Err(ProjectServiceError::RepositoryNotGitWorktree(_)),
        ));
    }
}

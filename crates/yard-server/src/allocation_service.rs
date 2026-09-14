use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use yard_domain::{
    Assignments, ConfirmProfileAllocation, ConfirmWorkerAllocation, ConfirmWorkerHandoff,
    ConfirmedAllocation, ConfirmedWorkerHandoff, HandoffTargetRole, ProviderSessionRef,
    RecordCompletionReceipt, RecordedCompletionReceipt, RuntimeObservationState,
    RuntimeProcessState, Worker, WorkerCandidates, WorkerProfile, WorkerRuntimeBinding,
    herdr_agent_name,
};
use yard_store::{
    BeginProfileAllocation, BeginWorkerAllocation, BeginWorkerHandoff, ProjectStoreError, YardStore,
};

use crate::intervention_service::{
    RuntimeIntervention, RuntimeInterventionError, RuntimePromptRequest,
};
use crate::inventory_service::{InventoryServiceError, InventorySource};
use crate::runtime_cleanup_service::RuntimeCleanupService;
use crate::status_protocol::{with_orchestrator_status_contract, with_orchestrator_workflow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProvisionRequest {
    pub command_id: String,
    pub session: String,
    pub workspace_id: String,
    pub cwd: String,
    pub tab_label: String,
    pub agent_name: String,
    pub kind: String,
    pub args: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeWorkspaceProvisionRequest {
    pub command_id: String,
    pub session: String,
    pub workspace_label: String,
    pub cwd: String,
    pub agent_name: String,
    pub kind: String,
    pub args: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionRequest {
    pub session: String,
    pub startup_cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeWorkerRestartRequest {
    pub command_id: String,
    pub runtime: WorkerRuntimeBinding,
    pub agent_name: String,
    pub kind: String,
    pub args: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Error)]
pub enum RuntimeProvisionError {
    #[error("{0}")]
    BeforeWorker(String),
    #[error("{message}")]
    PromptDelivery {
        runtime: Box<WorkerRuntimeBinding>,
        message: String,
    },
    #[error("{message}")]
    AfterPreparation {
        message: String,
        ambiguous: bool,
        started_runtime: Option<Box<WorkerRuntimeBinding>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRetirementRequest {
    pub cleanup_id: String,
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub provider_session: Option<ProviderSessionRef>,
    pub owns_tab: bool,
}

#[derive(Debug, Error)]
pub enum RuntimeRetirementError {
    #[error("runtime adapter '{0}' does not support destructive retirement")]
    UnsupportedAdapter(String),
    #[error("{0}")]
    Runtime(String),
    #[error("the captured runtime identity is not currently observable")]
    IdentityNotObserved,
    #[error(
        "the installed Herdr protocol cannot atomically guard tab.close or pane.close by runtime identity"
    )]
    AtomicIdentityGuardUnavailable,
}

#[async_trait]
pub trait RuntimeControl: Send + Sync {
    async fn ensure_session(
        &self,
        _request: RuntimeSessionRequest,
    ) -> Result<(), RuntimeProvisionError> {
        Err(RuntimeProvisionError::BeforeWorker(
            "runtime does not support named session startup".to_owned(),
        ))
    }

    async fn restart_worker(
        &self,
        _request: RuntimeWorkerRestartRequest,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        Err(RuntimeProvisionError::BeforeWorker(
            "runtime does not support restarting a worker in an existing pane".to_owned(),
        ))
    }

    async fn bootstrap_worker(
        &self,
        _request: RuntimeWorkspaceProvisionRequest,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        Err(RuntimeProvisionError::BeforeWorker(
            "runtime does not support workspace bootstrap".to_owned(),
        ))
    }

    async fn prepare_workspace_worker(
        &self,
        request: RuntimeWorkspaceProvisionRequest,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        self.bootstrap_worker(request).await
    }

    async fn start_prepared_workspace_worker(
        &self,
        _request: RuntimeWorkspaceProvisionRequest,
        prepared: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        Ok(prepared)
    }

    async fn provision_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError>;

    async fn prepare_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        self.provision_worker(request).await
    }

    async fn start_prepared_worker(
        &self,
        _request: RuntimeProvisionRequest,
        prepared: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        Ok(prepared)
    }

    async fn prepare_replacement_worker(
        &self,
        _request: RuntimeProvisionRequest,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        Err(RuntimeProvisionError::BeforeWorker(
            "runtime does not support staged orchestrator replacement".to_owned(),
        ))
    }

    async fn start_prepared_replacement_worker(
        &self,
        _request: RuntimeProvisionRequest,
        _prepared: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
        Err(RuntimeProvisionError::BeforeWorker(
            "runtime does not support staged orchestrator replacement".to_owned(),
        ))
    }

    async fn retire_runtime(
        &self,
        request: RuntimeRetirementRequest,
    ) -> Result<(), RuntimeRetirementError> {
        Err(RuntimeRetirementError::UnsupportedAdapter(request.adapter))
    }
}

#[derive(Clone)]
pub struct AllocationService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    store: Arc<dyn YardStore>,
    cleanup: RuntimeCleanupService,
}

impl AllocationService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        intervention: Arc<dyn RuntimeIntervention>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        let cleanup = RuntimeCleanupService::new(Arc::clone(&runtime), Arc::clone(&store));
        Self {
            source,
            runtime,
            intervention,
            store,
            cleanup,
        }
    }

    /// Confirm a profile proposal, create one Herdr-backed worker, and deliver
    /// its initial assignment exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`AllocationServiceError`] for invalid or stale commands,
    /// unsupported profile capabilities, unavailable runtime state, Herdr
    /// provisioning failures, or persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn confirm(
        &self,
        project_id: &str,
        command: ConfirmProfileAllocation,
    ) -> Result<ConfirmedAllocation, AllocationServiceError> {
        let command = command.normalize()?;
        let context = match self
            .store
            .begin_profile_allocation(project_id, command)
            .await?
        {
            BeginProfileAllocation::Replayed(allocation) => return Ok(*allocation),
            BeginProfileAllocation::Started(context) => *context,
        };
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_profile_allocation(&context.command.command_id, &message, false)
                .await?;
            return Err(AllocationServiceError::UnsupportedProfile(message));
        }
        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(error) => {
                self.store
                    .fail_profile_allocation(&context.command.command_id, &error.to_string(), false)
                    .await?;
                return Err(error);
            }
        };

        let inventory = match self
            .source
            .inventory(&context.project.runtime.session)
            .await
        {
            Ok(inventory) => inventory,
            Err(error) => {
                self.store
                    .fail_profile_allocation(&context.command.command_id, &error.to_string(), false)
                    .await?;
                return Err(error.into());
            }
        };
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == context.project.runtime.workspace_id)
            .ok_or_else(|| AllocationServiceError::RuntimeWorkspaceMissing {
                workspace_id: context.project.runtime.workspace_id.clone(),
            });
        let workspace = match workspace {
            Ok(workspace) => workspace,
            Err(error) => {
                self.store
                    .fail_profile_allocation(&context.command.command_id, &error.to_string(), false)
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
            });
        let Some(cwd) = cwd else {
            let error = AllocationServiceError::RuntimeCwdUnavailable;
            self.store
                .fail_profile_allocation(&context.command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        };
        let provision = RuntimeProvisionRequest {
            command_id: context.command.command_id.clone(),
            session: context.project.runtime.session,
            workspace_id: context.project.runtime.workspace_id,
            cwd,
            tab_label: context.profile.spec.name.clone(),
            agent_name: agent_name(&context.command.command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt: assignment_prompt(
                &context.command.objective,
                &context.command.role,
                &context.profile,
            ),
        };

        let prepared = match self.runtime.prepare_worker(provision.clone()).await {
            Ok(prepared) => prepared,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_profile_allocation(&context.command.command_id, &message, false)
                    .await?;
                return Err(AllocationServiceError::RuntimeProvision(message));
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                    .await?;
                self.store
                    .fail_profile_allocation(&context.command.command_id, &message, true)
                    .await?;
                return Err(AllocationServiceError::ObjectiveDeliveryFailed(message));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                self.store
                    .fail_profile_allocation(&context.command.command_id, &message, ambiguous)
                    .await?;
                return if ambiguous {
                    Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(AllocationServiceError::RuntimeProvision(message))
                };
            }
        };
        if let Err(error) = self
            .store
            .claim_provisioning_runtime(&context.command.command_id, prepared.clone())
            .await
        {
            self.store
                .fail_profile_allocation(&context.command.command_id, &error.to_string(), true)
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
                            .quarantine_provisioning_runtime(
                                &context.command.command_id,
                                unverified_runtime,
                            )
                            .await?;
                        self.store
                            .fail_profile_allocation(
                                &context.command.command_id,
                                &error.to_string(),
                                true,
                            )
                            .await?;
                        return Err(error);
                    }
                };
                if let Err(error) = self
                    .store
                    .persist_runtime_allocation(&context.command.command_id, runtime.clone())
                    .await
                {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, runtime)
                        .await?;
                    self.store
                        .fail_profile_allocation(
                            &context.command.command_id,
                            &error.to_string(),
                            true,
                        )
                        .await?;
                    return Err(AllocationServiceError::RuntimeProvisionAmbiguous(
                        error.to_string(),
                    ));
                }
                self.store
                    .activate_profile_allocation(&context.command.command_id)
                    .await
                    .map_err(Into::into)
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                    .await?;
                self.store
                    .fail_profile_allocation(&context.command.command_id, &message, true)
                    .await?;
                Err(AllocationServiceError::ObjectiveDeliveryFailed(message))
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_profile_allocation(&context.command.command_id, &message, true)
                    .await?;
                Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                if !ambiguous {
                    self.store
                        .release_provisioning_runtime_claim(&context.command.command_id)
                        .await?;
                }
                self.store
                    .fail_profile_allocation(&context.command.command_id, &message, ambiguous)
                    .await?;
                if ambiguous {
                    Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(AllocationServiceError::RuntimeProvision(message))
                }
            }
        }
    }

    /// Confirm allocation of an existing worker, preserving its Yard identity.
    ///
    /// Live workers are validated and prompted in place. Resumable workers get
    /// one explicitly provisioned replacement runtime from their pinned
    /// profile.
    ///
    /// # Errors
    ///
    /// Returns [`AllocationServiceError`] for stale worker state, unsupported
    /// profiles, runtime validation or delivery failures, and persistence
    /// failures.
    #[allow(clippy::too_many_lines)]
    pub async fn confirm_worker(
        &self,
        project_id: &str,
        command: ConfirmWorkerAllocation,
    ) -> Result<ConfirmedAllocation, AllocationServiceError> {
        let command = command.normalize()?;
        let context = match self
            .store
            .begin_worker_allocation(project_id, command)
            .await?
        {
            BeginWorkerAllocation::Replayed(allocation) => return Ok(*allocation),
            BeginWorkerAllocation::Started(context) => *context,
        };
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_worker_allocation(&context.command.command_id, &message, false)
                .await?;
            return Err(AllocationServiceError::UnsupportedProfile(message));
        }

        if !context.replace_runtime {
            let runtime = match self.verify_live_worker(&context.worker).await {
                Ok(runtime) => runtime,
                Err(error) => {
                    self.store
                        .fail_worker_allocation(
                            &context.command.command_id,
                            &error.to_string(),
                            false,
                        )
                        .await?;
                    return Err(error);
                }
            };
            let result = self
                .intervention
                .prompt(RuntimePromptRequest {
                    command_id: context.command.command_id.clone(),
                    session: runtime.session,
                    pane_id: runtime.pane_id,
                    text: assignment_prompt(
                        &context.command.objective,
                        &context.command.role,
                        &context.profile,
                    ),
                })
                .await;
            return match result {
                Ok(_) => {
                    if let Err(error) = self.verify_live_worker(&context.worker).await {
                        let error = RuntimeInterventionError::Ambiguous(format!(
                            "Herdr acknowledged the assignment, but the worker binding changed: \
                             {error}"
                        ));
                        self.store
                            .fail_worker_allocation(
                                &context.command.command_id,
                                &error.to_string(),
                                true,
                            )
                            .await?;
                        return Err(error.into());
                    }
                    self.store
                        .activate_worker_allocation(&context.command.command_id)
                        .await
                        .map_err(Into::into)
                }
                Err(error) => {
                    let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                    self.store
                        .fail_worker_allocation(
                            &context.command.command_id,
                            &error.to_string(),
                            ambiguous,
                        )
                        .await?;
                    Err(error.into())
                }
            };
        }

        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(error) => {
                self.store
                    .fail_worker_allocation(&context.command.command_id, &error.to_string(), false)
                    .await?;
                return Err(error);
            }
        };
        let inventory = match self
            .source
            .inventory(&context.project.runtime.session)
            .await
        {
            Ok(inventory) => inventory,
            Err(error) => {
                self.store
                    .fail_worker_allocation(&context.command.command_id, &error.to_string(), false)
                    .await?;
                return Err(error.into());
            }
        };
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == context.project.runtime.workspace_id)
            .ok_or_else(|| AllocationServiceError::RuntimeWorkspaceMissing {
                workspace_id: context.project.runtime.workspace_id.clone(),
            });
        let workspace = match workspace {
            Ok(workspace) => workspace,
            Err(error) => {
                self.store
                    .fail_worker_allocation(&context.command.command_id, &error.to_string(), false)
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
            });
        let Some(cwd) = cwd else {
            let error = AllocationServiceError::RuntimeCwdUnavailable;
            self.store
                .fail_worker_allocation(&context.command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        };
        let provision = RuntimeProvisionRequest {
            command_id: context.command.command_id.clone(),
            session: context.project.runtime.session,
            workspace_id: context.project.runtime.workspace_id,
            cwd,
            tab_label: context.profile.spec.name.clone(),
            agent_name: agent_name(&context.command.command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt: assignment_prompt(
                &context.command.objective,
                &context.command.role,
                &context.profile,
            ),
        };

        let prepared = match self.runtime.prepare_worker(provision.clone()).await {
            Ok(prepared) => prepared,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_worker_allocation(&context.command.command_id, &message, false)
                    .await?;
                return Err(AllocationServiceError::RuntimeProvision(message));
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                    .await?;
                self.store
                    .fail_worker_allocation(&context.command.command_id, &message, true)
                    .await?;
                return Err(AllocationServiceError::ObjectiveDeliveryFailed(message));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                self.store
                    .fail_worker_allocation(&context.command.command_id, &message, ambiguous)
                    .await?;
                return if ambiguous {
                    Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(AllocationServiceError::RuntimeProvision(message))
                };
            }
        };
        if let Err(error) = self
            .store
            .claim_provisioning_runtime(&context.command.command_id, prepared.clone())
            .await
        {
            self.store
                .fail_worker_allocation(&context.command.command_id, &error.to_string(), true)
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
                            .quarantine_provisioning_runtime(
                                &context.command.command_id,
                                unverified_runtime,
                            )
                            .await?;
                        self.store
                            .fail_worker_allocation(
                                &context.command.command_id,
                                &error.to_string(),
                                true,
                            )
                            .await?;
                        return Err(error);
                    }
                };
                if let Err(error) = self
                    .store
                    .replace_worker_runtime(&context.command.command_id, runtime.clone())
                    .await
                {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, runtime)
                        .await?;
                    self.store
                        .fail_worker_allocation(
                            &context.command.command_id,
                            &error.to_string(),
                            true,
                        )
                        .await?;
                    return Err(AllocationServiceError::RuntimeProvisionAmbiguous(
                        error.to_string(),
                    ));
                }
                self.store
                    .activate_worker_allocation(&context.command.command_id)
                    .await
                    .map_err(Into::into)
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                    .await?;
                self.store
                    .fail_worker_allocation(&context.command.command_id, &message, true)
                    .await?;
                Err(AllocationServiceError::ObjectiveDeliveryFailed(message))
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_worker_allocation(&context.command.command_id, &message, true)
                    .await?;
                Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                if !ambiguous {
                    self.store
                        .release_provisioning_runtime_claim(&context.command.command_id)
                        .await?;
                }
                self.store
                    .fail_worker_allocation(&context.command.command_id, &message, ambiguous)
                    .await?;
                if ambiguous {
                    Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(AllocationServiceError::RuntimeProvision(message))
                }
            }
        }
    }

    /// Confirm a cross-project worker handoff by provisioning one replacement
    /// runtime in the target workspace and preserving the Yard worker identity.
    ///
    /// The source assignment remains reserved while Herdr is called. A
    /// pre-creation failure rolls it back; any post-creation uncertainty is
    /// retained as ambiguous and is never resubmitted automatically.
    ///
    /// # Errors
    ///
    /// Returns [`AllocationServiceError`] for stale state, unsupported profile
    /// capabilities, unavailable target runtime state, Herdr failures, or
    /// persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn confirm_handoff(
        &self,
        source_project_id: &str,
        source_assignment_id: &str,
        command: ConfirmWorkerHandoff,
    ) -> Result<ConfirmedWorkerHandoff, AllocationServiceError> {
        let command = command.normalize()?;
        let command_id = command.command_id.clone();
        let context = match self
            .store
            .begin_worker_handoff(source_project_id, source_assignment_id, command)
            .await?
        {
            BeginWorkerHandoff::Replayed(handoff) => {
                self.process_runtime_cleanup(&command_id).await;
                return Ok(*handoff);
            }
            BeginWorkerHandoff::Started(context) => *context,
        };
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_worker_handoff(&context.command.command_id, &message, false)
                .await?;
            return Err(AllocationServiceError::UnsupportedProfile(message));
        }
        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(error) => {
                self.store
                    .fail_worker_handoff(&context.command.command_id, &error.to_string(), false)
                    .await?;
                return Err(error);
            }
        };
        let inventory = match self
            .source
            .inventory(&context.target_project.runtime.session)
            .await
        {
            Ok(inventory) => inventory,
            Err(error) => {
                self.store
                    .fail_worker_handoff(&context.command.command_id, &error.to_string(), false)
                    .await?;
                return Err(error.into());
            }
        };
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == context.target_project.runtime.workspace_id)
            .ok_or_else(|| AllocationServiceError::RuntimeWorkspaceMissing {
                workspace_id: context.target_project.runtime.workspace_id.clone(),
            });
        let workspace = match workspace {
            Ok(workspace) => workspace,
            Err(error) => {
                self.store
                    .fail_worker_handoff(&context.command.command_id, &error.to_string(), false)
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
            });
        let Some(cwd) = cwd else {
            let error = AllocationServiceError::RuntimeCwdUnavailable;
            self.store
                .fail_worker_handoff(&context.command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        };
        let prompt = assignment_prompt(
            &context.command.objective,
            &context.command.role,
            &context.profile,
        );
        let prompt = if context.command.target_role == HandoffTargetRole::Orchestrator {
            let prompt = with_orchestrator_workflow(&prompt, &context.workflow_profile)
                .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
            with_orchestrator_status_contract(&prompt, &context.command.command_id)
        } else {
            prompt
        };
        let provision = RuntimeProvisionRequest {
            command_id: context.command.command_id.clone(),
            session: context.target_project.runtime.session,
            workspace_id: context.target_project.runtime.workspace_id,
            cwd,
            tab_label: context.profile.spec.name.clone(),
            agent_name: agent_name(&context.command.command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt,
        };

        let prepared = match self.runtime.prepare_worker(provision.clone()).await {
            Ok(runtime) => runtime,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_worker_handoff(&context.command.command_id, &message, false)
                    .await?;
                return Err(AllocationServiceError::RuntimeProvision(message));
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                self.store
                    .fail_worker_handoff(&context.command.command_id, &message, ambiguous)
                    .await?;
                return if ambiguous {
                    Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(AllocationServiceError::RuntimeProvision(message))
                };
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                let claim = self
                    .store
                    .claim_worker_handoff_runtime(&context.command.command_id, *runtime)
                    .await;
                let retained_message = match claim {
                    Ok(()) => message,
                    Err(error) => format!("{message}; target runtime claim failed: {error}"),
                };
                self.store
                    .fail_worker_handoff(&context.command.command_id, &retained_message, true)
                    .await?;
                return Err(AllocationServiceError::ObjectiveDeliveryFailed(
                    retained_message,
                ));
            }
        };
        if let Err(error) = self
            .store
            .claim_worker_handoff_runtime(&context.command.command_id, prepared.clone())
            .await
        {
            self.store
                .fail_worker_handoff(&context.command.command_id, &error.to_string(), true)
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
                            .quarantine_provisioning_runtime(
                                &context.command.command_id,
                                unverified_runtime,
                            )
                            .await?;
                        self.store
                            .fail_worker_handoff(
                                &context.command.command_id,
                                &error.to_string(),
                                true,
                            )
                            .await?;
                        return Err(error);
                    }
                };
                match self
                    .store
                    .finalize_worker_handoff(&context.command.command_id, runtime.clone())
                    .await
                {
                    Ok(handoff) => {
                        self.process_runtime_cleanup(&command_id).await;
                        Ok(handoff)
                    }
                    Err(error) => {
                        self.store
                            .quarantine_provisioning_runtime(&context.command.command_id, runtime)
                            .await?;
                        self.store
                            .fail_worker_handoff(
                                &context.command.command_id,
                                &error.to_string(),
                                true,
                            )
                            .await?;
                        Err(AllocationServiceError::RuntimeProvisionAmbiguous(
                            error.to_string(),
                        ))
                    }
                }
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                self.store
                    .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                    .await?;
                self.store
                    .fail_worker_handoff(&context.command.command_id, &message, true)
                    .await?;
                Err(AllocationServiceError::ObjectiveDeliveryFailed(message))
            }
            Err(RuntimeProvisionError::AfterPreparation {
                message,
                ambiguous,
                started_runtime,
            }) => {
                let ambiguous = if let Some(runtime) = started_runtime {
                    self.store
                        .quarantine_provisioning_runtime(&context.command.command_id, *runtime)
                        .await?;
                    true
                } else {
                    ambiguous
                };
                self.store
                    .fail_worker_handoff(&context.command.command_id, &message, ambiguous)
                    .await?;
                if ambiguous {
                    Err(AllocationServiceError::RuntimeProvisionAmbiguous(message))
                } else {
                    Err(AllocationServiceError::RuntimeProvision(message))
                }
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_worker_handoff(&context.command.command_id, &message, false)
                    .await?;
                Err(AllocationServiceError::RuntimeProvision(message))
            }
        }
    }

    async fn process_runtime_cleanup(&self, command_id: &str) {
        match self.cleanup.process_command(command_id).await {
            Ok(report) if report.failed > 0 => {
                tracing::warn!(
                    command_id,
                    failed = report.failed,
                    "Handoff committed with runtime cleanup pending"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(
                    command_id,
                    error = %error,
                    "Handoff committed but immediate runtime cleanup could not be processed"
                );
            }
        }
    }

    async fn verify_live_worker(
        &self,
        worker: &Worker,
    ) -> Result<WorkerRuntimeBinding, AllocationServiceError> {
        let runtime = worker.runtime.as_ref().ok_or_else(|| {
            AllocationServiceError::RuntimeBindingUnverified(
                "Yard worker has no runtime binding".to_owned(),
            )
        })?;
        if runtime.observation_state != RuntimeObservationState::Observed
            || runtime.process_state != RuntimeProcessState::Running
        {
            return Err(AllocationServiceError::RuntimeBindingUnverified(
                "Yard worker runtime is not observed and running".to_owned(),
            ));
        }
        let inventory = self.source.inventory(&runtime.session).await?;
        let observed = inventory
            .workers
            .iter()
            .find(|observed| observed.terminal_id == runtime.terminal_id)
            .ok_or_else(|| {
                AllocationServiceError::RuntimeBindingUnverified(
                    "Herdr no longer reports the selected live worker".to_owned(),
                )
            })?;
        if observed.workspace_id != runtime.workspace_id
            || observed.pane_id != runtime.pane_id
            || Some(observed.tab_id.as_str()) != runtime.tab_id.as_deref()
            || runtime
                .provider_session
                .as_ref()
                .is_some_and(|expected| observed.provider_session.as_ref() != Some(expected))
        {
            return Err(AllocationServiceError::RuntimeBindingUnverified(
                "Herdr worker topology or provider identity changed".to_owned(),
            ));
        }
        Ok(runtime.clone())
    }

    /// List the authoritative durable worker availability projection.
    ///
    /// # Errors
    ///
    /// Returns [`AllocationServiceError`] when storage cannot be read.
    pub async fn list_workers(&self) -> Result<WorkerCandidates, AllocationServiceError> {
        self.store
            .list_worker_candidates()
            .await
            .map_err(Into::into)
    }

    async fn verify_runtime_identity(
        &self,
        mut runtime: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, AllocationServiceError> {
        let inventory = self.source.inventory(&runtime.session).await?;
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == runtime.terminal_id)
            .ok_or_else(|| {
                AllocationServiceError::RuntimeBindingUnverified(
                    "Provisioned Herdr terminal is not present in the fresh inventory".to_owned(),
                )
            })?;
        if observed.workspace_id != runtime.workspace_id
            || observed.pane_id != runtime.pane_id
            || Some(observed.tab_id.as_str()) != runtime.tab_id.as_deref()
            || runtime
                .provider_session
                .as_ref()
                .is_some_and(|expected| observed.provider_session.as_ref() != Some(expected))
        {
            return Err(AllocationServiceError::RuntimeBindingUnverified(
                "Provisioned Herdr topology or provider identity changed before Yard could persist it"
                    .to_owned(),
            ));
        }
        if let Some(provider_session) = observed.provider_session.clone() {
            if runtime
                .provider_session
                .as_ref()
                .is_some_and(|expected| expected != &provider_session)
            {
                return Err(AllocationServiceError::RuntimeBindingUnverified(
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
        Ok(runtime)
    }

    /// List durable assignments for one project.
    ///
    /// # Errors
    ///
    /// Returns [`AllocationServiceError`] when the project is missing or
    /// storage cannot be read.
    pub async fn list_project(
        &self,
        project_id: &str,
    ) -> Result<Assignments, AllocationServiceError> {
        self.store
            .list_project_assignments(project_id)
            .await
            .map_err(Into::into)
    }

    /// Record explicit evidence-backed completion for the current assignment
    /// attempt without consulting runtime status.
    ///
    /// # Errors
    ///
    /// Returns [`AllocationServiceError`] for invalid, stale, conflicting, or
    /// missing assignments and persistence failures.
    pub async fn complete(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: RecordCompletionReceipt,
    ) -> Result<RecordedCompletionReceipt, AllocationServiceError> {
        self.store
            .record_completion_receipt(project_id, assignment_id, command)
            .await
            .map_err(Into::into)
    }
}

pub(crate) fn validate_supported_profile(profile: &WorkerProfile) -> Result<(), String> {
    if profile.spec.runtime_adapter != "herdr" {
        return Err(format!(
            "Runtime adapter '{}' is not supported for allocation",
            profile.spec.runtime_adapter
        ));
    }
    if !SUPPORTED_AGENT_KINDS.contains(&profile.spec.provider.as_str()) {
        return Err(format!(
            "Herdr agent kind '{}' is not supported",
            profile.spec.provider
        ));
    }
    if profile.spec.worktree_policy != "project_workspace" {
        return Err("Only the project_workspace worktree policy is supported".to_owned());
    }
    if profile.spec.sandbox_policy != "runtime_default" {
        return Err("This Yard slice only supports runtime_default sandbox policy".to_owned());
    }
    match profile.spec.permission_policy.as_str() {
        "runtime_default" | "yolo" => {}
        "auto" if profile.spec.provider == "claude" => {}
        "auto" => {
            return Err("The auto permission policy is only supported for Claude".to_owned());
        }
        policy => {
            return Err(format!("Permission policy '{policy}' is not supported"));
        }
    }
    if !profile.spec.tools.is_empty()
        || !profile.spec.skills.is_empty()
        || !profile.spec.mcp_servers.is_empty()
    {
        return Err("Herdr profile tool, skill, and MCP injection is not available yet".to_owned());
    }
    if profile.spec.completion_contract != "manual_receipt" {
        return Err("Only the manual_receipt completion contract is supported".to_owned());
    }
    Ok(())
}

pub(crate) fn provider_args(
    profile: &WorkerProfile,
) -> Result<Vec<String>, AllocationServiceError> {
    let mut args = Vec::new();
    if profile.spec.provider == "codex" {
        args.push("--no-alt-screen".to_owned());
    }
    match profile.spec.permission_policy.as_str() {
        "yolo" => match profile.spec.provider.as_str() {
            "codex" => args.push("--yolo".to_owned()),
            "claude" => args.push("--dangerously-skip-permissions".to_owned()),
            provider => {
                return Err(AllocationServiceError::UnsupportedProfile(format!(
                    "The yolo permission policy is not mapped for Herdr agent kind '{provider}'"
                )));
            }
        },
        "auto" if profile.spec.provider == "claude" => {
            args.push("--permission-mode".to_owned());
            args.push("auto".to_owned());
        }
        "auto" => {
            return Err(AllocationServiceError::UnsupportedProfile(
                "The auto permission policy is only supported for Claude".to_owned(),
            ));
        }
        _ => {}
    }
    if let Some(model) = profile.spec.model.as_ref() {
        match profile.spec.provider.as_str() {
            "codex" => {
                args.push("-m".to_owned());
                args.push(model.clone());
            }
            "claude" => {
                args.push("--model".to_owned());
                args.push(model.clone());
            }
            provider => {
                return Err(AllocationServiceError::UnsupportedProfile(format!(
                    "Model selection is not mapped for Herdr agent kind '{provider}'"
                )));
            }
        }
    }
    Ok(args)
}

pub(crate) fn agent_name(command_id: &str) -> String {
    herdr_agent_name(command_id)
}

pub(crate) fn assignment_prompt(objective: &str, role: &str, profile: &WorkerProfile) -> String {
    let instructions = profile
        .spec
        .instructions_ref
        .as_ref()
        .map_or_else(String::new, |reference| {
            format!("\nInstructions: Follow {reference}.")
        });
    format!(
        "Yard assignment\nRole: {role}\nObjective: {objective}{instructions}\n\
         Completion: Do not infer completion from runtime status. A manual Yard receipt is required."
    )
}

#[cfg(test)]
mod tests {
    use yard_domain::{WorkerProfile, WorkerProfileSpec};

    use super::{AllocationServiceError, provider_args, validate_supported_profile};

    fn profile(provider: &str, permission_policy: &str, model: Option<&str>) -> WorkerProfile {
        WorkerProfile {
            id: "profile-1".to_owned(),
            spec: WorkerProfileSpec {
                name: "Implementer".to_owned(),
                runtime_adapter: "herdr".to_owned(),
                provider: provider.to_owned(),
                model: model.map(str::to_owned),
                default_role: "implementer".to_owned(),
                instructions_ref: None,
                tools: Vec::new(),
                skills: Vec::new(),
                mcp_servers: Vec::new(),
                sandbox_policy: "runtime_default".to_owned(),
                worktree_policy: "project_workspace".to_owned(),
                permission_policy: permission_policy.to_owned(),
                completion_contract: "manual_receipt".to_owned(),
            },
            version: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn provider_args_map_explicit_permissions_and_models() {
        let codex = provider_args(&profile("codex", "yolo", Some("gpt-5.4"))).unwrap();
        let claude = provider_args(&profile("claude", "yolo", Some("sonnet"))).unwrap();
        let claude_auto = provider_args(&profile("claude", "auto", None)).unwrap();
        let runtime_default = provider_args(&profile("codex", "runtime_default", None)).unwrap();

        assert_eq!(codex, ["--no-alt-screen", "--yolo", "-m", "gpt-5.4"]);
        assert_eq!(
            claude,
            ["--dangerously-skip-permissions", "--model", "sonnet"]
        );
        assert_eq!(claude_auto, ["--permission-mode", "auto"]);
        assert_eq!(runtime_default, ["--no-alt-screen"]);
    }

    #[test]
    fn provider_args_reject_unmapped_yolo_permissions() {
        let error = provider_args(&profile("kiro", "yolo", None)).unwrap_err();

        assert!(matches!(
            error,
            AllocationServiceError::UnsupportedProfile(message)
                if message.contains("not mapped")
        ));
    }

    #[test]
    fn auto_permission_policy_is_claude_only() {
        assert!(validate_supported_profile(&profile("claude", "auto", None)).is_ok());
        assert!(validate_supported_profile(&profile("codex", "runtime_default", None)).is_ok());
        assert!(validate_supported_profile(&profile("codex", "yolo", None)).is_ok());

        let validation = validate_supported_profile(&profile("codex", "auto", None)).unwrap_err();
        let mapping = provider_args(&profile("codex", "auto", None)).unwrap_err();

        assert!(validation.contains("only supported for Claude"));
        assert!(matches!(
            mapping,
            AllocationServiceError::UnsupportedProfile(message)
                if message.contains("only supported for Claude")
        ));
    }
}

const SUPPORTED_AGENT_KINDS: &[&str] = &[
    "pi",
    "claude",
    "codex",
    "gemini",
    "cursor",
    "devin",
    "agy",
    "cline",
    "omp",
    "mastracode",
    "opencode",
    "copilot",
    "kimi",
    "kiro",
    "droid",
    "amp",
    "grok",
    "hermes",
    "kilo",
    "qodercli",
    "maki",
];

#[derive(Debug, Error)]
pub enum AllocationServiceError {
    #[error(transparent)]
    InvalidAssignment(#[from] yard_domain::AssignmentValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    RuntimeIntervention(#[from] RuntimeInterventionError),
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("bound Herdr workspace '{workspace_id}' is not currently observed")]
    RuntimeWorkspaceMissing { workspace_id: String },
    #[error("Herdr did not report a working directory for the project workspace")]
    RuntimeCwdUnavailable,
    #[error("Herdr worker provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("Herdr worker provisioning outcome is ambiguous: {0}")]
    RuntimeProvisionAmbiguous(String),
    #[error("Herdr worker binding could not be verified: {0}")]
    RuntimeBindingUnverified(String),
    #[error("worker was created but assignment delivery failed: {0}")]
    ObjectiveDeliveryFailed(String),
}

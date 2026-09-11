use std::{collections::HashSet, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::Mutex,
    time::{Instant, sleep},
};
use tracing::warn;
use yard_domain::{
    ConfigureYardOrchestrator, ConfiguredYardOrchestrator, ObservedWorker,
    OrchestratorWorkflowProfile, ProvisionYardOrchestrator, RecoverYardOrchestrator,
    RecoveredYardOrchestrator, RuntimeInventory, RuntimeObservationState, RuntimeProcessState,
    SendYardOrchestratorPrompt, WorkerAvailability, WorkerRuntimeBinding,
    YARD_STANDARD_ORCHESTRATOR_PROFILE_ID, YardOrchestrator,
};
use yard_store::{
    BeginYardOrchestratorPrompt, ProjectStoreError, TokenSpendCommandSource, YardStore,
};

use crate::{
    allocation_service::{
        AllocationServiceError, RuntimeControl, RuntimeProvisionError, RuntimeRetirementError,
        RuntimeRetirementRequest, RuntimeSessionRequest, RuntimeWorkerRestartRequest,
        RuntimeWorkspaceProvisionRequest, assignment_prompt, provider_args,
        validate_supported_profile,
    },
    intervention_service::{RuntimeIntervention, RuntimeInterventionError, RuntimePromptRequest},
    reconciliation_service::{ReconciliationService, ReconciliationServiceError},
    status_protocol::{
        validate_executable_orchestrator_workflow, with_orchestrator_status_contract,
        with_orchestrator_workflow,
    },
};

pub const YARD_ORCHESTRATOR_SESSION: &str = "yard-orchestrator";
pub const YARD_ORCHESTRATOR_WORKSPACE_LABEL: &str = "Yard central coordination";
pub const YARD_ORCHESTRATOR_AGENT_NAME: &str = "yard-orchestrator";

const RUNTIME_IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);
const CENTRAL_COORDINATION_OBJECTIVE: &str = "Coordinate all Yard projects from this dedicated \
    central session. Maintain portfolio-wide priorities, route work to project orchestrators, \
    surface blockers, and leave project-scoped implementation to those project teams.";
const RECOVERY_PROMPT: &str = "Resume central Yard orchestration after runtime recovery. Reconcile \
    the current fleet before advancing work.";

#[derive(Clone)]
pub struct YardOrchestratorService {
    runtime: Arc<dyn RuntimeControl>,
    intervention: Arc<dyn RuntimeIntervention>,
    store: Arc<dyn YardStore>,
    reconciliation: ReconciliationService,
    operation: Arc<Mutex<()>>,
    cwd: String,
}

enum RecoveryDelivery {
    Replayed,
    Prompt {
        restored: ObservedWorker,
        text: String,
    },
    Restarted {
        restored: ObservedWorker,
    },
}

impl YardOrchestratorService {
    /// Build the singleton provisioner with a backend-resolved stable local
    /// CWD. The same absolute path is used for the Herdr server and workspace.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn RuntimeControl>,
        intervention: Arc<dyn RuntimeIntervention>,
        store: Arc<dyn YardStore>,
        reconciliation: ReconciliationService,
        cwd: String,
    ) -> Self {
        Self {
            runtime,
            intervention,
            store,
            reconciliation,
            operation: Arc::default(),
            cwd,
        }
    }

    /// Ensure, adopt, and configure the dedicated Yard orchestrator.
    ///
    /// Calls are serialized in-process. Every attempt snapshots and
    /// reconciles the deterministic Herdr session before deciding whether a
    /// new workspace is needed.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorServiceError`] for invalid or stale commands,
    /// unsupported profiles, Herdr failures, missing reconciled identities, or
    /// persistence failures.
    pub async fn provision(
        &self,
        command: ProvisionYardOrchestrator,
    ) -> Result<ConfiguredYardOrchestrator, YardOrchestratorServiceError> {
        let command = command.normalize()?;
        let _operation = self.operation.lock().await;

        if let Some(configured) = self.replay_provision(&command).await? {
            return Ok(configured);
        }

        let profile = self.store.get_worker_profile(&command.profile_id).await?;
        if profile.version != command.expected_profile_version {
            return Err(ProjectStoreError::ProfileVersionConflict {
                current_version: profile.version,
            }
            .into());
        }
        let args = validated_provider_args(&profile)?;
        let active_workflow = self.store.get_orchestrator_workflow_profile().await?;
        validate_executable_orchestrator_workflow(&active_workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;

        self.runtime
            .ensure_session(RuntimeSessionRequest {
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                startup_cwd: self.cwd.clone(),
            })
            .await
            .map_err(runtime_error)?;
        let current = self.store.get_yard_orchestrator().await?;
        let (inventory, _) = self
            .reconciliation
            .refresh(YARD_ORCHESTRATOR_SESSION)
            .await?;
        let mut observed = dedicated_worker(&inventory, current.worker.as_ref());
        let bootstrapped = observed.is_none();
        self.store
            .begin_yard_orchestrator_runtime_provision(command.clone())
            .await?;

        if bootstrapped {
            observed = Some(
                self.bootstrap_dedicated_worker(&command, &profile, args, &active_workflow)
                    .await?,
            );
        }

        let observed = observed.ok_or(YardOrchestratorServiceError::RuntimeBindingUnverified)?;
        let candidates = self.store.list_worker_candidates().await?;
        let candidate = candidates
            .workers
            .into_iter()
            .find(|candidate| {
                candidate.worker.runtime.as_ref().is_some_and(|runtime| {
                    runtime.adapter == "herdr"
                        && runtime.session == YARD_ORCHESTRATOR_SESSION
                        && runtime.terminal_id == observed.terminal_id
                })
            })
            .ok_or(YardOrchestratorServiceError::ReconciledWorkerMissing)?;
        if !matches!(
            candidate.availability,
            WorkerAvailability::UnassignedLive | WorkerAvailability::YardOrchestrator
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
        let workflow_profile_version =
            current
                .worker
                .as_ref()
                .map_or(active_workflow.version, |current_worker| {
                    if current_worker.id == worker.id {
                        current.workflow_profile_version
                    } else {
                        active_workflow.version
                    }
                });
        let configured = self
            .store
            .configure_yard_orchestrator(ConfigureYardOrchestrator {
                command_id: command.command_id.clone(),
                actor: command.actor.clone(),
                worker_id: worker.id,
                expected_worker_version: worker.version,
                expected_orchestrator_version: command.expected_orchestrator_version,
                workflow_profile_version: Some(workflow_profile_version),
            })
            .await?;
        let workflow = self
            .workflow_revision(configured.orchestrator.workflow_profile_version)
            .await?;
        self.deliver_provision_adoption_workflow(&command, &configured, &workflow, bootstrapped)
            .await?;
        Ok(configured)
    }

    async fn workflow_revision(
        &self,
        version: u64,
    ) -> Result<OrchestratorWorkflowProfile, YardOrchestratorServiceError> {
        let workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                version,
            )
            .await
            .map_err(YardOrchestratorServiceError::from)?;
        validate_executable_orchestrator_workflow(&workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        Ok(workflow)
    }

    async fn replay_provision(
        &self,
        command: &ProvisionYardOrchestrator,
    ) -> Result<Option<ConfiguredYardOrchestrator>, YardOrchestratorServiceError> {
        let Some(replayed) = self
            .store
            .replay_yard_orchestrator_provision(command.clone())
            .await?
        else {
            return Ok(None);
        };
        self.workflow_revision(replayed.orchestrator.workflow_profile_version)
            .await?;
        self.runtime
            .ensure_session(RuntimeSessionRequest {
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                startup_cwd: self.cwd.clone(),
            })
            .await
            .map_err(runtime_error)?;
        self.reconciliation
            .refresh(YARD_ORCHESTRATOR_SESSION)
            .await?;
        let configured = self
            .store
            .replay_yard_orchestrator_provision(command.clone())
            .await?
            .ok_or(ProjectStoreError::CommandNotFound)?;
        let current = self.store.get_yard_orchestrator().await?;
        if current.version != configured.orchestrator.version
            || current.worker.as_ref().map(|worker| &worker.id)
                != configured
                    .orchestrator
                    .worker
                    .as_ref()
                    .map(|worker| &worker.id)
        {
            return Ok(Some(configured));
        }
        let workflow = self
            .workflow_revision(configured.orchestrator.workflow_profile_version)
            .await?;
        self.deliver_provision_adoption_workflow(command, &configured, &workflow, false)
            .await?;
        Ok(Some(configured))
    }

    /// Configure or replace the Yard orchestrator and deliver its pinned workflow.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorServiceError`] when ownership is stale or
    /// the configured runtime cannot receive its workflow.
    pub async fn configure(
        &self,
        mut command: ConfigureYardOrchestrator,
    ) -> Result<ConfiguredYardOrchestrator, YardOrchestratorServiceError> {
        command = command.normalize()?;
        let _operation = self.operation.lock().await;
        if let Some(replayed) = self
            .store
            .replay_yard_orchestrator_configuration(command.clone())
            .await?
        {
            return Ok(replayed);
        }
        let current = self.store.get_yard_orchestrator().await?;
        let active_workflow = self.store.get_orchestrator_workflow_profile().await?;
        let replacing = current
            .worker
            .as_ref()
            .is_none_or(|worker| worker.id != command.worker_id);
        let workflow_profile_version = if replacing {
            active_workflow.version
        } else {
            current.workflow_profile_version
        };
        let workflow = self.workflow_revision(workflow_profile_version).await?;
        command.workflow_profile_version = Some(workflow_profile_version);
        let configured = self
            .store
            .configure_yard_orchestrator(command.clone())
            .await?;
        // Ownership commits before external delivery. A changed result must
        // retry delivery on replay with the same command identity.
        if configured.orchestrator.version != command.expected_orchestrator_version {
            let runtime = configured
                .orchestrator
                .worker
                .as_ref()
                .and_then(|worker| worker.runtime.as_ref())
                .ok_or(YardOrchestratorServiceError::RuntimeBindingUnverified)?;
            self.deliver_lifecycle_prompt(
                &command.command_id,
                runtime,
                &workflow,
                "Assume ownership of central Yard orchestration. Apply the pinned workflow to \
                 every subsequent Yard command.",
            )
            .await?;
        }
        Ok(configured)
    }

    /// Restart the dedicated Herdr session, restart an exited agent in its
    /// retained shell pane when necessary, and restore the existing Yard
    /// orchestrator binding without provisioning or changing ownership.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorServiceError`] when the command is stale, the
    /// current orchestrator is not dedicated, Herdr cannot restart, or the
    /// configured worker cannot be restored unambiguously.
    pub async fn recover(
        &self,
        command: RecoverYardOrchestrator,
    ) -> Result<RecoveredYardOrchestrator, YardOrchestratorServiceError> {
        let command = command.normalize()?;
        let _operation = self.operation.lock().await;
        let (current, workflow) = self.recovery_context(&command).await?;
        let (current_worker, current_runtime) = dedicated_recovery_binding(&current)?;

        let delivery = self
            .restore_or_restart_dedicated_worker(
                &command,
                current_worker,
                current_runtime,
                &workflow,
            )
            .await?;
        if matches!(delivery, RecoveryDelivery::Replayed) {
            return Ok(RecoveredYardOrchestrator {
                command_id: command.command_id,
                orchestrator: self.store.get_yard_orchestrator().await?,
            });
        }
        let restored = match &delivery {
            RecoveryDelivery::Prompt { restored, .. }
            | RecoveryDelivery::Restarted { restored } => restored.clone(),
            RecoveryDelivery::Replayed => unreachable!(),
        };
        let (orchestrator, runtime) = match self
            .verified_recovered_orchestrator(current_worker, current_runtime, &restored)
            .await
        {
            Ok(verified) => verified,
            Err(error) => {
                self.store
                    .fail_yard_orchestrator_prompt(&command.command_id, &error.to_string(), true)
                    .await?;
                return Err(error);
            }
        };

        let runtime_status = match delivery {
            RecoveryDelivery::Prompt { text, .. } => {
                let result = self
                    .intervention
                    .prompt(RuntimePromptRequest {
                        command_id: command.command_id.clone(),
                        session: runtime.session.clone(),
                        pane_id: runtime.pane_id.clone(),
                        text,
                    })
                    .await;
                match result {
                    Ok(result) => {
                        if let Err(error) = self
                            .verified_recovered_orchestrator(
                                current_worker,
                                current_runtime,
                                &restored,
                            )
                            .await
                        {
                            self.store
                                .fail_yard_orchestrator_prompt(
                                    &command.command_id,
                                    &error.to_string(),
                                    true,
                                )
                                .await?;
                            return Err(error);
                        }
                        result.status
                    }
                    Err(error) => {
                        let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                        self.store
                            .fail_yard_orchestrator_prompt(
                                &command.command_id,
                                &error.to_string(),
                                ambiguous,
                            )
                            .await?;
                        return Err(YardOrchestratorServiceError::ObjectiveDeliveryFailed(
                            error.to_string(),
                        ));
                    }
                }
            }
            RecoveryDelivery::Restarted { .. } => "submitted".to_owned(),
            RecoveryDelivery::Replayed => unreachable!(),
        };
        self.store
            .succeed_yard_orchestrator_prompt(&command.command_id, &runtime_status)
            .await?;

        Ok(RecoveredYardOrchestrator {
            command_id: command.command_id,
            orchestrator,
        })
    }

    async fn recovery_context(
        &self,
        command: &RecoverYardOrchestrator,
    ) -> Result<(YardOrchestrator, OrchestratorWorkflowProfile), YardOrchestratorServiceError> {
        let current = self.store.get_yard_orchestrator().await?;
        if current.version != command.expected_orchestrator_version {
            return Err(ProjectStoreError::YardOrchestratorVersionConflict {
                current_version: current.version,
            }
            .into());
        }
        dedicated_recovery_binding(&current)?;
        let workflow = self
            .workflow_revision(current.workflow_profile_version)
            .await?;
        self.runtime
            .ensure_session(RuntimeSessionRequest {
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                startup_cwd: self.cwd.clone(),
            })
            .await
            .map_err(runtime_error)?;
        Ok((current, workflow))
    }

    async fn restore_or_restart_dedicated_worker(
        &self,
        command: &RecoverYardOrchestrator,
        current_worker: &yard_domain::Worker,
        current_runtime: &WorkerRuntimeBinding,
        workflow: &OrchestratorWorkflowProfile,
    ) -> Result<RecoveryDelivery, YardOrchestratorServiceError> {
        let (inventory, _) = self
            .reconciliation
            .refresh(YARD_ORCHESTRATOR_SESSION)
            .await?;
        if let Some(restored) = dedicated_worker(&inventory, Some(current_worker)) {
            if self
                .begin_recovery_prompt(command, current_worker, RECOVERY_PROMPT.to_owned())
                .await?
            {
                return Ok(RecoveryDelivery::Replayed);
            }
            let text = lifecycle_prompt_text(&command.command_id, workflow, RECOVERY_PROMPT)?;
            return Ok(RecoveryDelivery::Prompt { restored, text });
        }
        if pending_dedicated_worker(&inventory, current_worker) {
            return Err(YardOrchestratorServiceError::RecoveryLaunchPending);
        }
        if !retained_dedicated_pane(&inventory, current_runtime) {
            return Err(YardOrchestratorServiceError::RecoveryBindingMissing);
        }

        let profile_id = current_worker
            .profile_id
            .as_deref()
            .ok_or(YardOrchestratorServiceError::RecoveryProfileMissing)?;
        let profile_version = current_worker
            .profile_version
            .ok_or(YardOrchestratorServiceError::RecoveryProfileMissing)?;
        let profile = self
            .store
            .get_worker_profile_revision(profile_id, profile_version)
            .await?;
        let args = validated_provider_args(&profile)?;
        let prompt = format!(
            "{}\n\n{RECOVERY_PROMPT}",
            assignment_prompt(
                CENTRAL_COORDINATION_OBJECTIVE,
                "central orchestrator",
                &profile,
            )
        );
        if self
            .begin_recovery_prompt(command, current_worker, RECOVERY_PROMPT.to_owned())
            .await?
        {
            return Ok(RecoveryDelivery::Replayed);
        }
        let prompt = lifecycle_prompt_text(&command.command_id, workflow, &prompt)?;
        if let Err(error) = self
            .runtime
            .restart_worker(RuntimeWorkerRestartRequest {
                command_id: command.command_id.clone(),
                runtime: current_runtime.clone(),
                agent_name: YARD_ORCHESTRATOR_AGENT_NAME.to_owned(),
                kind: profile.spec.provider,
                args,
                prompt,
            })
            .await
        {
            let ambiguous = runtime_restart_outcome_ambiguous(&error);
            self.store
                .fail_yard_orchestrator_prompt(&command.command_id, &error.to_string(), ambiguous)
                .await?;
            return Err(runtime_error(error));
        }

        let refresh = self.reconciliation.refresh(YARD_ORCHESTRATOR_SESSION).await;
        let (inventory, _) = match refresh {
            Ok(inventory) => inventory,
            Err(error) => {
                self.store
                    .fail_yard_orchestrator_prompt(&command.command_id, &error.to_string(), true)
                    .await?;
                return Err(error.into());
            }
        };
        if let Some(restored) = dedicated_worker(&inventory, Some(current_worker)) {
            return Ok(RecoveryDelivery::Restarted { restored });
        }
        let error = if pending_dedicated_worker(&inventory, current_worker) {
            YardOrchestratorServiceError::RecoveryLaunchPending
        } else {
            YardOrchestratorServiceError::RecoveryBindingMissing
        };
        self.store
            .fail_yard_orchestrator_prompt(&command.command_id, &error.to_string(), true)
            .await?;
        Err(error)
    }

    async fn begin_recovery_prompt(
        &self,
        command: &RecoverYardOrchestrator,
        current_worker: &yard_domain::Worker,
        text: String,
    ) -> Result<bool, YardOrchestratorServiceError> {
        let result = self
            .store
            .begin_yard_orchestrator_prompt(
                SendYardOrchestratorPrompt {
                    command_id: command.command_id.clone(),
                    actor: command.actor.clone(),
                    expected_orchestrator_version: command.expected_orchestrator_version,
                    orchestrator_worker_id: current_worker.id.clone(),
                    text,
                },
                TokenSpendCommandSource::Manual,
            )
            .await?;
        Ok(matches!(result, BeginYardOrchestratorPrompt::Replayed(_)))
    }

    async fn verified_recovered_orchestrator(
        &self,
        current_worker: &yard_domain::Worker,
        current_runtime: &WorkerRuntimeBinding,
        restored: &ObservedWorker,
    ) -> Result<(YardOrchestrator, WorkerRuntimeBinding), YardOrchestratorServiceError> {
        if current_runtime
            .provider_session
            .as_ref()
            .is_some_and(|expected| restored.provider_session.as_ref() != Some(expected))
        {
            return Err(YardOrchestratorServiceError::RecoveryBindingAmbiguous);
        }
        let orchestrator = self.store.get_yard_orchestrator().await?;
        let worker = orchestrator
            .worker
            .as_ref()
            .filter(|worker| worker.id == current_worker.id)
            .ok_or(YardOrchestratorServiceError::RecoveryOwnershipChanged)?;
        let runtime = worker
            .runtime
            .as_ref()
            .filter(|runtime| {
                runtime.session == YARD_ORCHESTRATOR_SESSION
                    && runtime.terminal_id == restored.terminal_id
                    && runtime.observation_state == RuntimeObservationState::Observed
                    && runtime.process_state == RuntimeProcessState::Running
            })
            .ok_or(YardOrchestratorServiceError::RecoveryBindingMissing)?
            .clone();
        if current_runtime
            .provider_session
            .as_ref()
            .is_some_and(|expected| runtime.provider_session.as_ref() != Some(expected))
        {
            return Err(YardOrchestratorServiceError::RecoveryBindingAmbiguous);
        }
        Ok((orchestrator, runtime))
    }

    /// Start a fresh worker under the reserved `yard-orchestrator` name and
    /// wait for it to reconcile.
    ///
    /// A successful bootstrap (or one that fails only after the agent
    /// claimed the reserved name, i.e. `PromptDelivery`) leaves a live
    /// Herdr worker holding that globally-unique name. If it cannot be
    /// durably adopted here, it is explicitly retired before returning the
    /// error — otherwise it becomes a permanent orphan that blocks every
    /// future provisioning attempt with `agent_name_taken` until a human
    /// manually finds and stops it in Herdr.
    async fn bootstrap_dedicated_worker(
        &self,
        command: &ProvisionYardOrchestrator,
        profile: &yard_domain::WorkerProfile,
        args: Vec<String>,
        workflow: &OrchestratorWorkflowProfile,
    ) -> Result<ObservedWorker, YardOrchestratorServiceError> {
        let provision = RuntimeWorkspaceProvisionRequest {
            command_id: command.command_id.clone(),
            session: YARD_ORCHESTRATOR_SESSION.to_owned(),
            workspace_label: YARD_ORCHESTRATOR_WORKSPACE_LABEL.to_owned(),
            cwd: self.cwd.clone(),
            agent_name: YARD_ORCHESTRATOR_AGENT_NAME.to_owned(),
            kind: profile.spec.provider.clone(),
            args,
            prompt: with_orchestrator_status_contract(
                &with_orchestrator_workflow(
                    &assignment_prompt(
                        CENTRAL_COORDINATION_OBJECTIVE,
                        "central orchestrator",
                        profile,
                    ),
                    workflow,
                )
                .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?,
                &command.command_id,
            ),
        };
        let prepared = match self
            .runtime
            .prepare_workspace_worker(provision.clone())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => return self.fail_dedicated_preparation(command, error).await,
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
            return Err(YardOrchestratorServiceError::RuntimeProvisionAmbiguous(
                error.to_string(),
            ));
        }
        let runtime = match self
            .runtime
            .start_prepared_workspace_worker(provision, prepared)
            .await
        {
            Ok(runtime) => runtime,
            Err(error) => return self.fail_dedicated_start(command, error).await,
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
            return Err(YardOrchestratorServiceError::RuntimeProvisionAmbiguous(
                error.to_string(),
            ));
        }
        match self.reconcile_provisioned_worker(&runtime).await {
            Ok(worker) => Ok(worker),
            Err(error) => {
                self.store
                    .quarantine_provisioning_runtime(&command.command_id, runtime.clone())
                    .await?;
                self.store
                    .fail_dedicated_runtime_provision(&command.command_id, &error.to_string(), true)
                    .await?;
                self.retire_orphaned_runtime(runtime).await;
                Err(error)
            }
        }
    }

    async fn fail_dedicated_preparation(
        &self,
        command: &ProvisionYardOrchestrator,
        error: RuntimeProvisionError,
    ) -> Result<ObservedWorker, YardOrchestratorServiceError> {
        let (message, ambiguous, runtime, objective_delivery) = provisioning_failure(error);
        if let Some(runtime) = runtime {
            self.store
                .quarantine_provisioning_runtime(&command.command_id, runtime)
                .await?;
        }
        self.store
            .fail_dedicated_runtime_provision(&command.command_id, &message, ambiguous)
            .await?;
        if objective_delivery {
            Err(YardOrchestratorServiceError::ObjectiveDeliveryFailed(
                message,
            ))
        } else if ambiguous {
            Err(YardOrchestratorServiceError::RuntimeProvisionAmbiguous(
                message,
            ))
        } else {
            Err(YardOrchestratorServiceError::RuntimeProvision(message))
        }
    }

    async fn fail_dedicated_start(
        &self,
        command: &ProvisionYardOrchestrator,
        error: RuntimeProvisionError,
    ) -> Result<ObservedWorker, YardOrchestratorServiceError> {
        let (message, mut ambiguous, runtime, objective_delivery) = provisioning_failure(error);
        if let Some(runtime) = runtime {
            ambiguous = true;
            self.store
                .quarantine_provisioning_runtime(&command.command_id, runtime)
                .await?;
        } else if !ambiguous {
            self.store
                .release_provisioning_runtime_claim(&command.command_id)
                .await?;
        }
        self.store
            .fail_dedicated_runtime_provision(&command.command_id, &message, ambiguous)
            .await?;
        if objective_delivery {
            Err(YardOrchestratorServiceError::ObjectiveDeliveryFailed(
                message,
            ))
        } else if ambiguous {
            Err(YardOrchestratorServiceError::RuntimeProvisionAmbiguous(
                message,
            ))
        } else {
            Err(YardOrchestratorServiceError::RuntimeProvision(message))
        }
    }

    async fn deliver_lifecycle_prompt(
        &self,
        command_id: &str,
        runtime: &WorkerRuntimeBinding,
        workflow: &OrchestratorWorkflowProfile,
        prompt: &str,
    ) -> Result<(), YardOrchestratorServiceError> {
        let text = lifecycle_prompt_text(command_id, workflow, prompt)?;
        self.intervention
            .prompt(RuntimePromptRequest {
                command_id: command_id.to_owned(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text,
            })
            .await
            .map(|_| ())
            .map_err(|error| {
                YardOrchestratorServiceError::ObjectiveDeliveryFailed(error.to_string())
            })
    }

    async fn deliver_provision_adoption_workflow(
        &self,
        command: &ProvisionYardOrchestrator,
        configured: &ConfiguredYardOrchestrator,
        workflow: &OrchestratorWorkflowProfile,
        bootstrapped: bool,
    ) -> Result<(), YardOrchestratorServiceError> {
        // A replay can be recovering a rejected post-commit adoption prompt.
        if bootstrapped || configured.orchestrator.version == command.expected_orchestrator_version
        {
            return Ok(());
        }
        let runtime = configured
            .orchestrator
            .worker
            .as_ref()
            .and_then(|worker| worker.runtime.as_ref())
            .ok_or(YardOrchestratorServiceError::RuntimeBindingUnverified)?;
        self.deliver_lifecycle_prompt(
            &command.command_id,
            runtime,
            workflow,
            "Assume ownership of central Yard orchestration. Apply the pinned workflow to every \
             subsequent Yard command.",
        )
        .await
    }

    async fn reconcile_provisioned_worker(
        &self,
        runtime: &WorkerRuntimeBinding,
    ) -> Result<ObservedWorker, YardOrchestratorServiceError> {
        let deadline = Instant::now() + RUNTIME_IDENTITY_TIMEOUT;
        loop {
            let (inventory, _) = self
                .reconciliation
                .refresh(YARD_ORCHESTRATOR_SESSION)
                .await?;
            let workspace_matches = inventory.workspaces.iter().any(|workspace| {
                workspace.runtime_id == runtime.workspace_id
                    && workspace.label == YARD_ORCHESTRATOR_WORKSPACE_LABEL
            });
            if inventory.adapter == runtime.adapter
                && inventory.session == runtime.session
                && workspace_matches
                && let Some(worker) = inventory.workers.into_iter().find(|worker| {
                    worker.name.as_deref() == Some(YARD_ORCHESTRATOR_AGENT_NAME)
                        && worker.workspace_id == runtime.workspace_id
                        && worker.terminal_id == runtime.terminal_id
                        && Some(worker.tab_id.as_str()) == runtime.tab_id.as_deref()
                        && worker.pane_id == runtime.pane_id
                        && worker.interactive_ready
                        && runtime.provider_session.as_ref().is_none_or(|expected| {
                            worker.provider_session.as_ref() == Some(expected)
                        })
                })
            {
                return Ok(worker);
            }
            if Instant::now() >= deadline {
                return Err(YardOrchestratorServiceError::RuntimeBindingUnverified);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    /// Best-effort retirement of a Herdr runtime that claimed the reserved
    /// `yard-orchestrator` agent name but that Yard could not durably adopt
    /// (initial prompt delivery failed, or the runtime never reconciled
    /// within [`RUNTIME_IDENTITY_TIMEOUT`]). Without this, the runtime keeps
    /// holding the name and every subsequent provisioning attempt fails with
    /// Herdr's `agent_name_taken` until a human manually finds and stops it.
    ///
    /// Retirement failures are only logged: the caller already has a
    /// provisioning error to return, and retirement can be retried by hand
    /// (or by a future provisioning attempt) if this best-effort call fails.
    async fn retire_orphaned_runtime(&self, runtime: WorkerRuntimeBinding) {
        let request = RuntimeRetirementRequest {
            cleanup_id: format!("yard-orchestrator-orphan-{}", runtime.terminal_id),
            adapter: runtime.adapter,
            session: runtime.session,
            workspace_id: runtime.workspace_id,
            terminal_id: runtime.terminal_id,
            tab_id: runtime.tab_id,
            pane_id: runtime.pane_id,
            provider_session: runtime.provider_session,
            owns_tab: runtime.owns_tab,
        };
        let deadline = Instant::now() + RUNTIME_IDENTITY_TIMEOUT;
        loop {
            match self.runtime.retire_runtime(request.clone()).await {
                Ok(()) => break,
                Err(error)
                    if !matches!(
                        &error,
                        RuntimeRetirementError::AtomicIdentityGuardUnavailable
                    ) && Instant::now() < deadline =>
                {
                    sleep(Duration::from_millis(100)).await;
                }
                Err(error) => {
                    warn!(
                        error = %error,
                        "Failed to retire an orphaned Yard orchestrator runtime; the \
                         reserved yard-orchestrator agent name may remain blocked \
                         until it is retired manually"
                    );
                    break;
                }
            }
        }
    }
}

fn dedicated_worker(
    inventory: &RuntimeInventory,
    current: Option<&yard_domain::Worker>,
) -> Option<ObservedWorker> {
    let dedicated_workspaces = inventory
        .workspaces
        .iter()
        .filter(|workspace| workspace.label == YARD_ORCHESTRATOR_WORKSPACE_LABEL)
        .map(|workspace| workspace.runtime_id.as_str())
        .collect::<HashSet<_>>();
    let current_terminal = current
        .and_then(|worker| worker.runtime.as_ref())
        .filter(|runtime| runtime.session == YARD_ORCHESTRATOR_SESSION)
        .map(|runtime| runtime.terminal_id.as_str());
    let mut workers = inventory
        .workers
        .iter()
        .filter(|worker| {
            worker.name.as_deref() == Some(YARD_ORCHESTRATOR_AGENT_NAME)
                && dedicated_workspaces.contains(worker.workspace_id.as_str())
                && worker.interactive_ready
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

fn pending_dedicated_worker(inventory: &RuntimeInventory, current: &yard_domain::Worker) -> bool {
    let Some(runtime) = current
        .runtime
        .as_ref()
        .filter(|runtime| runtime.session == YARD_ORCHESTRATOR_SESSION)
    else {
        return false;
    };
    let workspace_is_dedicated = inventory.workspaces.iter().any(|workspace| {
        workspace.runtime_id == runtime.workspace_id
            && workspace.label == YARD_ORCHESTRATOR_WORKSPACE_LABEL
    });
    workspace_is_dedicated
        && inventory.workers.iter().any(|worker| {
            worker.name.as_deref() == Some(YARD_ORCHESTRATOR_AGENT_NAME)
                && worker.workspace_id == runtime.workspace_id
                && worker.terminal_id == runtime.terminal_id
                && Some(worker.tab_id.as_str()) == runtime.tab_id.as_deref()
                && worker.pane_id == runtime.pane_id
                && worker.launch_pending
                && !worker.interactive_ready
        })
}

fn retained_dedicated_pane(inventory: &RuntimeInventory, runtime: &WorkerRuntimeBinding) -> bool {
    if inventory.adapter != runtime.adapter
        || inventory.session != runtime.session
        || runtime.session != YARD_ORCHESTRATOR_SESSION
        || runtime.tab_id.is_none()
    {
        return false;
    }
    let workspace_is_dedicated = inventory.workspaces.iter().any(|workspace| {
        workspace.runtime_id == runtime.workspace_id
            && workspace.label == YARD_ORCHESTRATOR_WORKSPACE_LABEL
    });
    let pane_is_retained = inventory.panes.iter().any(|pane| {
        pane.runtime_id == runtime.pane_id
            && pane.terminal_id == runtime.terminal_id
            && pane.workspace_id == runtime.workspace_id
            && Some(pane.tab_id.as_str()) == runtime.tab_id.as_deref()
            && pane.provider.is_none()
            && pane.provider_session.is_none()
    });
    let pane_has_agent = inventory.workers.iter().any(|worker| {
        worker.pane_id == runtime.pane_id || worker.terminal_id == runtime.terminal_id
    });
    workspace_is_dedicated && pane_is_retained && !pane_has_agent
}

fn validated_provider_args(
    profile: &yard_domain::WorkerProfile,
) -> Result<Vec<String>, YardOrchestratorServiceError> {
    validate_supported_profile(profile)
        .map_err(YardOrchestratorServiceError::UnsupportedProfile)?;
    provider_args(profile).map_err(|error| match error {
        AllocationServiceError::UnsupportedProfile(message) => {
            YardOrchestratorServiceError::UnsupportedProfile(message)
        }
        other => YardOrchestratorServiceError::UnsupportedProfile(other.to_string()),
    })
}

fn runtime_error(error: RuntimeProvisionError) -> YardOrchestratorServiceError {
    match error {
        RuntimeProvisionError::PromptDelivery { message, .. } => {
            YardOrchestratorServiceError::ObjectiveDeliveryFailed(message)
        }
        RuntimeProvisionError::BeforeWorker(message)
        | RuntimeProvisionError::AfterPreparation { message, .. } => {
            YardOrchestratorServiceError::RuntimeProvision(message)
        }
    }
}

fn runtime_restart_outcome_ambiguous(error: &RuntimeProvisionError) -> bool {
    match error {
        RuntimeProvisionError::BeforeWorker(_) => false,
        RuntimeProvisionError::PromptDelivery { .. } => true,
        RuntimeProvisionError::AfterPreparation {
            ambiguous,
            started_runtime,
            ..
        } => *ambiguous || started_runtime.is_some(),
    }
}

fn lifecycle_prompt_text(
    command_id: &str,
    workflow: &OrchestratorWorkflowProfile,
    prompt: &str,
) -> Result<String, YardOrchestratorServiceError> {
    let prompt = with_orchestrator_workflow(prompt, workflow)
        .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
    Ok(with_orchestrator_status_contract(&prompt, command_id))
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

fn dedicated_recovery_binding(
    orchestrator: &YardOrchestrator,
) -> Result<(&yard_domain::Worker, &WorkerRuntimeBinding), YardOrchestratorServiceError> {
    let worker = orchestrator
        .worker
        .as_ref()
        .ok_or(YardOrchestratorServiceError::RecoveryNotConfigured)?;
    let runtime = worker
        .runtime
        .as_ref()
        .filter(|runtime| runtime.session == YARD_ORCHESTRATOR_SESSION)
        .ok_or(YardOrchestratorServiceError::RecoveryNotDedicated)?;
    Ok((worker, runtime))
}

#[derive(Debug, Error)]
pub enum YardOrchestratorServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::YardOrchestratorValidationError),
    #[error(transparent)]
    Reconciliation(#[from] ReconciliationServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("Herdr Yard orchestrator provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("Herdr Yard orchestrator provisioning outcome is ambiguous: {0}")]
    RuntimeProvisionAmbiguous(String),
    #[error("the provisioned Herdr worker did not appear in a fresh inventory")]
    RuntimeBindingUnverified,
    #[error("the reconciled Herdr worker is missing from Yard's durable worker inventory")]
    ReconciledWorkerMissing,
    #[error("the worker was created but the central coordination prompt failed: {0}")]
    ObjectiveDeliveryFailed(String),
    #[error("the Yard orchestrator has not been configured")]
    RecoveryNotConfigured,
    #[error("the configured Yard orchestrator does not use the dedicated Herdr session")]
    RecoveryNotDedicated,
    #[error(
        "the configured Yard orchestrator remains launch-pending in its retained Herdr pane; \
         wait briefly and retry, or press Ctrl+C once in that pane first if it shows a zsh \
         quote> prompt"
    )]
    RecoveryLaunchPending,
    #[error("the configured Yard orchestrator did not reappear after the session restarted")]
    RecoveryBindingMissing,
    #[error("the restarted session did not restore the configured provider identity uniquely")]
    RecoveryBindingAmbiguous,
    #[error("the configured Yard orchestrator has no pinned worker profile to restart")]
    RecoveryProfileMissing,
    #[error("Yard orchestrator ownership changed while the session was restarting")]
    RecoveryOwnershipChanged,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use rusqlite::Connection;
    use tempfile::TempDir;
    use yard_domain::{
        ConfigureYardOrchestrator, CreateWorkerProfile, FocusObservation, ObservedStatus,
        ObservedWorker, PaneObservation, ProviderSessionRef, ProvisionYardOrchestrator,
        RuntimeInventory, RuntimeSession, RuntimeSessions, WorkerProfileSpec, WorkerRuntimeBinding,
        WorkspaceObservation, yard_standard_orchestrator_commands,
    };
    use yard_store::{SqliteProjectStore, YardStore};

    use crate::{
        allocation_service::{
            RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeRetirementError,
            RuntimeRetirementRequest, RuntimeSessionRequest, RuntimeWorkerRestartRequest,
            RuntimeWorkspaceProvisionRequest,
        },
        inventory_service::{InventoryServiceError, InventorySource},
        reconciliation_service::ReconciliationService,
    };

    use super::{
        YARD_ORCHESTRATOR_AGENT_NAME, YARD_ORCHESTRATOR_SESSION, YARD_ORCHESTRATOR_WORKSPACE_LABEL,
        YardOrchestratorService,
    };

    #[derive(Default)]
    struct DedicatedRuntime {
        live: AtomicBool,
        observed_at: AtomicU64,
        ensure_calls: AtomicUsize,
        bootstrap_calls: AtomicUsize,
        retained_shell: AtomicBool,
        launch_pending: AtomicBool,
        restart_calls: Mutex<Vec<RuntimeWorkerRestartRequest>>,
        restore_on_ensure: AtomicBool,
        /// When set, `bootstrap_worker` reports the same failure Herdr
        /// returns when an agent is successfully created and claims the
        /// reserved name, but the initial prompt cannot be delivered.
        fail_prompt_delivery: AtomicBool,
        fail_after_preparation: AtomicBool,
        lifecycle_prompt_failures: AtomicUsize,
        /// When set, the bootstrapped worker never reports as
        /// `interactive_ready`, so
        /// `reconcile_provisioned_worker` runs out its identity-verification
        /// deadline.
        never_ready: AtomicBool,
        omit_provider_session: AtomicBool,
        retirement_identity_misses: AtomicUsize,
        retirement_runtime_failures: AtomicUsize,
        retirement_guard_unavailable: AtomicBool,
        session_requests: Mutex<Vec<RuntimeSessionRequest>>,
        bootstrap_requests: Mutex<Vec<RuntimeWorkspaceProvisionRequest>>,
        prompt_requests: Mutex<Vec<crate::intervention_service::RuntimePromptRequest>>,
        retirement_calls: Mutex<Vec<RuntimeRetirementRequest>>,
    }

    impl DedicatedRuntime {
        fn live() -> Self {
            Self {
                live: AtomicBool::new(true),
                ..Self::default()
            }
        }

        fn binding() -> WorkerRuntimeBinding {
            WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                workspace_id: "workspace-yard-orchestrator".to_owned(),
                terminal_id: "terminal-yard-orchestrator".to_owned(),
                tab_id: Some("tab-yard-orchestrator".to_owned()),
                pane_id: "pane-yard-orchestrator".to_owned(),
                provider_session: Some(provider_session()),
                owns_tab: true,
                observation_state: yard_domain::RuntimeObservationState::Observed,
                process_state: yard_domain::RuntimeProcessState::Running,
                status: ObservedStatus::Idle,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 1,
            }
        }

        fn current_binding(&self) -> WorkerRuntimeBinding {
            let mut runtime = Self::binding();
            if self.omit_provider_session.load(Ordering::SeqCst) {
                runtime.provider_session = None;
            }
            runtime
        }
    }

    #[async_trait]
    impl InventorySource for DedicatedRuntime {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![RuntimeSession {
                    name: YARD_ORCHESTRATOR_SESSION.to_owned(),
                    is_default: false,
                    running: true,
                }],
            })
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            assert_eq!(session_name, YARD_ORCHESTRATOR_SESSION);
            let observed_at_unix_ms = self.observed_at.fetch_add(1, Ordering::SeqCst) + 1;
            let live = self.live.load(Ordering::SeqCst);
            let retained_shell = self.retained_shell.load(Ordering::SeqCst);
            let topology_present = live || retained_shell;
            Ok(RuntimeInventory {
                adapter: "herdr".to_owned(),
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                runtime_version: "test".to_owned(),
                protocol: 19,
                observed_at_unix_ms,
                focus: FocusObservation::default(),
                workspaces: topology_present
                    .then(|| WorkspaceObservation {
                        runtime_id: "workspace-yard-orchestrator".to_owned(),
                        order: 1,
                        label: YARD_ORCHESTRATOR_WORKSPACE_LABEL.to_owned(),
                        focused: false,
                        active_tab_id: "tab-yard-orchestrator".to_owned(),
                        pane_count: 1,
                        tab_count: 1,
                        status: ObservedStatus::Idle,
                        tokens: BTreeMap::new(),
                        worktree: None,
                    })
                    .into_iter()
                    .collect(),
                tabs: Vec::new(),
                panes: retained_shell
                    .then(|| PaneObservation {
                        runtime_id: "pane-yard-orchestrator".to_owned(),
                        terminal_id: "terminal-yard-orchestrator".to_owned(),
                        workspace_id: "workspace-yard-orchestrator".to_owned(),
                        tab_id: "tab-yard-orchestrator".to_owned(),
                        focused: false,
                        cwd: Some("/tmp/yard-backend".to_owned()),
                        foreground_cwd: Some("/tmp/yard-backend".to_owned()),
                        label: None,
                        provider: None,
                        display_provider: None,
                        status: ObservedStatus::Unknown,
                        tokens: BTreeMap::new(),
                        provider_session: None,
                        revision: 0,
                    })
                    .into_iter()
                    .collect(),
                workers: live
                    .then(|| ObservedWorker {
                        runtime_id: "terminal-yard-orchestrator".to_owned(),
                        terminal_id: "terminal-yard-orchestrator".to_owned(),
                        workspace_id: "workspace-yard-orchestrator".to_owned(),
                        tab_id: "tab-yard-orchestrator".to_owned(),
                        pane_id: "pane-yard-orchestrator".to_owned(),
                        name: Some(YARD_ORCHESTRATOR_AGENT_NAME.to_owned()),
                        provider: Some("codex".to_owned()),
                        display_provider: Some("Codex".to_owned()),
                        status: ObservedStatus::Idle,
                        focused: false,
                        launch_pending: self.launch_pending.load(Ordering::SeqCst),
                        interactive_ready: !self.never_ready.load(Ordering::SeqCst)
                            && !self.launch_pending.load(Ordering::SeqCst),
                        state_change_sequence: 1,
                        cwd: Some("/tmp/yard-backend".to_owned()),
                        foreground_cwd: Some("/tmp/yard-backend".to_owned()),
                        tokens: BTreeMap::new(),
                        provider_session: (!self.omit_provider_session.load(Ordering::SeqCst))
                            .then(provider_session),
                        revision: 1,
                    })
                    .into_iter()
                    .collect(),
                child_agents: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl RuntimeControl for DedicatedRuntime {
        async fn ensure_session(
            &self,
            request: RuntimeSessionRequest,
        ) -> Result<(), RuntimeProvisionError> {
            self.ensure_calls.fetch_add(1, Ordering::SeqCst);
            self.session_requests.lock().unwrap().push(request);
            if self.restore_on_ensure.load(Ordering::SeqCst) {
                self.live.store(true, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn restart_worker(
            &self,
            request: RuntimeWorkerRestartRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.restart_calls.lock().unwrap().push(request);
            self.live.store(true, Ordering::SeqCst);
            self.retained_shell.store(false, Ordering::SeqCst);
            Ok(self.current_binding())
        }

        async fn bootstrap_worker(
            &self,
            request: RuntimeWorkspaceProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.bootstrap_calls.fetch_add(1, Ordering::SeqCst);
            self.bootstrap_requests.lock().unwrap().push(request);
            if self.fail_after_preparation.load(Ordering::SeqCst) {
                return Err(RuntimeProvisionError::AfterPreparation {
                    message: "agent.start returned an unverified runtime".to_owned(),
                    ambiguous: false,
                    started_runtime: Some(Box::new(self.current_binding())),
                });
            }
            if self.fail_prompt_delivery.load(Ordering::SeqCst) {
                // Mirrors Herdr: the agent was created and claimed the
                // reserved name, but the initial prompt could not be
                // delivered.
                return Err(RuntimeProvisionError::PromptDelivery {
                    runtime: Box::new(self.current_binding()),
                    message: "Herdr agent is not ready to receive input".to_owned(),
                });
            }
            self.live.store(true, Ordering::SeqCst);
            Ok(self.current_binding())
        }

        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            unreachable!("the Yard singleton always bootstraps a new workspace")
        }

        async fn retire_runtime(
            &self,
            request: RuntimeRetirementRequest,
        ) -> Result<(), RuntimeRetirementError> {
            self.retirement_calls.lock().unwrap().push(request);
            if self.retirement_guard_unavailable.load(Ordering::SeqCst) {
                return Err(RuntimeRetirementError::AtomicIdentityGuardUnavailable);
            }
            if self
                .retirement_runtime_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(RuntimeRetirementError::Runtime(
                    "transient Herdr socket failure".to_owned(),
                ));
            }
            if self
                .retirement_identity_misses
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(RuntimeRetirementError::IdentityNotObserved);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl crate::intervention_service::RuntimeIntervention for DedicatedRuntime {
        async fn prompt(
            &self,
            request: crate::intervention_service::RuntimePromptRequest,
        ) -> Result<
            crate::intervention_service::RuntimePromptResult,
            crate::intervention_service::RuntimeInterventionError,
        > {
            self.prompt_requests.lock().unwrap().push(request);
            if self
                .lifecycle_prompt_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(
                    crate::intervention_service::RuntimeInterventionError::Rejected(
                        "Herdr rejected the lifecycle prompt".to_owned(),
                    ),
                );
            }
            Ok(crate::intervention_service::RuntimePromptResult {
                status: "submitted".to_owned(),
            })
        }

        async fn read_output(
            &self,
            _request: crate::intervention_service::RuntimeOutputRequest,
        ) -> Result<
            crate::intervention_service::RuntimeOutputResult,
            crate::intervention_service::RuntimeInterventionError,
        > {
            unreachable!("Yard orchestrator service tests do not read terminal output")
        }
    }

    fn profile() -> CreateWorkerProfile {
        CreateWorkerProfile {
            spec: WorkerProfileSpec {
                name: "Central coordinator".to_owned(),
                runtime_adapter: "herdr".to_owned(),
                provider: "codex".to_owned(),
                model: Some("gpt-5.4".to_owned()),
                default_role: "orchestrator".to_owned(),
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

    fn provider_session() -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: "yard-orchestrator-session".to_owned(),
        }
    }

    async fn service(
        runtime: Arc<DedicatedRuntime>,
    ) -> (
        YardOrchestratorService,
        Arc<SqliteProjectStore>,
        TempDir,
        String,
    ) {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let profile = store.create_worker_profile(profile()).await.unwrap();
        let source: Arc<dyn InventorySource> = runtime.clone();
        let reconciliation = ReconciliationService::new(source, store.clone());
        let control: Arc<dyn RuntimeControl> = runtime.clone();
        let intervention: Arc<dyn crate::intervention_service::RuntimeIntervention> = runtime;
        (
            YardOrchestratorService::new(
                control,
                intervention,
                store.clone(),
                reconciliation,
                "/tmp/yard-backend".to_owned(),
            ),
            store,
            temp,
            profile.id,
        )
    }

    fn command(profile_id: &str) -> ProvisionYardOrchestrator {
        ProvisionYardOrchestrator {
            command_id: "provision-yard-orchestrator".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile_id.to_owned(),
            expected_profile_version: 1,
            expected_orchestrator_version: 1,
        }
    }

    #[tokio::test]
    async fn concurrent_retries_bootstrap_once_and_replay_configuration() {
        let runtime = Arc::new(DedicatedRuntime::default());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let first_service = service.clone();
        let second_service = service;
        let first_command = command(&profile_id);
        let second_command = first_command.clone();

        let (first, second) = tokio::join!(
            first_service.provision(first_command),
            second_service.provision(second_command)
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.ensure_calls.load(Ordering::SeqCst), 2);
        assert_ne!(first.replayed, second.replayed);
        assert_eq!(
            first.orchestrator.worker.as_ref().unwrap().id,
            second.orchestrator.worker.as_ref().unwrap().id
        );
        assert_eq!(
            first.orchestrator.worker.as_ref().unwrap().profile_id,
            Some(profile_id)
        );
        assert_eq!(
            first.orchestrator.worker.as_ref().unwrap().profile_version,
            Some(1)
        );
        assert_eq!(first.orchestrator.workflow_profile_version, 1);

        let sessions = runtime.session_requests.lock().unwrap();
        assert!(sessions.iter().all(|request| {
            request.session == YARD_ORCHESTRATOR_SESSION
                && request.startup_cwd == "/tmp/yard-backend"
        }));
        let bootstraps = runtime.bootstrap_requests.lock().unwrap();
        assert_eq!(bootstraps.len(), 1);
        let request = &bootstraps[0];
        assert_eq!(request.session, YARD_ORCHESTRATOR_SESSION);
        assert_eq!(request.workspace_label, YARD_ORCHESTRATOR_WORKSPACE_LABEL);
        assert_eq!(request.agent_name, YARD_ORCHESTRATOR_AGENT_NAME);
        assert_eq!(request.cwd, "/tmp/yard-backend");
        assert_eq!(request.args, ["--no-alt-screen", "--yolo", "-m", "gpt-5.4"]);
        assert!(request.prompt.contains("Coordinate all Yard projects"));
        assert!(request.prompt.contains("Role: central orchestrator"));
        assert!(request.prompt.contains("Allocate independent workers"));
        assert!(
            request
                .prompt
                .contains("originating central orchestrator pane")
        );
        assert!(
            request
                .prompt
                .contains("exact captured orchestrator workspace")
        );
        assert!(request.prompt.contains("isolated Git worktree"));
        assert!(
            request
                .prompt
                .contains("plain tab for read-only investigation")
        );
        assert!(request.prompt.contains("every 10 minutes"));
        assert!(request.prompt.contains("configured cadence"));
        assert!(request.prompt.contains("concrete observation"));
        assert!(request.prompt.contains("coherent commit IDs"));
        assert!(
            request
                .prompt
                .contains("deterministic artifacts at named paths")
        );
        assert!(request.prompt.contains("independent quality review"));
        assert!(request.prompt.contains("integrate accepted commits"));
        assert!(request.prompt.contains("retaining branches, worktrees"));
        assert!(
            request
                .prompt
                .contains("command \"provision-yard-orchestrator\"")
        );
        assert!(request.prompt.contains("There is no completed state"));
        assert!(
            yard_domain::OrchestratorStatusReport::scan_terminal_output(&request.prompt).is_none()
        );
    }

    #[tokio::test]
    async fn completed_configuration_replays_historical_explicit_workflow_without_runtime_call() {
        let runtime = Arc::new(DedicatedRuntime::default());
        let (service, store, temp, profile_id) = service(runtime.clone()).await;
        let provisioned = service.provision(command(&profile_id)).await.unwrap();
        let worker = provisioned.orchestrator.worker.as_ref().unwrap();
        let prompt_count = runtime.prompt_requests.lock().unwrap().len();
        let mut incomplete_commands = yard_standard_orchestrator_commands();
        incomplete_commands.retain(|command| command.id != "result.integrate");
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "INSERT INTO orchestrator_workflow_profile_revisions (
                    profile_id, version, name, description,
                    instructions_markdown, monitor_interval_ms,
                    commands_json, adapter_context_files_json,
                    source, updated_by, created_at_unix_ms
                 )
                 SELECT profile_id, 2, name, description,
                        instructions_markdown, monitor_interval_ms,
                        ?1, adapter_context_files_json,
                        source, 'historical-replay-fixture', created_at_unix_ms
                   FROM orchestrator_workflow_profile_revisions
                  WHERE profile_id = 'yard:standard-orchestrator'
                    AND version = 1",
                [serde_json::to_string(&incomplete_commands).unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE yard_orchestrator_configure_commands
                    SET workflow_profile_version = 2
                  WHERE command_id = 'provision-yard-orchestrator'",
                [],
            )
            .unwrap();
        drop(connection);

        let replayed = service
            .configure(ConfigureYardOrchestrator {
                command_id: "provision-yard-orchestrator".to_owned(),
                actor: "local-user".to_owned(),
                worker_id: worker.id.clone(),
                expected_worker_version: worker.version,
                expected_orchestrator_version: 1,
                workflow_profile_version: Some(2),
            })
            .await
            .unwrap();

        assert!(replayed.replayed);
        assert_eq!(replayed.orchestrator.workflow_profile_version, 2);
        assert_eq!(runtime.prompt_requests.lock().unwrap().len(), prompt_count);
        assert_eq!(
            store
                .get_yard_orchestrator()
                .await
                .unwrap()
                .workflow_profile_version,
            1
        );
    }

    #[tokio::test]
    async fn fresh_provision_uses_and_pins_the_active_workflow_revision() {
        let runtime = Arc::new(DedicatedRuntime::default());
        let (service, store, _temp, profile_id) = service(runtime.clone()).await;
        let factory = store.get_orchestrator_workflow_profile().await.unwrap();
        let edited = store
            .update_orchestrator_workflow_profile(
                yard_domain::YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                yard_domain::UpdateOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: factory.version,
                    instructions_markdown: "# Current workflow\n\nUse current lanes.".to_owned(),
                    monitor_interval_ms: 900_000,
                    commands: Some(yard_domain::yard_standard_orchestrator_commands()),
                    adapter_context_files: None,
                },
            )
            .await
            .unwrap();

        let configured = service.provision(command(&profile_id)).await.unwrap();

        assert_eq!(
            configured.orchestrator.workflow_profile_version,
            edited.version
        );
        let request = &runtime.bootstrap_requests.lock().unwrap()[0];
        assert!(
            request.prompt.contains(
                "Yard orchestrator workflow profile yard:standard-orchestrator revision 2",
            )
        );
        assert!(request.prompt.contains("# Current workflow"));
        assert!(request.prompt.contains("monitor interval: 900000 ms"));
        assert!(request.prompt.contains("\"id\":\"result.integrate\""));
        assert!(
            request.prompt.find("\"id\":\"work.decompose\"").unwrap()
                < request.prompt.find("\"id\":\"worker.allocate\"").unwrap()
        );
    }

    #[tokio::test]
    async fn unsupported_persisted_workflow_capability_fails_before_runtime_mutation() {
        let runtime = Arc::new(DedicatedRuntime::default());
        let (service, _store, temp, profile_id) = service(runtime.clone()).await;
        let mut commands = yard_domain::yard_standard_orchestrator_commands();
        commands[0].capability = "orchestration.unsupported".to_owned();
        let commands = serde_json::to_string(&commands).unwrap();
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        connection
            .execute(
                "INSERT INTO orchestrator_workflow_profile_revisions (
                    profile_id, version, name, description, instructions_markdown,
                    monitor_interval_ms, commands_json, adapter_context_files_json,
                    source, updated_by, created_at_unix_ms
                 )
                 SELECT profile_id, 2, name, description, instructions_markdown,
                        monitor_interval_ms, ?1, adapter_context_files_json,
                        'user', 'corrupt-fixture', 2
                   FROM orchestrator_workflow_profile_revisions
                  WHERE profile_id = 'yard:standard-orchestrator' AND version = 1",
                [commands],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE orchestrator_workflow_profiles
                    SET current_version = 2
                  WHERE id = 'yard:standard-orchestrator'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE orchestrator_workflow_profile_current
                    SET current_version = 2
                  WHERE singleton_id = 1",
                [],
            )
            .unwrap();
        drop(connection);

        let error = service.provision(command(&profile_id)).await.unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::Store(
                yard_store::ProjectStoreError::InvalidOrchestratorWorkflowProfile(_)
            )
        ));
        assert_eq!(runtime.ensure_calls.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let intents: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM dedicated_runtime_provision_intents",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(intents, 0);
    }

    #[tokio::test]
    async fn provisions_interactive_worker_without_provider_session() {
        let runtime = Arc::new(DedicatedRuntime::default());
        runtime.omit_provider_session.store(true, Ordering::SeqCst);
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;

        let configured = service.provision(command(&profile_id)).await.unwrap();

        assert!(configured.orchestrator.worker.is_some());
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        assert!(runtime.retirement_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn adopts_a_preexisting_dedicated_worker_without_bootstrapping() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;

        let configured = service.provision(command(&profile_id)).await.unwrap();

        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        let worker = configured.orchestrator.worker.unwrap();
        assert_eq!(
            worker.runtime.unwrap().terminal_id,
            "terminal-yard-orchestrator"
        );
        let prompts = runtime.prompt_requests.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].command_id, "provision-yard-orchestrator");
        assert!(
            prompts[0]
                .text
                .contains("Assume ownership of central Yard orchestration")
        );
        assert!(prompts[0].text.contains("Allocate independent workers"));
    }

    #[tokio::test]
    async fn retries_rejected_adoption_workflow_with_the_same_command_identity() {
        let runtime = Arc::new(DedicatedRuntime::live());
        runtime.lifecycle_prompt_failures.store(1, Ordering::SeqCst);
        let (service, store, _temp, profile_id) = service(runtime.clone()).await;
        let command = command(&profile_id);

        let error = service.provision(command.clone()).await.unwrap_err();
        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::ObjectiveDeliveryFailed(_)
        ));
        assert!(
            store
                .get_yard_orchestrator()
                .await
                .unwrap()
                .worker
                .is_some()
        );

        let profile = store.get_worker_profile(&profile_id).await.unwrap();
        let mut updated_spec = profile.spec;
        updated_spec.model = Some("gpt-5.5".to_owned());
        store
            .update_worker_profile(
                &profile_id,
                yard_domain::UpdateWorkerProfile {
                    spec: updated_spec,
                    expected_version: profile.version,
                },
            )
            .await
            .unwrap();
        let active_workflow = store.get_orchestrator_workflow_profile().await.unwrap();
        store
            .update_orchestrator_workflow_profile(
                yard_domain::YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                yard_domain::UpdateOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: active_workflow.version,
                    instructions_markdown:
                        "# Replacement workflow\n\nDo not use this for the replay.".to_owned(),
                    monitor_interval_ms: 300_000,
                    commands: None,
                    adapter_context_files: None,
                },
            )
            .await
            .unwrap();

        let replay = service.provision(command).await.unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.orchestrator.workflow_profile_version, 1);
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        let prompts = runtime.prompt_requests.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        assert!(
            prompts
                .iter()
                .all(|prompt| prompt.command_id == "provision-yard-orchestrator")
        );
        assert!(
            prompts
                .iter()
                .all(|prompt| prompt.text.contains("Allocate independent workers"))
        );
        assert!(prompts.iter().all(|prompt| {
            prompt.text.contains(
                "Yard orchestrator workflow profile yard:standard-orchestrator revision 1",
            )
        }));
        assert!(
            prompts
                .iter()
                .all(|prompt| !prompt.text.contains("# Replacement workflow"))
        );
    }

    #[tokio::test]
    async fn recovers_the_configured_worker_without_bootstrapping_or_reassigning() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, store, temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        let worker_id = configured.orchestrator.worker.unwrap().id;
        runtime.prompt_requests.lock().unwrap().clear();

        runtime.live.store(false, Ordering::SeqCst);
        runtime.restore_on_ensure.store(true, Ordering::SeqCst);
        let recovery = yard_domain::RecoverYardOrchestrator {
            command_id: "recover-yard-orchestrator".to_owned(),
            actor: "local-user".to_owned(),
            expected_orchestrator_version: configured.orchestrator.version,
        };
        let recovered = service.recover(recovery.clone()).await.unwrap();
        let replayed = service.recover(recovery).await.unwrap();

        assert_eq!(recovered.command_id, "recover-yard-orchestrator");
        assert_eq!(replayed.command_id, "recover-yard-orchestrator");
        assert_eq!(
            recovered.orchestrator.worker.as_ref().unwrap().id,
            worker_id
        );
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        let (prompt_command_id, prompt_text) = {
            let prompts = runtime.prompt_requests.lock().unwrap();
            assert_eq!(prompts.len(), 1);
            (prompts[0].command_id.clone(), prompts[0].text.clone())
        };
        assert_eq!(prompt_command_id, "recover-yard-orchestrator");
        assert!(prompt_text.contains("Allocate independent workers"));
        assert!(prompt_text.contains("Resume central Yard orchestration after runtime recovery"));
        let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let durable_status: (String, String) = connection
            .query_row(
                "SELECT command_type, status
                   FROM command_acknowledgements
                  WHERE id = 'recover-yard-orchestrator'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            durable_status,
            (
                "yard_orchestrator_prompt".to_owned(),
                "succeeded".to_owned()
            )
        );
        assert_eq!(
            store
                .get_yard_orchestrator()
                .await
                .unwrap()
                .worker
                .unwrap()
                .runtime
                .unwrap()
                .observation_state,
            yard_domain::RuntimeObservationState::Observed
        );
    }

    #[tokio::test]
    async fn restart_recovery_replays_after_runtime_is_restored() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        let worker_id = configured.orchestrator.worker.as_ref().unwrap().id.clone();
        runtime.prompt_requests.lock().unwrap().clear();

        runtime.live.store(false, Ordering::SeqCst);
        runtime.retained_shell.store(true, Ordering::SeqCst);
        let recovery = yard_domain::RecoverYardOrchestrator {
            command_id: "recover-retained-yard-orchestrator".to_owned(),
            actor: "local-user".to_owned(),
            expected_orchestrator_version: configured.orchestrator.version,
        };
        let recovered = service.recover(recovery.clone()).await.unwrap();
        let replayed = service.recover(recovery).await.unwrap();

        assert_eq!(
            recovered.orchestrator.worker.as_ref().unwrap().id,
            worker_id
        );
        assert_eq!(replayed.orchestrator.worker.as_ref().unwrap().id, worker_id);
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        let restart_calls = runtime.restart_calls.lock().unwrap();
        assert_eq!(restart_calls.len(), 1);
        assert_eq!(restart_calls[0].runtime.pane_id, "pane-yard-orchestrator");
        assert!(
            restart_calls[0]
                .prompt
                .contains("Coordinate all Yard projects from this dedicated central session")
        );
        assert!(
            restart_calls[0]
                .prompt
                .contains("Resume central Yard orchestration after runtime recovery")
        );
        drop(restart_calls);

        let prompts = runtime.prompt_requests.lock().unwrap();
        assert!(prompts.is_empty());
    }

    #[tokio::test]
    async fn recovers_the_configured_worker_without_provider_session() {
        let runtime = Arc::new(DedicatedRuntime::default());
        runtime.omit_provider_session.store(true, Ordering::SeqCst);
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        let worker_id = configured.orchestrator.worker.as_ref().unwrap().id.clone();
        runtime.prompt_requests.lock().unwrap().clear();

        let recovered = service
            .recover(yard_domain::RecoverYardOrchestrator {
                command_id: "recover-yard-orchestrator-without-provider-session".to_owned(),
                actor: "local-user".to_owned(),
                expected_orchestrator_version: configured.orchestrator.version,
            })
            .await
            .unwrap();

        assert_eq!(
            recovered.orchestrator.worker.as_ref().unwrap().id,
            worker_id
        );
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.prompt_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recovery_reports_a_launch_pending_worker_without_starting_a_duplicate() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        runtime.prompt_requests.lock().unwrap().clear();
        runtime.launch_pending.store(true, Ordering::SeqCst);
        runtime.omit_provider_session.store(true, Ordering::SeqCst);

        let error = service
            .recover(yard_domain::RecoverYardOrchestrator {
                command_id: "recover-launch-pending-yard-orchestrator".to_owned(),
                actor: "local-user".to_owned(),
                expected_orchestrator_version: configured.orchestrator.version,
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::RecoveryLaunchPending
        ));
        assert!(runtime.restart_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn recovery_fails_closed_when_the_configured_worker_is_not_restored() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        runtime.live.store(false, Ordering::SeqCst);

        let error = service
            .recover(yard_domain::RecoverYardOrchestrator {
                command_id: "recover-yard-orchestrator".to_owned(),
                actor: "local-user".to_owned(),
                expected_orchestrator_version: configured.orchestrator.version,
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::RecoveryBindingMissing
        ));
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
    }

    // Regression coverage for the recurring "Herdr agent start failed:
    // agent_name_taken" bug: Herdr enforces the `yard-orchestrator` agent
    // name uniquely, so any runtime that claims it but that Yard fails to
    // durably adopt must be retired immediately. Otherwise it becomes a
    // permanent orphan and every later `provision` call fails the same way
    // until a human runs `herdr session list`/`stop`/`delete` by hand.

    #[tokio::test]
    async fn quarantines_prompt_delivery_identity_and_never_retries_bootstrap() {
        let runtime = Arc::new(DedicatedRuntime::default());
        runtime.fail_prompt_delivery.store(true, Ordering::SeqCst);
        let (service, store, temp, profile_id) = service(runtime.clone()).await;

        let provision = command(&profile_id);
        let error = service.provision(provision.clone()).await.unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::ObjectiveDeliveryFailed(_)
        ));
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        assert!(runtime.retirement_calls.lock().unwrap().is_empty());
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let quarantined: (String, Option<String>) = connection
            .query_row(
                "SELECT terminal_id, provider_session_value
                   FROM quarantined_provisioning_runtime_bindings
                  WHERE command_id = ?1",
                [&provision.command_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            quarantined,
            (
                "terminal-yard-orchestrator".to_owned(),
                Some("yard-orchestrator-session".to_owned())
            )
        );
        drop(connection);
        assert!(matches!(
            service.provision(provision).await.unwrap_err(),
            super::YardOrchestratorServiceError::Store(
                yard_store::ProjectStoreError::CommandOutcomeAmbiguous(_)
            )
        ));
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        drop(store);
    }

    #[tokio::test]
    async fn after_preparation_started_runtime_is_quarantined_as_ambiguous() {
        let runtime = Arc::new(DedicatedRuntime::default());
        runtime.fail_after_preparation.store(true, Ordering::SeqCst);
        let (service, _store, temp, profile_id) = service(runtime.clone()).await;
        let provision = command(&profile_id);

        let error = service.provision(provision.clone()).await.unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::RuntimeProvisionAmbiguous(_)
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
                [&provision.command_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            captured,
            (
                "ambiguous".to_owned(),
                "terminal-yard-orchestrator".to_owned(),
                Some("yard-orchestrator-session".to_owned())
            )
        );
    }

    #[tokio::test]
    async fn retires_the_orphaned_runtime_when_the_bootstrapped_worker_never_reconciles() {
        let runtime = Arc::new(DedicatedRuntime::default());
        runtime.never_ready.store(true, Ordering::SeqCst);
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;

        let error = service.provision(command(&profile_id)).await.unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::RuntimeBindingUnverified
        ));
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);

        let retirements = runtime.retirement_calls.lock().unwrap();
        assert_eq!(retirements.len(), 1);
        assert_eq!(retirements[0].terminal_id, "terminal-yard-orchestrator");
    }

    #[tokio::test]
    async fn orphan_retirement_stops_when_atomic_identity_guard_is_unavailable() {
        let runtime = Arc::new(DedicatedRuntime::default());
        runtime.never_ready.store(true, Ordering::SeqCst);
        runtime
            .retirement_guard_unavailable
            .store(true, Ordering::SeqCst);
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;

        let error = service.provision(command(&profile_id)).await.unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::RuntimeBindingUnverified
        ));
        assert_eq!(runtime.retirement_calls.lock().unwrap().len(), 1);
    }
}

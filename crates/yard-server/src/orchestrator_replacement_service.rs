use std::sync::Arc;

use thiserror::Error;
use tracing::warn;
use yard_domain::{
    Project, ReplaceProjectOrchestrator, ReplacedProjectOrchestrator, RuntimeObservationState,
    RuntimeProcessState, WorkerRuntimeBinding,
};
use yard_store::{BeginProjectOrchestratorReplacement, ProjectStoreError, YardStore};

use crate::{
    allocation_service::{
        RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, agent_name,
        assignment_prompt, provider_args, validate_supported_profile,
    },
    inventory_service::{InventoryServiceError, InventorySource},
    runtime_cleanup_service::RuntimeCleanupService,
    status_protocol::with_orchestrator_status_contract,
};

#[derive(Clone)]
pub struct OrchestratorReplacementService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeControl>,
    store: Arc<dyn YardStore>,
    cleanup: RuntimeCleanupService,
}

impl OrchestratorReplacementService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        Self {
            cleanup: RuntimeCleanupService::new(Arc::clone(&runtime), Arc::clone(&store)),
            source,
            runtime,
            store,
        }
    }

    /// Replace one project orchestrator with a newly provisioned worker.
    ///
    /// The displaced runtime is durably captured before Herdr is called. The
    /// prepared replacement is claimed before start, both bindings are freshly
    /// verified, and ownership changes only in the final store transaction.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorReplacementServiceError`] for invalid or stale
    /// commands, unsupported profiles, unverifiable runtime identity, runtime
    /// failures, or persistence failures.
    #[allow(clippy::too_many_lines)]
    pub async fn replace_project(
        &self,
        project_id: &str,
        command: ReplaceProjectOrchestrator,
    ) -> Result<ReplacedProjectOrchestrator, OrchestratorReplacementServiceError> {
        let command = command.normalize()?;
        let command_id = command.command_id.clone();
        let context = match self
            .store
            .begin_project_orchestrator_replacement(project_id, command)
            .await?
        {
            BeginProjectOrchestratorReplacement::Replayed(mut replacement) => {
                self.process_cleanup(&command_id, &mut replacement).await;
                return Ok(*replacement);
            }
            BeginProjectOrchestratorReplacement::Started(context) => *context,
        };
        if let Err(message) = validate_supported_profile(&context.profile) {
            self.store
                .fail_project_orchestrator_replacement(&command_id, &message, false)
                .await?;
            return Err(OrchestratorReplacementServiceError::UnsupportedProfile(
                message,
            ));
        }
        let args = match provider_args(&context.profile) {
            Ok(args) => args,
            Err(error) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &error.to_string(), false)
                    .await?;
                return Err(OrchestratorReplacementServiceError::UnsupportedProfile(
                    error.to_string(),
                ));
            }
        };

        let cwd = match self
            .verify_target_and_displaced_runtime(
                &context.project,
                &context.command.expected_orchestrator_runtime,
            )
            .await
        {
            Ok(cwd) => cwd,
            Err(error) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &error.to_string(), false)
                    .await?;
                return Err(error);
            }
        };
        let provision = RuntimeProvisionRequest {
            command_id: command_id.clone(),
            session: context.project.runtime.session.clone(),
            workspace_id: context.project.runtime.workspace_id.clone(),
            cwd,
            tab_label: format!("{} replacement", context.project.name),
            agent_name: agent_name(&command_id),
            kind: context.profile.spec.provider.clone(),
            args,
            prompt: with_orchestrator_status_contract(
                &assignment_prompt(
                    &context.command.objective,
                    &context.command.role,
                    &context.profile,
                ),
                &command_id,
            ),
        };

        let prepared = match self
            .runtime
            .prepare_replacement_worker(provision.clone())
            .await
        {
            Ok(runtime) => runtime,
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, false)
                    .await?;
                return Err(OrchestratorReplacementServiceError::RuntimeProvision(
                    message,
                ));
            }
            Err(RuntimeProvisionError::AfterPreparation { message, ambiguous }) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, ambiguous)
                    .await?;
                return Err(OrchestratorReplacementServiceError::RuntimeProvision(
                    message,
                ));
            }
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                let retained = self
                    .store
                    .claim_project_orchestrator_replacement_runtime(&command_id, (*runtime).clone())
                    .await;
                let started = match retained {
                    Ok(()) => {
                        self.store
                            .record_project_orchestrator_replacement_started_runtime(
                                &command_id,
                                *runtime,
                            )
                            .await
                    }
                    Err(error) => Err(error),
                };
                let message = match started {
                    Ok(()) => message,
                    Err(error) => format!("{message}; started runtime capture failed: {error}"),
                };
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, true)
                    .await?;
                return Err(OrchestratorReplacementServiceError::ObjectiveDeliveryFailed(message));
            }
        };
        if let Err(error) = self
            .store
            .claim_project_orchestrator_replacement_runtime(&command_id, prepared.clone())
            .await
        {
            self.store
                .fail_project_orchestrator_replacement(&command_id, &error.to_string(), true)
                .await?;
            return Err(error.into());
        }

        let started = match self
            .runtime
            .start_prepared_replacement_worker(provision, prepared)
            .await
        {
            Ok(runtime) => runtime,
            Err(RuntimeProvisionError::PromptDelivery { runtime, message }) => {
                let message = match self
                    .store
                    .record_project_orchestrator_replacement_started_runtime(&command_id, *runtime)
                    .await
                {
                    Ok(()) => message,
                    Err(error) => format!("{message}; started runtime capture failed: {error}"),
                };
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, true)
                    .await?;
                return Err(OrchestratorReplacementServiceError::ObjectiveDeliveryFailed(message));
            }
            Err(RuntimeProvisionError::AfterPreparation { message, ambiguous }) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, ambiguous)
                    .await?;
                return Err(OrchestratorReplacementServiceError::RuntimeProvision(
                    message,
                ));
            }
            Err(RuntimeProvisionError::BeforeWorker(message)) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &message, true)
                    .await?;
                return Err(OrchestratorReplacementServiceError::RuntimeProvision(
                    message,
                ));
            }
        };
        if let Err(error) = self
            .store
            .record_project_orchestrator_replacement_started_runtime(&command_id, started.clone())
            .await
        {
            self.store
                .fail_project_orchestrator_replacement(&command_id, &error.to_string(), true)
                .await?;
            return Err(error.into());
        }
        let replacement = match self.verify_replacement_runtime(started).await {
            Ok(runtime) => runtime,
            Err(error) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &error.to_string(), true)
                    .await?;
                return Err(error);
            }
        };
        if let Err(error) = self
            .verify_target_and_displaced_runtime(
                &context.project,
                &context.command.expected_orchestrator_runtime,
            )
            .await
        {
            self.store
                .fail_project_orchestrator_replacement(&command_id, &error.to_string(), true)
                .await?;
            return Err(error);
        }

        let mut result = match self
            .store
            .finalize_project_orchestrator_replacement(&command_id, replacement)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                self.store
                    .fail_project_orchestrator_replacement(&command_id, &error.to_string(), true)
                    .await?;
                return Err(error.into());
            }
        };
        self.process_cleanup(&command_id, &mut result).await;
        Ok(result)
    }

    async fn verify_target_and_displaced_runtime(
        &self,
        project: &Project,
        expected: &WorkerRuntimeBinding,
    ) -> Result<String, OrchestratorReplacementServiceError> {
        if project.runtime.adapter != expected.adapter
            || project.runtime.session != expected.session
            || project.runtime.workspace_id != expected.workspace_id
        {
            return Err(OrchestratorReplacementServiceError::RuntimeIdentityChanged);
        }
        let expected_provider = expected
            .provider_session
            .as_ref()
            .ok_or(OrchestratorReplacementServiceError::RuntimeIdentityChanged)?;
        let inventory = self.source.inventory(&project.runtime.session).await?;
        if inventory.adapter != project.runtime.adapter
            || inventory.session != project.runtime.session
        {
            return Err(OrchestratorReplacementServiceError::RuntimeIdentityChanged);
        }
        let workspace = inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.runtime_id == project.runtime.workspace_id)
            .ok_or_else(|| {
                OrchestratorReplacementServiceError::RuntimeWorkspaceMissing(
                    project.runtime.workspace_id.clone(),
                )
            })?;
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == expected.terminal_id)
            .ok_or(OrchestratorReplacementServiceError::RuntimeIdentityChanged)?;
        if observed.workspace_id != project.runtime.workspace_id
            || observed.pane_id != expected.pane_id
            || Some(observed.tab_id.as_str()) != expected.tab_id.as_deref()
            || observed.provider_session.as_ref() != Some(expected_provider)
            || !observed.interactive_ready
        {
            return Err(OrchestratorReplacementServiceError::RuntimeIdentityChanged);
        }
        workspace
            .worktree
            .as_ref()
            .map(|worktree| worktree.checkout_path.clone())
            .or_else(|| {
                observed
                    .foreground_cwd
                    .clone()
                    .or_else(|| observed.cwd.clone())
            })
            .ok_or(OrchestratorReplacementServiceError::RuntimeCwdUnavailable)
    }

    async fn verify_replacement_runtime(
        &self,
        mut runtime: WorkerRuntimeBinding,
    ) -> Result<WorkerRuntimeBinding, OrchestratorReplacementServiceError> {
        let inventory = self.source.inventory(&runtime.session).await?;
        if inventory.adapter != runtime.adapter
            || inventory.session != runtime.session
            || !inventory
                .workspaces
                .iter()
                .any(|workspace| workspace.runtime_id == runtime.workspace_id)
        {
            return Err(OrchestratorReplacementServiceError::ReplacementUnverified);
        }
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == runtime.terminal_id)
            .ok_or(OrchestratorReplacementServiceError::ReplacementUnverified)?;
        if observed.workspace_id != runtime.workspace_id
            || observed.pane_id != runtime.pane_id
            || Some(observed.tab_id.as_str()) != runtime.tab_id.as_deref()
            || !observed.interactive_ready
        {
            return Err(OrchestratorReplacementServiceError::ReplacementUnverified);
        }
        let provider_session = observed
            .provider_session
            .clone()
            .ok_or(OrchestratorReplacementServiceError::ReplacementUnverified)?;
        if runtime
            .provider_session
            .as_ref()
            .is_some_and(|expected| expected != &provider_session)
        {
            return Err(OrchestratorReplacementServiceError::ReplacementUnverified);
        }
        runtime.provider_session = Some(provider_session);
        runtime.observation_state = RuntimeObservationState::Observed;
        runtime.process_state = RuntimeProcessState::Running;
        runtime.status = observed.status;
        runtime.state_change_sequence = observed.state_change_sequence;
        runtime.revision = observed.revision;
        runtime.last_observed_at_unix_ms = inventory.observed_at_unix_ms;
        Ok(runtime)
    }

    async fn process_cleanup(&self, command_id: &str, result: &mut ReplacedProjectOrchestrator) {
        if !result.cleanup_pending {
            return;
        }
        match self.cleanup.process_command(command_id).await {
            Ok(report) if report.attempted > 0 => {
                result.cleanup_pending = report.failed > 0;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    command_id,
                    error = %error,
                    "Orchestrator replacement committed with cleanup pending"
                );
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum OrchestratorReplacementServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::OrchestratorReplacementValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("the project-bound runtime workspace '{0}' is not currently observed")]
    RuntimeWorkspaceMissing(String),
    #[error("the project workspace has no usable working directory")]
    RuntimeCwdUnavailable,
    #[error("the displaced orchestrator provider or topology identity changed")]
    RuntimeIdentityChanged,
    #[error("Herdr replacement provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("the prepared replacement runtime failed fresh provider/topology verification")]
    ReplacementUnverified,
    #[error("the replacement worker was created but objective delivery failed: {0}")]
    ObjectiveDeliveryFailed(String),
}

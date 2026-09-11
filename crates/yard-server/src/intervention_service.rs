use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use yard_domain::{
    Assignment, AssignmentLifecycle, AttemptLifecycle, AutomaticSummaryRequestKind,
    OrchestratorPromptAcknowledgement, OrchestratorStatusReport, OrchestratorTerminalOutput,
    Project, PromptAcknowledgement, SendAssignmentPrompt, SendOrchestratorPrompt,
    SendYardOrchestratorPrompt, SendYardOrchestratorRoute, TerminalOutput, Worker,
    WorkerRuntimeBinding, YARD_STANDARD_ORCHESTRATOR_PROFILE_ID, YardOrchestrator,
    YardOrchestratorPromptAcknowledgement, YardOrchestratorRoute, YardOrchestratorTerminalOutput,
};
use yard_store::{
    BeginAssignmentPrompt, BeginOrchestratorPrompt, BeginYardOrchestratorPrompt,
    BeginYardOrchestratorRoute, ProjectStoreError, TokenSpendCommandSource, YardStore,
};

use crate::inventory_service::{InventoryServiceError, InventorySource};
use crate::status_protocol::{
    validate_executable_orchestrator_workflow, with_orchestrator_status_contract,
    with_orchestrator_workflow,
};

pub(crate) const MAX_TERMINAL_OUTPUT_LINES: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePromptRequest {
    pub command_id: String,
    pub session: String,
    pub pane_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePromptResult {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutputRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutputResult {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub source: String,
    pub format: String,
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
}

#[async_trait]
pub trait RuntimeIntervention: Send + Sync {
    async fn prompt(
        &self,
        request: RuntimePromptRequest,
    ) -> Result<RuntimePromptResult, RuntimeInterventionError>;

    async fn read_output(
        &self,
        request: RuntimeOutputRequest,
    ) -> Result<RuntimeOutputResult, RuntimeInterventionError>;
}

#[derive(Clone)]
pub struct InterventionService {
    source: Arc<dyn InventorySource>,
    runtime: Arc<dyn RuntimeIntervention>,
    store: Arc<dyn YardStore>,
}

#[derive(Clone, Copy)]
enum AutomaticTokenSpendPolicy {
    Summary(AutomaticSummaryRequestKind),
    ScheduledSummary,
}

impl AutomaticTokenSpendPolicy {
    const fn command_source(self) -> TokenSpendCommandSource {
        match self {
            Self::Summary(AutomaticSummaryRequestKind::SuperintendentProject) => {
                TokenSpendCommandSource::SuperintendentProjectSummary
            }
            Self::Summary(AutomaticSummaryRequestKind::ProjectWorker) => {
                TokenSpendCommandSource::ProjectWorkerSummary
            }
            Self::ScheduledSummary => TokenSpendCommandSource::ScheduledSummary,
        }
    }
}

impl InterventionService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeIntervention>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        Self {
            source,
            runtime,
            store,
        }
    }

    /// Submit one direct prompt to the current runtime binding.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when the command is invalid or
    /// stale, the assignment binding is missing, or Herdr rejects delivery.
    pub async fn prompt(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: SendAssignmentPrompt,
    ) -> Result<PromptAcknowledgement, InterventionServiceError> {
        self.prompt_with_automatic_policy(project_id, assignment_id, command, None)
            .await
    }

    pub(crate) async fn request_automatic_worker_summary(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: SendAssignmentPrompt,
    ) -> Result<PromptAcknowledgement, InterventionServiceError> {
        self.prompt_with_automatic_policy(
            project_id,
            assignment_id,
            command,
            Some(AutomaticTokenSpendPolicy::Summary(
                AutomaticSummaryRequestKind::ProjectWorker,
            )),
        )
        .await
    }

    async fn prompt_with_automatic_policy(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: SendAssignmentPrompt,
        automatic_policy: Option<AutomaticTokenSpendPolicy>,
    ) -> Result<PromptAcknowledgement, InterventionServiceError> {
        if let Some(policy) = automatic_policy {
            self.ensure_automatic_token_spend_enabled(policy).await?;
        }
        let (command, assignment) = match self
            .store
            .begin_assignment_prompt(
                project_id,
                assignment_id,
                command,
                automatic_policy.map_or(
                    TokenSpendCommandSource::Manual,
                    AutomaticTokenSpendPolicy::command_source,
                ),
            )
            .await?
        {
            BeginAssignmentPrompt::Replayed(acknowledgement) => return Ok(acknowledgement),
            BeginAssignmentPrompt::Started {
                command,
                assignment,
            } => (command, assignment),
        };
        let runtime = assignment
            .worker
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        if let Err(error) = self.validate_runtime(&assignment).await {
            self.store
                .fail_assignment_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if let Some(policy) = automatic_policy
            && let Err(error) = self.ensure_automatic_token_spend_enabled(policy).await
        {
            self.store
                .fail_assignment_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        let result = self
            .runtime
            .prompt(RuntimePromptRequest {
                command_id: command.command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: command.text,
            })
            .await;
        match result {
            Ok(result) => {
                if let Err(error) = self.validate_runtime(&assignment).await {
                    let error = RuntimeInterventionError::Ambiguous(format!(
                        "Herdr acknowledged the prompt, but the runtime binding changed: {error}"
                    ));
                    self.store
                        .fail_assignment_prompt(&command.command_id, &error.to_string(), true)
                        .await?;
                    return Err(error.into());
                }
                self.store
                    .succeed_assignment_prompt(&command.command_id, &result.status)
                    .await
                    .map_err(Into::into)
            }
            Err(error) => {
                let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                self.store
                    .fail_assignment_prompt(&command.command_id, &error.to_string(), ambiguous)
                    .await?;
                Err(error.into())
            }
        }
    }

    /// Submit one direct prompt to the project's current orchestrator.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when the command is invalid or
    /// stale, the orchestrator binding is missing, or Herdr rejects delivery.
    pub async fn prompt_orchestrator(
        &self,
        project_id: &str,
        command: SendOrchestratorPrompt,
    ) -> Result<OrchestratorPromptAcknowledgement, InterventionServiceError> {
        self.prompt_orchestrator_with_automatic_policy(project_id, command, None)
            .await
    }

    pub(crate) async fn prompt_orchestrator_for_scheduled_summary(
        &self,
        project_id: &str,
        command: SendOrchestratorPrompt,
    ) -> Result<OrchestratorPromptAcknowledgement, InterventionServiceError> {
        self.prompt_orchestrator_with_automatic_policy(
            project_id,
            command,
            Some(AutomaticTokenSpendPolicy::ScheduledSummary),
        )
        .await
    }

    async fn prompt_orchestrator_with_automatic_policy(
        &self,
        project_id: &str,
        command: SendOrchestratorPrompt,
        automatic_policy: Option<AutomaticTokenSpendPolicy>,
    ) -> Result<OrchestratorPromptAcknowledgement, InterventionServiceError> {
        if let Some(policy) = automatic_policy {
            self.ensure_automatic_token_spend_enabled(policy).await?;
        }
        let initial_project = self.store.get_project(project_id).await?;
        let initial_workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                &initial_project.workflow_profile.profile_id,
                initial_project.workflow_profile.profile_version,
            )
            .await?;
        validate_executable_orchestrator_workflow(&initial_workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let (command, project) = match self
            .store
            .begin_orchestrator_prompt(
                project_id,
                command,
                automatic_policy.map_or(
                    TokenSpendCommandSource::Manual,
                    AutomaticTokenSpendPolicy::command_source,
                ),
            )
            .await?
        {
            BeginOrchestratorPrompt::Replayed(acknowledgement) => return Ok(acknowledgement),
            BeginOrchestratorPrompt::Started { command, project } => (command, project),
        };
        let runtime = project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        if let Err(error) = self.validate_orchestrator_binding(&project).await {
            self.store
                .fail_orchestrator_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if let Some(policy) = automatic_policy
            && let Err(error) = self.ensure_automatic_token_spend_enabled(policy).await
        {
            self.store
                .fail_orchestrator_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        let workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                &project.workflow_profile.profile_id,
                project.workflow_profile.profile_version,
            )
            .await?;
        let prompt = with_orchestrator_workflow(&command.text, &workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let result = self
            .runtime
            .prompt(RuntimePromptRequest {
                command_id: command.command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: with_orchestrator_status_contract(&prompt, &command.command_id),
            })
            .await;
        match result {
            Ok(result) => {
                if let Err(error) = self.validate_orchestrator_binding(&project).await {
                    let error = RuntimeInterventionError::Ambiguous(format!(
                        "Herdr acknowledged the orchestrator prompt, but the runtime binding changed: {error}"
                    ));
                    self.store
                        .fail_orchestrator_prompt(&command.command_id, &error.to_string(), true)
                        .await?;
                    return Err(error.into());
                }
                self.store
                    .succeed_orchestrator_prompt(&command.command_id, &result.status)
                    .await
                    .map_err(Into::into)
            }
            Err(error) => {
                let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                self.store
                    .fail_orchestrator_prompt(&command.command_id, &error.to_string(), ambiguous)
                    .await?;
                Err(error.into())
            }
        }
    }

    /// Submit one direct prompt to the current Yard orchestrator.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when the command is invalid or
    /// stale, the configured binding is missing, or Herdr rejects delivery.
    pub async fn prompt_yard_orchestrator(
        &self,
        command: SendYardOrchestratorPrompt,
    ) -> Result<YardOrchestratorPromptAcknowledgement, InterventionServiceError> {
        self.prompt_yard_orchestrator_with_automatic_policy(command, None)
            .await
    }

    pub(crate) async fn prompt_yard_orchestrator_for_scheduled_summary(
        &self,
        command: SendYardOrchestratorPrompt,
    ) -> Result<YardOrchestratorPromptAcknowledgement, InterventionServiceError> {
        self.prompt_yard_orchestrator_with_automatic_policy(
            command,
            Some(AutomaticTokenSpendPolicy::ScheduledSummary),
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn prompt_yard_orchestrator_with_automatic_policy(
        &self,
        command: SendYardOrchestratorPrompt,
        automatic_policy: Option<AutomaticTokenSpendPolicy>,
    ) -> Result<YardOrchestratorPromptAcknowledgement, InterventionServiceError> {
        if let Some(policy) = automatic_policy {
            self.ensure_automatic_token_spend_enabled(policy).await?;
        }
        let initial_orchestrator = self.store.get_yard_orchestrator().await?;
        let initial_workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                initial_orchestrator.workflow_profile_version,
            )
            .await?;
        validate_executable_orchestrator_workflow(&initial_workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let (command, orchestrator) = match self
            .store
            .begin_yard_orchestrator_prompt(
                command,
                automatic_policy.map_or(
                    TokenSpendCommandSource::Manual,
                    AutomaticTokenSpendPolicy::command_source,
                ),
            )
            .await?
        {
            BeginYardOrchestratorPrompt::Replayed(acknowledgement) => {
                return Ok(acknowledgement);
            }
            BeginYardOrchestratorPrompt::Started {
                command,
                orchestrator,
            } => (command, orchestrator),
        };
        let runtime = orchestrator
            .worker
            .as_ref()
            .and_then(|worker| worker.runtime.as_ref())
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        if let Err(error) = self.validate_yard_orchestrator_binding(&orchestrator).await {
            self.store
                .fail_yard_orchestrator_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if let Some(policy) = automatic_policy
            && let Err(error) = self.ensure_automatic_token_spend_enabled(policy).await
        {
            self.store
                .fail_yard_orchestrator_prompt(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        let workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                orchestrator.workflow_profile_version,
            )
            .await?;
        let prompt = with_orchestrator_workflow(&command.text, &workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let result = self
            .runtime
            .prompt(RuntimePromptRequest {
                command_id: command.command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: with_orchestrator_status_contract(&prompt, &command.command_id),
            })
            .await;
        match result {
            Ok(result) => {
                if let Err(error) = self.validate_yard_orchestrator_binding(&orchestrator).await {
                    let error = RuntimeInterventionError::Ambiguous(format!(
                        "Herdr acknowledged the Yard orchestrator prompt, but the runtime binding changed: {error}"
                    ));
                    self.store
                        .fail_yard_orchestrator_prompt(
                            &command.command_id,
                            &error.to_string(),
                            true,
                        )
                        .await?;
                    return Err(error.into());
                }
                self.store
                    .succeed_yard_orchestrator_prompt(&command.command_id, &result.status)
                    .await
                    .map_err(Into::into)
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
                Err(error.into())
            }
        }
    }

    /// Route one durable command from Yard scope to a project's current
    /// orchestrator runtime.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when either orchestrator identity
    /// is stale, the target binding is missing, or Herdr rejects delivery.
    pub async fn route_yard_orchestrator(
        &self,
        command: SendYardOrchestratorRoute,
    ) -> Result<YardOrchestratorRoute, InterventionServiceError> {
        self.route_yard_orchestrator_with_automatic_policy(command, None)
            .await
    }

    pub(crate) async fn request_automatic_project_summary(
        &self,
        command: SendYardOrchestratorRoute,
    ) -> Result<YardOrchestratorRoute, InterventionServiceError> {
        self.route_yard_orchestrator_with_automatic_policy(
            command,
            Some(AutomaticTokenSpendPolicy::Summary(
                AutomaticSummaryRequestKind::SuperintendentProject,
            )),
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn route_yard_orchestrator_with_automatic_policy(
        &self,
        command: SendYardOrchestratorRoute,
        automatic_policy: Option<AutomaticTokenSpendPolicy>,
    ) -> Result<YardOrchestratorRoute, InterventionServiceError> {
        if let Some(policy) = automatic_policy {
            self.ensure_automatic_token_spend_enabled(policy).await?;
        }
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
        let (command, orchestrator, target_project) = match self
            .store
            .begin_yard_orchestrator_route(
                command,
                automatic_policy.map_or(
                    TokenSpendCommandSource::Manual,
                    AutomaticTokenSpendPolicy::command_source,
                ),
            )
            .await?
        {
            BeginYardOrchestratorRoute::Replayed(route) => return Ok(route),
            BeginYardOrchestratorRoute::Started {
                command,
                orchestrator,
                target_project,
            } => (command, orchestrator, target_project),
        };
        let runtime = target_project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        if let Err(error) = self
            .validate_yard_route_bindings(&orchestrator, &target_project)
            .await
        {
            self.store
                .fail_yard_orchestrator_route(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        if let Some(policy) = automatic_policy
            && let Err(error) = self.ensure_automatic_token_spend_enabled(policy).await
        {
            self.store
                .fail_yard_orchestrator_route(&command.command_id, &error.to_string(), false)
                .await?;
            return Err(error);
        }
        let workflow = self
            .store
            .get_orchestrator_workflow_profile_revision(
                &target_project.workflow_profile.profile_id,
                target_project.workflow_profile.profile_version,
            )
            .await?;
        let prompt = with_orchestrator_workflow(&command.text, &workflow)
            .map_err(ProjectStoreError::InvalidOrchestratorWorkflowProfile)?;
        let result = self
            .runtime
            .prompt(RuntimePromptRequest {
                command_id: command.command_id.clone(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                text: with_orchestrator_status_contract(&prompt, &command.command_id),
            })
            .await;
        match result {
            Ok(result) => {
                if let Err(error) = self
                    .validate_yard_route_bindings(&orchestrator, &target_project)
                    .await
                {
                    let error = RuntimeInterventionError::Ambiguous(format!(
                        "Herdr acknowledged the routed prompt, but an orchestrator binding changed: {error}"
                    ));
                    self.store
                        .fail_yard_orchestrator_route(&command.command_id, &error.to_string(), true)
                        .await?;
                    return Err(error.into());
                }
                self.store
                    .succeed_yard_orchestrator_route(&command.command_id, &result.status)
                    .await
                    .map_err(Into::into)
            }
            Err(error) => {
                let ambiguous = matches!(error, RuntimeInterventionError::Ambiguous(_));
                self.store
                    .fail_yard_orchestrator_route(
                        &command.command_id,
                        &error.to_string(),
                        ambiguous,
                    )
                    .await?;
                Err(error.into())
            }
        }
    }

    async fn ensure_automatic_token_spend_enabled(
        &self,
        policy: AutomaticTokenSpendPolicy,
    ) -> Result<(), InterventionServiceError> {
        let settings = self.store.get_token_spend_settings().await?;
        let enabled = match policy {
            AutomaticTokenSpendPolicy::Summary(kind) => settings.automatic_summary_enabled(kind),
            AutomaticTokenSpendPolicy::ScheduledSummary => settings.scheduled_automatic_summaries,
        };
        if enabled {
            Ok(())
        } else {
            Err(InterventionServiceError::AutomaticTokenSpendDisabled)
        }
    }

    /// Read bounded recent output from the current runtime binding.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when the assignment or binding is
    /// missing or Herdr cannot read the pane.
    pub async fn read_output(
        &self,
        project_id: &str,
        assignment_id: &str,
        lines: u32,
    ) -> Result<TerminalOutput, InterventionServiceError> {
        if !(1..=MAX_TERMINAL_OUTPUT_LINES).contains(&lines) {
            return Err(InterventionServiceError::InvalidLineCount);
        }
        let assignment = self.assignment(project_id, assignment_id).await?;
        if assignment.lifecycle != AssignmentLifecycle::Active
            || assignment.attempt.lifecycle != AttemptLifecycle::Active
        {
            return Err(InterventionServiceError::AssignmentNotActive);
        }
        self.validate_runtime(&assignment).await?;
        let runtime = assignment
            .worker
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        let result = self
            .runtime
            .read_output(RuntimeOutputRequest {
                request_id: uuid::Uuid::now_v7().to_string(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                lines,
            })
            .await?;
        if result.pane_id != runtime.pane_id
            || result.workspace_id != runtime.workspace_id
            || result.tab_id != runtime.tab_id.as_deref().unwrap_or_default()
        {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        self.validate_runtime(&assignment).await?;
        Ok(TerminalOutput {
            assignment_id: assignment.id,
            attempt_id: assignment.attempt.id,
            pane_id: result.pane_id,
            source: result.source,
            format: result.format,
            text: result.text,
            revision: result.revision,
            truncated: result.truncated,
        })
    }

    /// Read bounded recent output from the current project orchestrator.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when the project or orchestrator
    /// binding is missing, changes during the read, or Herdr cannot read it.
    pub async fn read_orchestrator_output(
        &self,
        project_id: &str,
        lines: u32,
    ) -> Result<OrchestratorTerminalOutput, InterventionServiceError> {
        if !(1..=MAX_TERMINAL_OUTPUT_LINES).contains(&lines) {
            return Err(InterventionServiceError::InvalidLineCount);
        }
        let project = self.project(project_id).await?;
        self.validate_orchestrator_binding(&project).await?;
        let runtime = project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        let result = self
            .runtime
            .read_output(RuntimeOutputRequest {
                request_id: uuid::Uuid::now_v7().to_string(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                lines,
            })
            .await?;
        if result.pane_id != runtime.pane_id
            || result.workspace_id != runtime.workspace_id
            || result.tab_id != runtime.tab_id.as_deref().unwrap_or_default()
        {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        self.validate_orchestrator_binding(&project).await?;
        let expected_command_id = self
            .store
            .latest_delivered_project_orchestrator_command_id(&project.id)
            .await?;
        let status_report = expected_command_id.as_deref().and_then(|command_id| {
            OrchestratorStatusReport::scan_terminal_output_for_command(&result.text, command_id)
        });
        Ok(OrchestratorTerminalOutput {
            project_id: project.id,
            worker_id: project.orchestrator.id,
            pane_id: result.pane_id,
            source: result.source,
            format: result.format,
            text: result.text,
            revision: result.revision,
            truncated: result.truncated,
            status_report,
        })
    }

    /// Read bounded recent output from the current Yard orchestrator.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionServiceError`] when the Yard orchestrator is
    /// unconfigured, its binding changes, or Herdr cannot read it.
    pub async fn read_yard_orchestrator_output(
        &self,
        lines: u32,
    ) -> Result<YardOrchestratorTerminalOutput, InterventionServiceError> {
        if !(1..=MAX_TERMINAL_OUTPUT_LINES).contains(&lines) {
            return Err(InterventionServiceError::InvalidLineCount);
        }
        let orchestrator = self.store.get_yard_orchestrator().await?;
        let current = self
            .validate_yard_orchestrator_binding(&orchestrator)
            .await?;
        let worker = current
            .worker
            .as_ref()
            .ok_or(ProjectStoreError::YardOrchestratorNotConfigured)?;
        let runtime = worker
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        let result = self
            .runtime
            .read_output(RuntimeOutputRequest {
                request_id: uuid::Uuid::now_v7().to_string(),
                session: runtime.session.clone(),
                pane_id: runtime.pane_id.clone(),
                lines,
            })
            .await?;
        if result.pane_id != runtime.pane_id
            || result.workspace_id != runtime.workspace_id
            || result.tab_id != runtime.tab_id.as_deref().unwrap_or_default()
        {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        self.validate_yard_orchestrator_binding(&current).await?;
        let expected_command_id = self
            .store
            .latest_delivered_yard_orchestrator_command_id()
            .await?;
        let status_report = expected_command_id.as_deref().and_then(|command_id| {
            OrchestratorStatusReport::scan_terminal_output_for_command(&result.text, command_id)
        });
        Ok(YardOrchestratorTerminalOutput {
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

    pub(crate) async fn yard_orchestrator(
        &self,
    ) -> Result<YardOrchestrator, InterventionServiceError> {
        self.store.get_yard_orchestrator().await.map_err(Into::into)
    }

    pub(crate) async fn validate_runtime(
        &self,
        assignment: &Assignment,
    ) -> Result<(), InterventionServiceError> {
        self.validate_worker_runtime(&assignment.worker).await
    }

    pub(crate) async fn validate_worker_runtime(
        &self,
        worker: &Worker,
    ) -> Result<(), InterventionServiceError> {
        let runtime = worker
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        let inventory = self.source.inventory(&runtime.session).await?;
        if inventory.adapter != runtime.adapter || inventory.session != runtime.session {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        let observed = inventory
            .workers
            .iter()
            .find(|worker| worker.terminal_id == runtime.terminal_id)
            .ok_or(InterventionServiceError::RuntimeBindingStale)?;
        if observed.workspace_id != runtime.workspace_id
            || observed.pane_id != runtime.pane_id
            || observed.tab_id != runtime.tab_id.as_deref().unwrap_or_default()
            || !provider_session_matches(
                runtime.provider_session.as_ref(),
                observed.provider_session.as_ref(),
            )
        {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        Ok(())
    }

    pub(crate) async fn validate_orchestrator_binding(
        &self,
        expected: &Project,
    ) -> Result<Project, InterventionServiceError> {
        let current = self.project(&expected.id).await?;
        if current.version != expected.version
            || current.workflow_profile.profile_id != expected.workflow_profile.profile_id
            || current.workflow_profile.profile_version != expected.workflow_profile.profile_version
            || current.orchestrator.id != expected.orchestrator.id
        {
            return Err(InterventionServiceError::OrchestratorChanged);
        }
        let expected_runtime = expected
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        let current_runtime = current
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        if !same_runtime_identity(expected_runtime, current_runtime) {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        self.validate_worker_runtime(&current.orchestrator).await?;
        Ok(current)
    }

    pub(crate) async fn validate_yard_orchestrator_binding(
        &self,
        expected: &YardOrchestrator,
    ) -> Result<YardOrchestrator, InterventionServiceError> {
        let current = self.store.get_yard_orchestrator().await?;
        let expected_worker = expected
            .worker
            .as_ref()
            .ok_or(ProjectStoreError::YardOrchestratorNotConfigured)?;
        let current_worker = current
            .worker
            .as_ref()
            .ok_or(ProjectStoreError::YardOrchestratorNotConfigured)?;
        if current.version != expected.version || current_worker.id != expected_worker.id {
            return Err(InterventionServiceError::YardOrchestratorChanged);
        }
        let expected_runtime = expected_worker
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        let current_runtime = current_worker
            .runtime
            .as_ref()
            .ok_or(InterventionServiceError::RuntimeBindingMissing)?;
        if !same_runtime_identity(expected_runtime, current_runtime) {
            return Err(InterventionServiceError::RuntimeBindingStale);
        }
        self.validate_worker_runtime(current_worker).await?;
        Ok(current)
    }

    async fn validate_yard_route_bindings(
        &self,
        orchestrator: &YardOrchestrator,
        target_project: &Project,
    ) -> Result<(), InterventionServiceError> {
        self.validate_yard_orchestrator_binding(orchestrator)
            .await?;
        self.validate_orchestrator_binding(target_project).await?;
        Ok(())
    }

    pub(crate) async fn assignment(
        &self,
        project_id: &str,
        assignment_id: &str,
    ) -> Result<Assignment, InterventionServiceError> {
        self.store
            .list_project_assignments(project_id)
            .await?
            .assignments
            .into_iter()
            .find(|assignment| assignment.id == assignment_id)
            .ok_or(InterventionServiceError::AssignmentNotFound)
    }

    pub(crate) async fn project(
        &self,
        project_id: &str,
    ) -> Result<Project, InterventionServiceError> {
        self.store.get_project(project_id).await.map_err(Into::into)
    }
}

fn same_runtime_identity(left: &WorkerRuntimeBinding, right: &WorkerRuntimeBinding) -> bool {
    left.adapter == right.adapter
        && left.session == right.session
        && left.workspace_id == right.workspace_id
        && left.terminal_id == right.terminal_id
        && left.tab_id == right.tab_id
        && left.pane_id == right.pane_id
        && left.provider_session == right.provider_session
}

fn provider_session_matches(
    expected: Option<&yard_domain::ProviderSessionRef>,
    observed: Option<&yard_domain::ProviderSessionRef>,
) -> bool {
    !matches!(
        (expected, observed),
        (Some(expected), Some(observed)) if expected != observed
    )
}

#[derive(Debug, Error)]
pub enum RuntimeInterventionError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Ambiguous(String),
}

#[derive(Debug, Error)]
pub enum InterventionServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::InterventionValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeInterventionError),
    #[error("assignment was not found in this Yard project")]
    AssignmentNotFound,
    #[error("only the active assignment attempt can receive prompts")]
    AssignmentNotActive,
    #[error("assignment changed concurrently; current version is {current_version}")]
    AssignmentVersionConflict { current_version: u64 },
    #[error("assignment attempt changed concurrently; current version is {current_version}")]
    AttemptVersionConflict { current_version: u64 },
    #[error("assignment attempt is stale; current attempt is {current_attempt_id}")]
    AttemptNotCurrent { current_attempt_id: String },
    #[error("worker has no runtime binding")]
    RuntimeBindingMissing,
    #[error("the Herdr pane no longer matches the worker runtime binding")]
    RuntimeBindingStale,
    #[error("the project's orchestrator changed while the intervention was in progress")]
    OrchestratorChanged,
    #[error("the Yard orchestrator changed while the intervention was in progress")]
    YardOrchestratorChanged,
    #[error("the requested automatic token-spend behavior is disabled")]
    AutomaticTokenSpendDisabled,
    #[error("lines must be between 1 and 10000")]
    InvalidLineCount,
}

#[cfg(test)]
mod tests {
    use yard_domain::ProviderSessionRef;

    use super::provider_session_matches;

    fn session(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn legacy_binding_accepts_missing_or_newly_observed_provider_session() {
        let observed = session("observed");

        assert!(provider_session_matches(None, None));
        assert!(provider_session_matches(None, Some(&observed)));
    }

    #[test]
    fn known_provider_session_accepts_missing_observation_but_rejects_change() {
        let expected = session("expected");
        let observed = session("observed");

        assert!(provider_session_matches(Some(&expected), Some(&expected)));
        assert!(provider_session_matches(Some(&expected), None));
        assert!(!provider_session_matches(Some(&expected), Some(&observed)));
    }
}

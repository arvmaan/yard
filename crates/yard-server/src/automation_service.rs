use std::{sync::Arc, time::Duration};

use chrono::{DateTime, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use thiserror::Error;
use tokio::{sync::Mutex, time::sleep};
use uuid::Uuid;
use yard_domain::{
    Automation, AutomationCommandResult, AutomationRun, AutomationRunCommandResult,
    AutomationRunTrigger, AutomationRuns, AutomationScope, Automations, CreateAutomation,
    DailySchedule, RunAutomationNow, SendCoordinationNodePrompt, SendOrchestratorPrompt,
    SendYardOrchestratorPrompt, SetAutomationPaused, UpdateAutomation, UpdateAutomationPlacement,
    automation_dispatch_command_id,
};
use yard_store::{ProjectStoreError, YardStore};

use crate::{
    coordination_node_service::{CoordinationNodeService, CoordinationNodeServiceError},
    intervention_service::{
        InterventionService, InterventionServiceError, RuntimeInterventionError,
    },
};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);
const SCHEDULER_BATCH_SIZE: usize = 100;
const MAX_RUN_LIST_LIMIT: usize = 500;

#[derive(Clone)]
pub struct AutomationService {
    store: Arc<dyn YardStore>,
    interventions: InterventionService,
    coordination_nodes: CoordinationNodeService,
    operation: Arc<Mutex<()>>,
}

impl AutomationService {
    #[must_use]
    pub fn new(
        store: Arc<dyn YardStore>,
        interventions: InterventionService,
        coordination_nodes: CoordinationNodeService,
    ) -> Self {
        Self {
            store,
            interventions,
            coordination_nodes,
            operation: Arc::default(),
        }
    }

    pub(crate) async fn list(&self) -> Result<Automations, AutomationServiceError> {
        self.store.list_automations().await.map_err(Into::into)
    }

    pub(crate) async fn get(
        &self,
        automation_id: &str,
    ) -> Result<Automation, AutomationServiceError> {
        self.store
            .get_automation(automation_id)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn create(
        &self,
        command: CreateAutomation,
    ) -> Result<AutomationCommandResult, AutomationServiceError> {
        let command = command.normalize()?;
        let next_run_at_unix_ms = next_daily_occurrence(&command.schedule, unix_time_ms()?)?;
        self.store
            .create_automation(command, next_run_at_unix_ms)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn update(
        &self,
        command: UpdateAutomation,
    ) -> Result<AutomationCommandResult, AutomationServiceError> {
        let command = command.normalize()?;
        let current = self.store.get_automation(&command.automation_id).await?;
        let next_run_at_unix_ms = if matches!(current.state, yard_domain::AutomationState::Paused) {
            None
        } else {
            Some(next_daily_occurrence(&command.schedule, unix_time_ms()?)?)
        };
        self.store
            .update_automation(command, next_run_at_unix_ms)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn update_placement(
        &self,
        command: UpdateAutomationPlacement,
    ) -> Result<AutomationCommandResult, AutomationServiceError> {
        self.store
            .update_automation_placement(command)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn set_paused(
        &self,
        command: SetAutomationPaused,
    ) -> Result<AutomationCommandResult, AutomationServiceError> {
        let command = command.normalize()?;
        let next_run_at_unix_ms = if command.paused {
            None
        } else {
            let automation = self.store.get_automation(&command.automation_id).await?;
            Some(next_daily_occurrence(
                &automation.schedule,
                unix_time_ms()?,
            )?)
        };
        self.store
            .set_automation_paused(command, next_run_at_unix_ms)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn list_runs(
        &self,
        automation_id: &str,
        limit: usize,
    ) -> Result<AutomationRuns, AutomationServiceError> {
        if !(1..=MAX_RUN_LIST_LIMIT).contains(&limit) {
            return Err(AutomationServiceError::InvalidRunLimit);
        }
        self.store
            .list_automation_runs(automation_id, limit)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn get_run(
        &self,
        automation_id: &str,
        run_id: &str,
    ) -> Result<AutomationRun, AutomationServiceError> {
        self.store
            .get_automation_run(automation_id, run_id)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn run_now(
        &self,
        command: RunAutomationNow,
    ) -> Result<AutomationRunCommandResult, AutomationServiceError> {
        let _operation = self.operation.lock().await;
        let command = command.normalize()?;
        let run_id = Uuid::now_v7().to_string();
        let dispatch_command_id = automation_dispatch_command_id(&run_id);
        let mut result = self
            .store
            .create_manual_automation_run(command, &run_id, &dispatch_command_id)
            .await?;
        if matches!(result.run.status, yard_domain::AutomationRunStatus::Pending) {
            result.run = self.dispatch(result.run).await?;
        }
        Ok(result)
    }

    /// Poll and dispatch durable automation runs forever.
    pub async fn run(self) {
        loop {
            if let Err(error) = self.run_due_once().await {
                tracing::warn!(%error, "Yard automation scheduler tick failed");
            }
            sleep(SCHEDULER_INTERVAL).await;
        }
    }

    pub(crate) async fn run_due_once(&self) -> Result<(), AutomationServiceError> {
        let _operation = self.operation.lock().await;

        for run in self
            .store
            .list_pending_automation_runs(SCHEDULER_BATCH_SIZE)
            .await?
            .runs
        {
            self.dispatch(run).await?;
        }

        let now = unix_time_ms()?;
        for automation in self
            .store
            .list_due_automations(now, SCHEDULER_BATCH_SIZE)
            .await?
            .automations
        {
            let scheduled_for_unix_ms = automation
                .next_run_at_unix_ms
                .ok_or(AutomationServiceError::MissingNextRun)?;
            let next_run_at_unix_ms = next_daily_occurrence(&automation.schedule, now)?;
            let run_id = Uuid::now_v7().to_string();
            let dispatch_command_id = automation_dispatch_command_id(&run_id);
            let run = self
                .store
                .claim_scheduled_automation_run(
                    &automation.id,
                    scheduled_for_unix_ms,
                    next_run_at_unix_ms,
                    &run_id,
                    &dispatch_command_id,
                    "yard:scheduler",
                )
                .await?;
            self.dispatch(run).await?;
        }
        Ok(())
    }

    async fn dispatch(&self, run: AutomationRun) -> Result<AutomationRun, AutomationServiceError> {
        let automation = self.store.get_automation(&run.automation_id).await?;
        let text = match self.dispatch_prompt(&automation, &run).await {
            Ok(text) => text,
            Err(error) => {
                return self
                    .store
                    .mark_automation_run_failed(&run.id, &error.to_string(), false)
                    .await
                    .map_err(Into::into);
            }
        };
        let delivery = match &run.scope_snapshot {
            AutomationScope::YardOrchestrator => self.deliver_to_superintendent(&run, text).await,
            AutomationScope::ProjectOrchestrator { project_id } => {
                self.deliver_to_project(&run, project_id, text).await
            }
            AutomationScope::WorkstreamCoordinationNode { node_id } => {
                self.deliver_to_workstream(&run, node_id, text).await
            }
        };

        match delivery {
            Ok((runtime_status, submitted_at_unix_ms)) => self
                .store
                .mark_automation_run_submitted(&run.id, &runtime_status, submitted_at_unix_ms)
                .await
                .map_err(Into::into),
            Err(error) => {
                let ambiguous = error.is_ambiguous();
                self.store
                    .mark_automation_run_failed(&run.id, &error.to_string(), ambiguous)
                    .await
                    .map_err(Into::into)
            }
        }
    }

    async fn deliver_to_superintendent(
        &self,
        run: &AutomationRun,
        text: String,
    ) -> Result<(String, u64), DispatchError> {
        let orchestrator = self
            .store
            .get_yard_orchestrator()
            .await
            .map_err(DispatchError::Store)?;
        let worker = orchestrator
            .worker
            .as_ref()
            .ok_or_else(|| DispatchError::Setup("Superintendent is not provisioned".to_owned()))?;
        self.interventions
            .prompt_yard_orchestrator(SendYardOrchestratorPrompt {
                command_id: run.dispatch_command_id.clone(),
                actor: format!("yard:automation:{}", run.automation_id),
                expected_orchestrator_version: orchestrator.version,
                orchestrator_worker_id: worker.id.clone(),
                text,
            })
            .await
            .map(|acknowledgement| {
                (
                    acknowledgement.runtime_status,
                    acknowledgement.submitted_at_unix_ms,
                )
            })
            .map_err(DispatchError::Intervention)
    }

    async fn deliver_to_project(
        &self,
        run: &AutomationRun,
        project_id: &str,
        text: String,
    ) -> Result<(String, u64), DispatchError> {
        let project = self
            .store
            .get_project(project_id)
            .await
            .map_err(DispatchError::Store)?;
        self.interventions
            .prompt_orchestrator(
                project_id,
                SendOrchestratorPrompt {
                    command_id: run.dispatch_command_id.clone(),
                    actor: format!("yard:automation:{}", run.automation_id),
                    expected_project_version: project.version,
                    orchestrator_worker_id: project.orchestrator.id,
                    text,
                },
            )
            .await
            .map(|acknowledgement| {
                (
                    acknowledgement.runtime_status,
                    acknowledgement.submitted_at_unix_ms,
                )
            })
            .map_err(DispatchError::Intervention)
    }

    async fn deliver_to_workstream(
        &self,
        run: &AutomationRun,
        node_id: &str,
        text: String,
    ) -> Result<(String, u64), DispatchError> {
        let node = self
            .store
            .get_coordination_node(node_id)
            .await
            .map_err(DispatchError::Store)?;
        if run
            .selected_project_ids
            .iter()
            .any(|project_id| !node.attached_project_ids.contains(project_id))
        {
            return Err(DispatchError::Setup(
                "Selected project is no longer attached to the workstream".to_owned(),
            ));
        }
        let worker = node
            .worker
            .as_ref()
            .ok_or_else(|| DispatchError::Setup("Workstream is not provisioned".to_owned()))?;
        self.coordination_nodes
            .prompt(
                node_id,
                SendCoordinationNodePrompt {
                    command_id: run.dispatch_command_id.clone(),
                    actor: format!("yard:automation:{}", run.automation_id),
                    expected_node_version: node.version,
                    worker_id: worker.id.clone(),
                    text,
                },
            )
            .await
            .map(|acknowledgement| {
                (
                    acknowledgement.runtime_status,
                    acknowledgement.submitted_at_unix_ms,
                )
            })
            .map_err(DispatchError::Coordination)
    }

    async fn dispatch_prompt(
        &self,
        automation: &Automation,
        run: &AutomationRun,
    ) -> Result<String, AutomationServiceError> {
        let mut projects = Vec::with_capacity(run.selected_project_ids.len());
        for project_id in &run.selected_project_ids {
            let project = self.store.get_project(project_id).await?;
            projects.push(format!("- {} ({})", project.name, project.id));
        }
        let projects = if projects.is_empty() {
            "- No projects selected; use only the target orchestrator's scope.".to_owned()
        } else {
            projects.join("\n")
        };
        Ok(format!(
            "YARD AUTOMATION RUN\n\
             Automation: {} ({})\n\
             Run: {}\n\
             Trigger: {}\n\
             Selected projects:\n{}\n\n\
             Task:\n{}\n\n\
             Treat this as a recurring Yard dispatch. Report what you observed and submitted, \
             identify any owner action needed, and do not infer completion from runtime state.",
            automation.name,
            automation.id,
            run.id,
            match run.trigger {
                AutomationRunTrigger::Manual => "manual",
                AutomationRunTrigger::Scheduled => "scheduled",
            },
            projects,
            run.prompt_template,
        ))
    }
}

fn next_daily_occurrence(
    schedule: &DailySchedule,
    after_unix_ms: u64,
) -> Result<u64, AutomationServiceError> {
    let timezone = schedule
        .timezone
        .parse::<Tz>()
        .map_err(|_| AutomationServiceError::InvalidTimezone)?;
    let after_millis =
        i64::try_from(after_unix_ms).map_err(|_| AutomationServiceError::InvalidTimestamp)?;
    let after = DateTime::<Utc>::from_timestamp_millis(after_millis)
        .ok_or(AutomationServiceError::InvalidTimestamp)?;
    let mut date = after.with_timezone(&timezone).date_naive();

    for _ in 0..=370 {
        if let Some(candidate) = local_schedule_on_date(timezone, date, schedule)
            && candidate > after_unix_ms
        {
            return Ok(candidate);
        }
        date = date
            .succ_opt()
            .ok_or(AutomationServiceError::InvalidTimestamp)?;
    }
    Err(AutomationServiceError::ScheduleUnavailable)
}

fn local_schedule_on_date(timezone: Tz, date: NaiveDate, schedule: &DailySchedule) -> Option<u64> {
    let local = date.and_hms_opt(u32::from(schedule.hour), u32::from(schedule.minute), 0)?;
    let candidate = match timezone.from_local_datetime(&local) {
        LocalResult::Single(candidate) => candidate,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => return None,
    };
    u64::try_from(candidate.timestamp_millis()).ok()
}

fn unix_time_ms() -> Result<u64, AutomationServiceError> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AutomationServiceError::InvalidTimestamp)?
        .as_millis();
    u64::try_from(millis).map_err(|_| AutomationServiceError::InvalidTimestamp)
}

enum DispatchError {
    Intervention(InterventionServiceError),
    Coordination(CoordinationNodeServiceError),
    Store(ProjectStoreError),
    Setup(String),
}

impl DispatchError {
    fn is_ambiguous(&self) -> bool {
        match self {
            Self::Intervention(InterventionServiceError::Runtime(
                RuntimeInterventionError::Ambiguous(_),
            ))
            | Self::Coordination(CoordinationNodeServiceError::Runtime(
                RuntimeInterventionError::Ambiguous(_),
            )) => true,
            Self::Intervention(_) | Self::Coordination(_) | Self::Store(_) | Self::Setup(_) => {
                false
            }
        }
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Intervention(error) => error.fmt(formatter),
            Self::Coordination(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Setup(error) => formatter.write_str(error),
        }
    }
}

#[derive(Debug, Error)]
pub enum AutomationServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::AutomationValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("automation schedule timezone is invalid")]
    InvalidTimezone,
    #[error("automation schedule timestamp is outside the supported range")]
    InvalidTimestamp,
    #[error("no valid daily schedule occurrence was found")]
    ScheduleUnavailable,
    #[error("active automation is missing its next run")]
    MissingNextRun,
    #[error("automation target orchestrator has not been provisioned")]
    ScopeNotProvisioned,
    #[error("run history limit must be between 1 and {MAX_RUN_LIST_LIMIT}")]
    InvalidRunLimit,
}

#[cfg(test)]
mod tests {
    use super::next_daily_occurrence;
    use yard_domain::DailySchedule;

    #[test]
    fn next_occurrence_honors_timezone_and_dst_gap() {
        let schedule = DailySchedule {
            hour: 2,
            minute: 30,
            timezone: "America/New_York".to_owned(),
        };
        // 2026-03-08 01:59 EST. The 02:30 wall time does not exist that day.
        let after = 1_772_953_140_000;
        let next = next_daily_occurrence(&schedule, after).unwrap();
        assert!(next > after);
        assert_eq!(next, 1_773_037_800_000);
    }

    #[test]
    fn next_occurrence_chooses_earlier_dst_overlap() {
        let schedule = DailySchedule {
            hour: 1,
            minute: 30,
            timezone: "America/New_York".to_owned(),
        };
        // Before the first 01:30 occurrence on the 2026 fall-back day.
        let after = 1_793_508_600_000;
        assert_eq!(
            next_daily_occurrence(&schedule, after).unwrap(),
            1_793_511_000_000
        );
    }
}

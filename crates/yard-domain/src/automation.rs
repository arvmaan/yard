use std::collections::HashSet;

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::CanvasPlacement;

const MAX_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_NAME_BYTES: usize = 120;
const MAX_PROMPT_BYTES: usize = 16_000;
const MAX_SELECTED_PROJECTS: usize = 256;
const MAX_AUTOMATION_SIZE: f64 = 2_400.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationScope {
    YardOrchestrator,
    ProjectOrchestrator { project_id: String },
    WorkstreamCoordinationNode { node_id: String },
}

impl AutomationScope {
    fn normalize(self) -> Result<Self, AutomationValidationError> {
        match self {
            Self::YardOrchestrator => Ok(Self::YardOrchestrator),
            Self::ProjectOrchestrator { project_id } => Ok(Self::ProjectOrchestrator {
                project_id: canonical_uuid("scope.project_id", &project_id)?,
            }),
            Self::WorkstreamCoordinationNode { node_id } => Ok(Self::WorkstreamCoordinationNode {
                node_id: canonical_uuid("scope.node_id", &node_id)?,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AutomationPlacement {
    pub geometry: CanvasPlacement,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    #[serde(with = "crate::serde_u64")]
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailySchedule {
    pub hour: u8,
    pub minute: u8,
    pub timezone: String,
}

impl DailySchedule {
    /// Normalize an IANA timezone and validate a daily wall-clock time.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationValidationError`] when the hour, minute, or IANA
    /// timezone is invalid.
    pub fn normalize(mut self) -> Result<Self, AutomationValidationError> {
        if self.hour > 23 {
            return Err(AutomationValidationError::InvalidScheduleHour);
        }
        if self.minute > 59 {
            return Err(AutomationValidationError::InvalidScheduleMinute);
        }
        let timezone = self.timezone.trim();
        let timezone = timezone
            .parse::<Tz>()
            .map_err(|_| AutomationValidationError::InvalidTimezone)?;
        self.timezone = timezone.to_string();
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationState {
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    Pending,
    Submitted,
    Failed,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunTrigger {
    Manual,
    Scheduled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub scope: AutomationScope,
    pub placement: AutomationPlacement,
    pub schedule: DailySchedule,
    pub selected_project_ids: Vec<String>,
    pub prompt_template: String,
    pub state: AutomationState,
    #[serde(default, with = "crate::serde_u64::option")]
    pub next_run_at_unix_ms: Option<u64>,
    pub latest_run: Option<AutomationRun>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_by: String,
    #[serde(with = "crate::serde_u64")]
    pub created_at_unix_ms: u64,
    #[serde(with = "crate::serde_u64")]
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automations {
    pub automations: Vec<Automation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationRun {
    pub id: String,
    pub automation_id: String,
    pub scope_snapshot: AutomationScope,
    #[serde(with = "crate::serde_u64")]
    pub automation_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub trigger: AutomationRunTrigger,
    pub status: AutomationRunStatus,
    pub prompt_template: String,
    pub selected_project_ids: Vec<String>,
    pub dispatch_command_id: String,
    pub requested_by: String,
    #[serde(default, with = "crate::serde_u64::option")]
    pub scheduled_for_unix_ms: Option<u64>,
    pub runtime_status: Option<String>,
    pub error_message: Option<String>,
    #[serde(default, with = "crate::serde_u64::option")]
    pub submitted_at_unix_ms: Option<u64>,
    #[serde(with = "crate::serde_u64")]
    pub created_at_unix_ms: u64,
    #[serde(with = "crate::serde_u64")]
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationRuns {
    pub runs: Vec<AutomationRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateAutomation {
    pub command_id: String,
    pub actor: String,
    #[serde(default)]
    pub automation_id: String,
    pub name: String,
    pub scope: AutomationScope,
    pub placement: CanvasPlacement,
    pub schedule: DailySchedule,
    #[serde(default)]
    pub selected_project_ids: Vec<String>,
    pub prompt_template: String,
}

impl CreateAutomation {
    /// Normalize and validate an automation creation command.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationValidationError`] for invalid identifiers, text,
    /// scope, placement, schedule, or project selection.
    pub fn normalize(mut self) -> Result<Self, AutomationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.automation_id = canonical_uuid("automation_id", &self.automation_id)?;
        self.name = required("name", &self.name, MAX_NAME_BYTES)?;
        self.scope = self.scope.normalize()?;
        validate_placement(&self.placement)?;
        self.schedule = self.schedule.normalize()?;
        self.selected_project_ids = normalize_project_ids(self.selected_project_ids)?;
        validate_project_scope_selection(&self.scope, &self.selected_project_ids)?;
        self.prompt_template =
            required("prompt_template", &self.prompt_template, MAX_PROMPT_BYTES)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateAutomation {
    pub command_id: String,
    pub actor: String,
    #[serde(default)]
    pub automation_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub name: String,
    pub scope: AutomationScope,
    pub schedule: DailySchedule,
    #[serde(default)]
    pub selected_project_ids: Vec<String>,
    pub prompt_template: String,
}

impl UpdateAutomation {
    /// Normalize and validate a versioned automation configuration update.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationValidationError`] for invalid identifiers, text,
    /// version, scope, schedule, or project selection.
    pub fn normalize(mut self) -> Result<Self, AutomationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.automation_id = canonical_uuid("automation_id", &self.automation_id)?;
        require_version(self.expected_version)?;
        self.name = required("name", &self.name, MAX_NAME_BYTES)?;
        self.scope = self.scope.normalize()?;
        self.schedule = self.schedule.normalize()?;
        self.selected_project_ids = normalize_project_ids(self.selected_project_ids)?;
        validate_project_scope_selection(&self.scope, &self.selected_project_ids)?;
        self.prompt_template =
            required("prompt_template", &self.prompt_template, MAX_PROMPT_BYTES)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateAutomationPlacement {
    pub command_id: String,
    pub actor: String,
    #[serde(default)]
    pub automation_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub placement: CanvasPlacement,
}

impl UpdateAutomationPlacement {
    /// Normalize and validate an automation placement update.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationValidationError`] for invalid identifiers, version,
    /// or geometry.
    pub fn normalize(mut self) -> Result<Self, AutomationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.automation_id = canonical_uuid("automation_id", &self.automation_id)?;
        require_version(self.expected_version)?;
        validate_placement(&self.placement)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetAutomationPaused {
    pub command_id: String,
    pub actor: String,
    #[serde(default)]
    pub automation_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub paused: bool,
}

impl SetAutomationPaused {
    /// Normalize and validate a pause or resume command.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationValidationError`] for invalid identifiers or a zero
    /// optimistic version.
    pub fn normalize(mut self) -> Result<Self, AutomationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.automation_id = canonical_uuid("automation_id", &self.automation_id)?;
        require_version(self.expected_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAutomationNow {
    pub command_id: String,
    pub actor: String,
    #[serde(default)]
    pub automation_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

impl RunAutomationNow {
    /// Normalize and validate a manual automation run command.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationValidationError`] for invalid identifiers or actor.
    pub fn normalize(mut self) -> Result<Self, AutomationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.automation_id = canonical_uuid("automation_id", &self.automation_id)?;
        require_version(self.expected_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationCommandResult {
    pub command_id: String,
    pub automation: Automation,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationRunCommandResult {
    pub command_id: String,
    pub run: AutomationRun,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AutomationValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("{field} must be a canonical UUID")]
    InvalidUuid { field: &'static str },
    #[error("selected_project_ids contains a duplicate project")]
    DuplicateProject,
    #[error("selected_project_ids contains more than {max} projects")]
    TooManyProjects { max: usize },
    #[error("a project-orchestrator automation must select exactly its scoped project")]
    ProjectScopeSelectionMismatch,
    #[error("optimistic versions must be greater than zero")]
    InvalidVersion,
    #[error("schedule hour must be between 0 and 23")]
    InvalidScheduleHour,
    #[error("schedule minute must be between 0 and 59")]
    InvalidScheduleMinute,
    #[error("schedule timezone must be a valid IANA timezone")]
    InvalidTimezone,
    #[error("automation placement coordinates must be finite numbers")]
    InvalidPlacementPosition,
    #[error("automation placement dimensions must be finite numbers between 0 and 2400")]
    InvalidPlacementSize,
}

/// Validate and canonicalize an automation UUID.
///
/// # Errors
///
/// Returns [`AutomationValidationError`] unless the value is a lowercase,
/// hyphenated canonical UUID.
pub fn canonical_automation_uuid(
    field: &'static str,
    value: &str,
) -> Result<String, AutomationValidationError> {
    canonical_uuid(field, value)
}

#[must_use]
pub fn automation_dispatch_command_id(run_id: &str) -> String {
    format!("automation-run:{run_id}")
}

fn validate_placement(placement: &CanvasPlacement) -> Result<(), AutomationValidationError> {
    if !placement.x.is_finite() || !placement.y.is_finite() {
        return Err(AutomationValidationError::InvalidPlacementPosition);
    }
    if !placement.width.is_finite()
        || !placement.height.is_finite()
        || placement.width <= 0.0
        || placement.height <= 0.0
        || placement.width > MAX_AUTOMATION_SIZE
        || placement.height > MAX_AUTOMATION_SIZE
    {
        return Err(AutomationValidationError::InvalidPlacementSize);
    }
    Ok(())
}

fn validate_project_scope_selection(
    scope: &AutomationScope,
    selected_project_ids: &[String],
) -> Result<(), AutomationValidationError> {
    if let AutomationScope::ProjectOrchestrator { project_id } = scope
        && (selected_project_ids.len() != 1 || selected_project_ids[0] != *project_id)
    {
        return Err(AutomationValidationError::ProjectScopeSelectionMismatch);
    }
    Ok(())
}

fn normalize_project_ids(
    mut project_ids: Vec<String>,
) -> Result<Vec<String>, AutomationValidationError> {
    if project_ids.len() > MAX_SELECTED_PROJECTS {
        return Err(AutomationValidationError::TooManyProjects {
            max: MAX_SELECTED_PROJECTS,
        });
    }
    let mut seen = HashSet::with_capacity(project_ids.len());
    for project_id in &mut project_ids {
        *project_id = canonical_uuid("selected_project_ids", project_id)?;
        if !seen.insert(project_id.clone()) {
            return Err(AutomationValidationError::DuplicateProject);
        }
    }
    project_ids.sort_unstable();
    Ok(project_ids)
}

fn canonical_uuid(field: &'static str, value: &str) -> Result<String, AutomationValidationError> {
    let value = value.trim();
    let parsed =
        Uuid::parse_str(value).map_err(|_| AutomationValidationError::InvalidUuid { field })?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != value {
        return Err(AutomationValidationError::InvalidUuid { field });
    }
    Ok(canonical)
}

fn require_version(version: u64) -> Result<(), AutomationValidationError> {
    if version == 0 {
        Err(AutomationValidationError::InvalidVersion)
    } else {
        Ok(())
    }
}

fn required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, AutomationValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AutomationValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(AutomationValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        AutomationScope, AutomationValidationError, CreateAutomation, DailySchedule,
        UpdateAutomation,
    };
    use crate::CanvasPlacement;

    const AUTOMATION_ID: &str = "0198a81c-3773-7c60-b7d2-ff795ad88ad0";
    const PROJECT_A: &str = "0198a81c-3773-7c60-b7d2-ff795ad88ad1";
    const PROJECT_B: &str = "0198a81c-3773-7c60-b7d2-ff795ad88ad2";

    fn placement() -> CanvasPlacement {
        CanvasPlacement {
            x: 10.0,
            y: 20.0,
            width: 240.0,
            height: 120.0,
        }
    }

    #[test]
    fn normalizes_timezone_text_and_project_ids() {
        let command = CreateAutomation {
            command_id: " create-automation ".to_owned(),
            actor: " local-user ".to_owned(),
            automation_id: AUTOMATION_ID.to_owned(),
            name: " Daily status ".to_owned(),
            scope: AutomationScope::YardOrchestrator,
            placement: placement(),
            schedule: DailySchedule {
                hour: 9,
                minute: 30,
                timezone: " America/Los_Angeles ".to_owned(),
            },
            selected_project_ids: vec![PROJECT_B.to_owned(), PROJECT_A.to_owned()],
            prompt_template: " Summarize project status. ".to_owned(),
        }
        .normalize()
        .unwrap();

        assert_eq!(command.command_id, "create-automation");
        assert_eq!(command.name, "Daily status");
        assert_eq!(command.schedule.timezone, "America/Los_Angeles");
        assert_eq!(command.selected_project_ids, [PROJECT_A, PROJECT_B]);
        assert_eq!(command.prompt_template, "Summarize project status.");
    }

    #[test]
    fn rejects_invalid_timezone_strings() {
        let error = DailySchedule {
            hour: 9,
            minute: 30,
            timezone: "Mars/Olympus_Mons".to_owned(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(error, AutomationValidationError::InvalidTimezone);
    }

    #[test]
    fn project_scope_requires_exact_project_selection() {
        let error = UpdateAutomation {
            command_id: "update".to_owned(),
            actor: "local-user".to_owned(),
            automation_id: AUTOMATION_ID.to_owned(),
            expected_version: 1,
            name: "Daily status".to_owned(),
            scope: AutomationScope::ProjectOrchestrator {
                project_id: PROJECT_A.to_owned(),
            },
            schedule: DailySchedule {
                hour: 9,
                minute: 30,
                timezone: "UTC".to_owned(),
            },
            selected_project_ids: vec![PROJECT_B.to_owned()],
            prompt_template: "Report.".to_owned(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(
            error,
            AutomationValidationError::ProjectScopeSelectionMismatch
        );
    }

    #[test]
    fn scope_is_kind_tagged_and_versions_serialize_as_strings() {
        let command = UpdateAutomation {
            command_id: "update".to_owned(),
            actor: "local-user".to_owned(),
            automation_id: AUTOMATION_ID.to_owned(),
            expected_version: 7,
            name: "Daily status".to_owned(),
            scope: AutomationScope::ProjectOrchestrator {
                project_id: PROJECT_A.to_owned(),
            },
            schedule: DailySchedule {
                hour: 9,
                minute: 30,
                timezone: "UTC".to_owned(),
            },
            selected_project_ids: vec![PROJECT_A.to_owned()],
            prompt_template: "Report.".to_owned(),
        }
        .normalize()
        .unwrap();

        let json = serde_json::to_value(command).unwrap();
        assert_eq!(json["expected_version"], "7");
        assert_eq!(json["scope"]["kind"], "project_orchestrator");
        assert_eq!(json["scope"]["project_id"], PROJECT_A);
    }
}

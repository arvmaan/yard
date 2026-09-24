use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
pub const MAX_PANE_MANAGEMENT_CANDIDATES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneManagementCategory {
    Eligible,
    AlreadyManaged,
    Conflict,
    Ambiguous,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneManagementCandidate {
    pub candidate_key: String,
    pub category: PaneManagementCategory,
    pub reason: String,
    pub session: String,
    pub workspace_id: Option<String>,
    pub workspace_label: Option<String>,
    pub pane_id: Option<String>,
    pub pane_instance_id: Option<String>,
    pub terminal_id: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub worker_id: Option<String>,
    pub provider: Option<String>,
    pub display_provider: Option<String>,
    pub recovery_required: bool,
    pub management_controls_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneManagementPreview {
    pub supported: bool,
    pub endpoint: Option<String>,
    pub limit: usize,
    pub truncated: bool,
    pub candidate_count: usize,
    pub eligible_count: usize,
    pub candidates: Vec<PaneManagementCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManageAllAgents {
    pub command_id: String,
    pub actor: String,
    pub confirmed: bool,
    pub candidate_keys: Vec<String>,
}

impl ManageAllAgents {
    /// Normalize and validate one explicit pane-management batch.
    ///
    /// # Errors
    ///
    /// Returns [`PaneManagementValidationError`] for blank, oversized,
    /// duplicate, unconfirmed, or over-limit input.
    pub fn normalize(mut self) -> Result<Self, PaneManagementValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if !self.confirmed {
            return Err(PaneManagementValidationError::ConfirmationRequired);
        }
        if self.candidate_keys.is_empty()
            || self.candidate_keys.len() > MAX_PANE_MANAGEMENT_CANDIDATES
        {
            return Err(PaneManagementValidationError::InvalidCandidateCount);
        }
        for key in &mut self.candidate_keys {
            *key = bounded_required("candidate_key", key, 512)?;
        }
        self.candidate_keys.sort();
        if self
            .candidate_keys
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(PaneManagementValidationError::DuplicateCandidate);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneManagementOutcome {
    Managed,
    AlreadyManaged,
    Conflict,
    Skipped,
    Failed,
    RollbackFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneManagementItemResult {
    pub candidate_key: String,
    pub outcome: PaneManagementOutcome,
    pub reason: String,
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneManagementBatchResult {
    pub command_id: String,
    pub replayed: bool,
    pub managed_count: usize,
    pub results: Vec<PaneManagementItemResult>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PaneManagementValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("pane management requires explicit confirmation")]
    ConfirmationRequired,
    #[error("candidate count must be between 1 and {MAX_PANE_MANAGEMENT_CANDIDATES}")]
    InvalidCandidateCount,
    #[error("candidate keys must be unique")]
    DuplicateCandidate,
}

fn bounded_required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, PaneManagementValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(PaneManagementValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(PaneManagementValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

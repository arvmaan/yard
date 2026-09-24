use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_RATIONALE_BYTES: usize = 8_000;
const MAX_EVIDENCE_IDS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAdvisorRecommendation {
    Keep,
    Retire,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupAdvisorArtifact {
    pub recommendation: CleanupAdvisorRecommendation,
    pub evidence_ids: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupAdvisorRequest {
    pub run_id: String,
    pub project_id: String,
    pub target_worker_id: String,
    pub target_assignment_id: String,
    pub parent_worker_id: String,
    pub runtime_adapter: String,
    pub runtime_session: String,
    pub runtime_workspace_id: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupAdvisorResult {
    pub advisor_worker_id: String,
    pub advisor_assignment_id: String,
    pub completion_receipt_id: String,
    pub artifact_id: String,
    pub artifact: CleanupAdvisorArtifact,
}

impl CleanupAdvisorArtifact {
    /// Normalizes bounded evidence and rationale fields.
    ///
    /// # Errors
    ///
    /// Returns an error when evidence or rationale violates cleanup bounds.
    pub fn normalize(mut self) -> Result<Self, WorkerCleanupValidationError> {
        self.rationale = bounded_required("rationale", &self.rationale, MAX_RATIONALE_BYTES)?;
        if self.evidence_ids.is_empty() || self.evidence_ids.len() > MAX_EVIDENCE_IDS {
            return Err(WorkerCleanupValidationError::EvidenceCount);
        }
        for evidence_id in &mut self.evidence_ids {
            *evidence_id = bounded_required("evidence_id", evidence_id, MAX_ID_BYTES)?;
        }
        self.evidence_ids.sort();
        self.evidence_ids.dedup();
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCleanupPolicy {
    pub automatic_enabled: bool,
    pub schedule_minutes: u64,
    pub grace_period_ms: u64,
    pub batch_size: usize,
    pub advisor_profile_id: Option<String>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub updated_by: String,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateWorkerCleanupPolicy {
    pub actor: String,
    pub automatic_enabled: bool,
    pub confirm_automatic_enable: bool,
    pub schedule_minutes: u64,
    pub grace_period_ms: u64,
    pub batch_size: usize,
    pub advisor_profile_id: Option<String>,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

impl UpdateWorkerCleanupPolicy {
    /// Normalizes and validates a policy update.
    ///
    /// # Errors
    ///
    /// Returns an error when fields are invalid or automatic cleanup lacks confirmation.
    pub fn normalize(mut self) -> Result<Self, WorkerCleanupValidationError> {
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_version == 0 {
            return Err(WorkerCleanupValidationError::InvalidVersion);
        }
        if self.automatic_enabled && !self.confirm_automatic_enable {
            return Err(WorkerCleanupValidationError::AutomaticEnableNotConfirmed);
        }
        if !(5..=7 * 24 * 60).contains(&self.schedule_minutes) {
            return Err(WorkerCleanupValidationError::InvalidSchedule);
        }
        if !(60_000..=90 * 24 * 60 * 60 * 1_000).contains(&self.grace_period_ms) {
            return Err(WorkerCleanupValidationError::InvalidGracePeriod);
        }
        if !(1..=100).contains(&self.batch_size) {
            return Err(WorkerCleanupValidationError::InvalidBatchSize);
        }
        self.advisor_profile_id = self
            .advisor_profile_id
            .map(|value| bounded_required("advisor_profile_id", &value, MAX_ID_BYTES))
            .transpose()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCleanupRunTrigger {
    Preview,
    Manual,
    Scheduled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCleanupRunStatus {
    Pending,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAdvisorState {
    NotRequested,
    Unsupported,
    Pending,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCleanupItemStatus {
    Pending,
    Keep,
    Review,
    Reconciled,
    Retired,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCleanupRunItem {
    pub worker_id: String,
    pub project_id: String,
    pub assignment_id: String,
    pub completion_receipt_id: String,
    pub status: WorkerCleanupItemStatus,
    pub reason: String,
    pub attempts: u32,
    pub advisor_recommendation: Option<CleanupAdvisorRecommendation>,
    pub advisor_completion_receipt_id: Option<String>,
    pub advisor_artifact_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCleanupRun {
    pub id: String,
    pub command_id: String,
    pub trigger: WorkerCleanupRunTrigger,
    pub preview: bool,
    pub status: WorkerCleanupRunStatus,
    pub advisor_state: CleanupAdvisorState,
    pub requested_by: String,
    pub candidate_count: usize,
    pub keep_count: usize,
    pub review_count: usize,
    pub reconciled_count: usize,
    pub retired_count: usize,
    pub failed_count: usize,
    pub cancellation_requested: bool,
    pub last_error: Option<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub completed_at_unix_ms: Option<u64>,
    pub items: Vec<WorkerCleanupRunItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCleanupRuns {
    pub runs: Vec<WorkerCleanupRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCleanupDashboard {
    pub policy: WorkerCleanupPolicy,
    pub preview: crate::CompletedRuntimeCleanupPreview,
    pub runs: WorkerCleanupRuns,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartWorkerCleanupRun {
    pub command_id: String,
    pub actor: String,
    pub preview: bool,
}

impl StartWorkerCleanupRun {
    /// Normalizes a cleanup run command.
    ///
    /// # Errors
    ///
    /// Returns an error when the command identity or actor is invalid.
    pub fn normalize(mut self) -> Result<Self, WorkerCleanupValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelWorkerCleanupRun {
    pub command_id: String,
    pub actor: String,
}

impl CancelWorkerCleanupRun {
    /// Normalizes a cleanup cancellation command.
    ///
    /// # Errors
    ///
    /// Returns an error when the command identity or actor is invalid.
    pub fn normalize(mut self) -> Result<Self, WorkerCleanupValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        Ok(self)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkerCleanupValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("worker cleanup versions must be greater than zero")]
    InvalidVersion,
    #[error("automatic cleanup requires explicit confirmation")]
    AutomaticEnableNotConfirmed,
    #[error("cleanup schedule must be between 5 minutes and 7 days")]
    InvalidSchedule,
    #[error("cleanup grace period must be between 1 minute and 90 days")]
    InvalidGracePeriod,
    #[error("cleanup batch size must be between 1 and 100")]
    InvalidBatchSize,
    #[error("cleanup advisor output must contain between 1 and 64 evidence IDs")]
    EvidenceCount,
}

fn bounded_required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, WorkerCleanupValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(WorkerCleanupValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(WorkerCleanupValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisor_output_is_typed_and_deduplicated() {
        let artifact = CleanupAdvisorArtifact {
            recommendation: CleanupAdvisorRecommendation::Review,
            evidence_ids: vec![" receipt-1 ".to_owned(), "receipt-1".to_owned()],
            rationale: " Ownership is ambiguous. ".to_owned(),
        }
        .normalize()
        .unwrap();
        assert_eq!(artifact.evidence_ids, ["receipt-1"]);
        assert_eq!(artifact.rationale, "Ownership is ambiguous.");
    }

    #[test]
    fn automatic_enable_requires_confirmation() {
        let error = UpdateWorkerCleanupPolicy {
            actor: "local-user".to_owned(),
            automatic_enabled: true,
            confirm_automatic_enable: false,
            schedule_minutes: 60,
            grace_period_ms: 86_400_000,
            batch_size: 25,
            advisor_profile_id: None,
            expected_version: 1,
        }
        .normalize()
        .unwrap_err();
        assert_eq!(
            error,
            WorkerCleanupValidationError::AutomaticEnableNotConfirmed
        );
    }
}

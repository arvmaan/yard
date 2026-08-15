use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Artifact, Worker, WorkerProfile};

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_OBJECTIVE_BYTES: usize = 16_000;
const MAX_ROLE_BYTES: usize = 240;
const MAX_RECEIPT_SUMMARY_BYTES: usize = 16_000;
const MAX_RECEIPT_VALUE_BYTES: usize = 2_048;
const MAX_RECEIPT_VALUES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDesiredState {
    Running,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationMode {
    CreateNew,
    AdoptExisting,
    Handoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationPolicy {
    ProjectWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentLifecycle {
    Allocating,
    Active,
    HandingOff,
    HandedOff,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptLifecycle {
    Starting,
    Active,
    HandingOff,
    HandedOff,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffTargetRole {
    Member,
    Orchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionOutcome {
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionReceipt {
    pub id: String,
    pub assignment_id: String,
    pub attempt_id: String,
    pub outcome: CompletionOutcome,
    pub summary: String,
    pub artifact_refs: Vec<String>,
    pub artifacts: Vec<Artifact>,
    pub evidence_refs: Vec<String>,
    pub unresolved_blockers: Vec<String>,
    pub actor: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerAllocation {
    pub id: String,
    pub project_id: String,
    pub worker_id: String,
    pub mode: AllocationMode,
    pub started_by_command_id: Option<String>,
    pub started_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentAttempt {
    pub id: String,
    pub assignment_id: String,
    pub ordinal: u32,
    pub lifecycle: AttemptLifecycle,
    pub error: Option<String>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub project_id: String,
    pub allocation_id: String,
    pub worker: Worker,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub profile_version: u64,
    pub profile_name: String,
    pub objective: String,
    pub role: String,
    pub isolation_policy: IsolationPolicy,
    pub lifecycle: AssignmentLifecycle,
    pub attempt: AssignmentAttempt,
    pub completion_receipt: Option<CompletionReceipt>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignments {
    pub assignments: Vec<Assignment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedAllocation {
    pub command_id: String,
    pub allocation: WorkerAllocation,
    pub assignment: Assignment,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedWorkerHandoff {
    pub command_id: String,
    pub source_assignment: Assignment,
    pub allocation: WorkerAllocation,
    pub assignment: Assignment,
    pub target_role: HandoffTargetRole,
    pub replaced_orchestrator_worker_id: Option<String>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCompletionReceipt {
    pub command_id: String,
    pub actor: String,
    pub attempt_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_assignment_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_attempt_version: u64,
    pub outcome: CompletionOutcome,
    pub summary: String,
    pub artifact_refs: Vec<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub unresolved_blockers: Vec<String>,
}

impl RecordCompletionReceipt {
    /// Normalize and validate an explicit assignment completion command.
    ///
    /// # Errors
    ///
    /// Returns [`CompletionValidationError`] for blank or oversized values,
    /// missing evidence, oversized lists, or zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, CompletionValidationError> {
        self.command_id = receipt_required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = receipt_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.attempt_id = receipt_required("attempt_id", &self.attempt_id, MAX_COMMAND_ID_BYTES)?;
        self.summary = receipt_required("summary", &self.summary, MAX_RECEIPT_SUMMARY_BYTES)?;
        self.artifact_refs = receipt_values("artifact_refs", self.artifact_refs)?;
        self.artifact_ids = receipt_values("artifact_ids", self.artifact_ids)?;
        let unique_artifact_ids = self.artifact_ids.iter().collect::<HashSet<_>>();
        if unique_artifact_ids.len() != self.artifact_ids.len() {
            return Err(CompletionValidationError::DuplicateArtifactId);
        }
        self.evidence_refs = receipt_values("evidence_refs", self.evidence_refs)?;
        self.unresolved_blockers = receipt_values("unresolved_blockers", self.unresolved_blockers)?;
        if self.expected_assignment_version == 0 || self.expected_attempt_version == 0 {
            return Err(CompletionValidationError::InvalidVersion);
        }
        if self.artifact_refs.is_empty()
            && self.artifact_ids.is_empty()
            && self.evidence_refs.is_empty()
        {
            return Err(CompletionValidationError::EvidenceRequired);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedCompletionReceipt {
    pub command_id: String,
    pub receipt: CompletionReceipt,
    pub assignment: Assignment,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmProfileAllocation {
    pub command_id: String,
    pub actor: String,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub objective: String,
    pub role: String,
    pub isolation_policy: IsolationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmWorkerAllocation {
    pub command_id: String,
    pub actor: String,
    pub worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
    pub profile_id: Option<String>,
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_profile_version: Option<u64>,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub objective: String,
    pub role: String,
    pub isolation_policy: IsolationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmWorkerHandoff {
    pub command_id: String,
    pub actor: String,
    pub worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_source_project_version: u64,
    pub target_project_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_target_project_version: u64,
    pub source_attempt_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_source_assignment_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_source_attempt_version: u64,
    pub target_role: HandoffTargetRole,
    pub objective: String,
    pub role: String,
    pub isolation_policy: IsolationPolicy,
}

impl ConfirmWorkerHandoff {
    /// Normalize and validate a confirmed cross-project worker handoff.
    ///
    /// # Errors
    ///
    /// Returns [`AssignmentValidationError`] for blank or oversized text and
    /// zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, AssignmentValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.worker_id = required("worker_id", &self.worker_id, MAX_COMMAND_ID_BYTES)?;
        self.target_project_id = required(
            "target_project_id",
            &self.target_project_id,
            MAX_COMMAND_ID_BYTES,
        )?;
        self.source_attempt_id = required(
            "source_attempt_id",
            &self.source_attempt_id,
            MAX_COMMAND_ID_BYTES,
        )?;
        self.objective = required("objective", &self.objective, MAX_OBJECTIVE_BYTES)?;
        self.role = required("role", &self.role, MAX_ROLE_BYTES)?;
        if self.expected_worker_version == 0
            || self.expected_source_project_version == 0
            || self.expected_target_project_version == 0
            || self.expected_source_assignment_version == 0
            || self.expected_source_attempt_version == 0
        {
            return Err(AssignmentValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

impl ConfirmWorkerAllocation {
    /// Normalize and validate a confirmed existing-worker allocation command.
    ///
    /// # Errors
    ///
    /// Returns [`AssignmentValidationError`] for blank or oversized text,
    /// incomplete profile selection, and zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, AssignmentValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.worker_id = required("worker_id", &self.worker_id, MAX_COMMAND_ID_BYTES)?;
        self.profile_id = self
            .profile_id
            .map(|profile_id| required("profile_id", &profile_id, MAX_COMMAND_ID_BYTES))
            .transpose()?;
        self.objective = required("objective", &self.objective, MAX_OBJECTIVE_BYTES)?;
        self.role = required("role", &self.role, MAX_ROLE_BYTES)?;
        if self.expected_worker_version == 0 || self.expected_project_version == 0 {
            return Err(AssignmentValidationError::InvalidVersion);
        }
        if self.profile_id.is_some() != self.expected_profile_version.is_some() {
            return Err(AssignmentValidationError::IncompleteProfileSelection);
        }
        if self.expected_profile_version == Some(0) {
            return Err(AssignmentValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

impl ConfirmProfileAllocation {
    /// Normalize and validate a confirmed profile allocation command.
    ///
    /// # Errors
    ///
    /// Returns [`AssignmentValidationError`] for blank or oversized text and
    /// zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, AssignmentValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.profile_id = required("profile_id", &self.profile_id, MAX_COMMAND_ID_BYTES)?;
        self.objective = required("objective", &self.objective, MAX_OBJECTIVE_BYTES)?;
        self.role = required("role", &self.role, MAX_ROLE_BYTES)?;
        if self.expected_profile_version == 0 || self.expected_project_version == 0 {
            return Err(AssignmentValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AllocationContext {
    pub command: ConfirmProfileAllocation,
    pub project: crate::Project,
    pub profile: WorkerProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssignmentValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("expected versions must be greater than zero")]
    InvalidVersion,
    #[error("profile_id and expected_profile_version must be supplied together")]
    IncompleteProfileSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompletionValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("{field} must contain at most {max} values")]
    TooManyValues { field: &'static str, max: usize },
    #[error("completed receipts require at least one artifact or evidence reference")]
    EvidenceRequired,
    #[error("artifact_ids must not contain duplicates")]
    DuplicateArtifactId,
    #[error("expected versions must be greater than zero")]
    InvalidVersion,
}

fn required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, AssignmentValidationError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(AssignmentValidationError::Required(field));
    }
    if value.len() > max {
        return Err(AssignmentValidationError::TooLong { field, max });
    }
    Ok(value)
}

fn receipt_required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, CompletionValidationError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(CompletionValidationError::Required(field));
    }
    if value.len() > max {
        return Err(CompletionValidationError::TooLong { field, max });
    }
    Ok(value)
}

fn receipt_values(
    field: &'static str,
    values: Vec<String>,
) -> Result<Vec<String>, CompletionValidationError> {
    if values.len() > MAX_RECEIPT_VALUES {
        return Err(CompletionValidationError::TooManyValues {
            field,
            max: MAX_RECEIPT_VALUES,
        });
    }
    values
        .into_iter()
        .map(|value| receipt_required(field, &value, MAX_RECEIPT_VALUE_BYTES))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CompletionOutcome, CompletionValidationError, ConfirmProfileAllocation,
        ConfirmWorkerAllocation, ConfirmWorkerHandoff, HandoffTargetRole, IsolationPolicy,
        RecordCompletionReceipt,
    };

    #[test]
    fn normalizes_confirmed_allocation() {
        let command = ConfirmProfileAllocation {
            command_id: " command-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            profile_id: " profile-1 ".to_owned(),
            expected_profile_version: 1,
            expected_project_version: 1,
            objective: " Implement the API. ".to_owned(),
            role: " implementer ".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        }
        .normalize()
        .unwrap();

        assert_eq!(command.command_id, "command-1");
        assert_eq!(command.objective, "Implement the API.");
    }

    #[test]
    fn normalizes_confirmed_existing_worker_allocation() {
        let command = ConfirmWorkerAllocation {
            command_id: " command-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            worker_id: " worker-1 ".to_owned(),
            expected_worker_version: 2,
            profile_id: Some(" profile-1 ".to_owned()),
            expected_profile_version: Some(1),
            expected_project_version: 3,
            objective: " Continue the API. ".to_owned(),
            role: " implementer ".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        }
        .normalize()
        .unwrap();

        assert_eq!(command.worker_id, "worker-1");
        assert_eq!(command.profile_id.as_deref(), Some("profile-1"));
        assert_eq!(command.objective, "Continue the API.");
    }

    #[test]
    fn normalizes_confirmed_worker_handoff() {
        let command = ConfirmWorkerHandoff {
            command_id: " handoff-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            worker_id: " worker-1 ".to_owned(),
            expected_worker_version: 4,
            expected_source_project_version: 2,
            target_project_id: " project-2 ".to_owned(),
            expected_target_project_version: 3,
            source_attempt_id: " attempt-1 ".to_owned(),
            expected_source_assignment_version: 2,
            expected_source_attempt_version: 2,
            target_role: HandoffTargetRole::Member,
            objective: " Continue in the target workspace. ".to_owned(),
            role: " implementer ".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        }
        .normalize()
        .unwrap();

        assert_eq!(command.command_id, "handoff-1");
        assert_eq!(command.target_project_id, "project-2");
        assert_eq!(command.objective, "Continue in the target workspace.");
    }

    #[test]
    fn normalizes_evidence_backed_completion_receipt() {
        let command = RecordCompletionReceipt {
            command_id: " receipt-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            attempt_id: " attempt-1 ".to_owned(),
            expected_assignment_version: 2,
            expected_attempt_version: 2,
            outcome: CompletionOutcome::Completed,
            summary: " Implemented the API. ".to_owned(),
            artifact_refs: vec![" yard://artifacts/change ".to_owned()],
            artifact_ids: Vec::new(),
            evidence_refs: vec![" test://workspace ".to_owned()],
            unresolved_blockers: Vec::new(),
        }
        .normalize()
        .unwrap();

        assert_eq!(command.summary, "Implemented the API.");
        assert_eq!(command.evidence_refs, ["test://workspace"]);
    }

    #[test]
    fn rejects_completion_without_evidence() {
        let error = RecordCompletionReceipt {
            command_id: "receipt-1".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: "attempt-1".to_owned(),
            expected_assignment_version: 2,
            expected_attempt_version: 2,
            outcome: CompletionOutcome::Completed,
            summary: "Implemented the API.".to_owned(),
            artifact_refs: Vec::new(),
            artifact_ids: Vec::new(),
            evidence_refs: Vec::new(),
            unresolved_blockers: Vec::new(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(error, CompletionValidationError::EvidenceRequired);
    }
}

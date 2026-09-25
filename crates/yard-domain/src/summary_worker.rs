use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Artifact, ArtifactContent, Assignment, ProviderSessionRef};

const MAX_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_OBJECTIVE_BYTES: usize = 16_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestSummaryWorker {
    pub command_id: String,
    pub actor: String,
    pub parent_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_parent_worker_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    pub artifact_id: String,
    pub objective: String,
}

impl RequestSummaryWorker {
    /// Normalize one explicit, user-initiated summary-worker request.
    ///
    /// # Errors
    ///
    /// Returns [`SummaryWorkerValidationError`] for blank, oversized, or
    /// zero-version input.
    pub fn normalize(mut self) -> Result<Self, SummaryWorkerValidationError> {
        self.command_id = bounded("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = bounded("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.parent_worker_id = bounded("parent_worker_id", &self.parent_worker_id, MAX_ID_BYTES)?;
        self.profile_id = bounded("profile_id", &self.profile_id, MAX_ID_BYTES)?;
        self.artifact_id = bounded("artifact_id", &self.artifact_id, MAX_ID_BYTES)?;
        if Uuid::parse_str(&self.artifact_id)
            .ok()
            .is_none_or(|artifact_id| artifact_id.to_string() != self.artifact_id)
        {
            return Err(SummaryWorkerValidationError::InvalidArtifactId);
        }
        self.objective = bounded("objective", &self.objective, MAX_OBJECTIVE_BYTES)?;
        if self.expected_parent_worker_version == 0
            || self.expected_project_version == 0
            || self.expected_profile_version == 0
        {
            return Err(SummaryWorkerValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveSummaryWorker {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_parent_worker_version: u64,
}

impl ReceiveSummaryWorker {
    /// Normalize one explicit parent handoff command.
    ///
    /// # Errors
    ///
    /// Returns [`SummaryWorkerValidationError`] for blank, oversized, or
    /// zero-version input.
    pub fn normalize(mut self) -> Result<Self, SummaryWorkerValidationError> {
        self.command_id = bounded("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = bounded("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_parent_worker_version == 0 {
            return Err(SummaryWorkerValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryParentRuntimeCapture {
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub pane_instance_id: Option<String>,
    pub provider_session: Option<ProviderSessionRef>,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryWorkerState {
    Allocating,
    Active,
    Ready,
    RetirementPending,
    RetirementDeferred,
    Retired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryWorker {
    pub command_id: String,
    pub project_id: String,
    pub parent_worker_id: String,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub profile_version: u64,
    pub objective: String,
    pub expected_artifact_id: String,
    pub captured_workspace_id: String,
    pub state: SummaryWorkerState,
    pub assignment: Option<Assignment>,
    pub artifact: Option<Artifact>,
    pub cleanup_run_id: Option<String>,
    pub retirement_reason: Option<String>,
    pub error: Option<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryWorkers {
    pub summaries: Vec<SummaryWorker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceivedSummaryWorker {
    pub summary: SummaryWorker,
    pub artifact: ArtifactContent,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SummaryWorkerValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("summary-worker versions must be greater than zero")]
    InvalidVersion,
    #[error("artifact_id must be a canonical UUID")]
    InvalidArtifactId,
}

fn bounded(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, SummaryWorkerValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(SummaryWorkerValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(SummaryWorkerValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{RequestSummaryWorker, SummaryWorkerValidationError};

    #[test]
    fn normalizes_explicit_summary_request() {
        let request = RequestSummaryWorker {
            command_id: " summary-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            parent_worker_id: " parent-1 ".to_owned(),
            expected_parent_worker_version: 2,
            expected_project_version: 3,
            profile_id: " profile-1 ".to_owned(),
            expected_profile_version: 4,
            artifact_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            objective: " Summarize the completed work. ".to_owned(),
        }
        .normalize()
        .unwrap();

        assert_eq!(request.command_id, "summary-1");
        assert_eq!(request.objective, "Summarize the completed work.");
    }

    #[test]
    fn rejects_zero_versions() {
        let error = RequestSummaryWorker {
            command_id: "summary-1".to_owned(),
            actor: "local-user".to_owned(),
            parent_worker_id: "parent-1".to_owned(),
            expected_parent_worker_version: 0,
            expected_project_version: 1,
            profile_id: "profile-1".to_owned(),
            expected_profile_version: 1,
            artifact_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            objective: "Summarize.".to_owned(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(error, SummaryWorkerValidationError::InvalidVersion);
    }
}

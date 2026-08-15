use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Worker;

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerAvailability {
    YardOrchestrator,
    CoordinationNode,
    Orchestrator,
    Assigned,
    UnassignedLive,
    Resumable,
    Unavailable,
    Ambiguous,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerCandidate {
    pub worker: Worker,
    pub profile_name: Option<String>,
    pub default_role: Option<String>,
    pub availability: WorkerAvailability,
    pub project_id: Option<String>,
    pub assignment_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerCandidates {
    pub workers: Vec<WorkerCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndWorkerSession {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_runtime_version: Option<u64>,
}

impl EndWorkerSession {
    /// Normalize and validate an explicit worker session termination command.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerSessionValidationError`] when a required value is blank
    /// or oversized, or an optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, WorkerSessionValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_worker_version == 0 || self.expected_runtime_version == Some(0) {
            return Err(WorkerSessionValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndedWorkerSession {
    pub command_id: String,
    pub worker: Worker,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkerSessionValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("optimistic versions must be greater than zero")]
    InvalidVersion,
}

fn bounded_required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, WorkerSessionValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(WorkerSessionValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(WorkerSessionValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

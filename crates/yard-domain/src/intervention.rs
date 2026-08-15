use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::OrchestratorStatusReport;

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_PROMPT_BYTES: usize = 16_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendAssignmentPrompt {
    pub command_id: String,
    pub actor: String,
    pub attempt_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_assignment_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_attempt_version: u64,
    pub text: String,
}

impl SendAssignmentPrompt {
    /// Normalize and validate a direct prompt command.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionValidationError`] for blank or oversized values
    /// and zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, InterventionValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.attempt_id = required("attempt_id", &self.attempt_id, MAX_COMMAND_ID_BYTES)?;
        self.text = required("text", &self.text, MAX_PROMPT_BYTES)?;
        if self.expected_assignment_version == 0 || self.expected_attempt_version == 0 {
            return Err(InterventionValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendOrchestratorPrompt {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub orchestrator_worker_id: String,
    pub text: String,
}

impl SendOrchestratorPrompt {
    /// Normalize and validate a project orchestrator prompt command.
    ///
    /// # Errors
    ///
    /// Returns [`InterventionValidationError`] for blank or oversized values
    /// and zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, InterventionValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.orchestrator_worker_id = required(
            "orchestrator_worker_id",
            &self.orchestrator_worker_id,
            MAX_COMMAND_ID_BYTES,
        )?;
        self.text = required("text", &self.text, MAX_PROMPT_BYTES)?;
        if self.expected_project_version == 0 {
            return Err(InterventionValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAcknowledgement {
    pub command_id: String,
    pub assignment_id: String,
    pub attempt_id: String,
    pub runtime_status: String,
    pub submitted_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorPromptAcknowledgement {
    pub command_id: String,
    pub project_id: String,
    pub worker_id: String,
    pub runtime_status: String,
    pub submitted_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutput {
    pub assignment_id: String,
    pub attempt_id: String,
    pub pane_id: String,
    pub source: String,
    pub format: String,
    pub text: String,
    #[serde(with = "crate::serde_u64")]
    pub revision: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorTerminalOutput {
    pub project_id: String,
    pub worker_id: String,
    pub pane_id: String,
    pub source: String,
    pub format: String,
    pub text: String,
    #[serde(with = "crate::serde_u64")]
    pub revision: u64,
    pub truncated: bool,
    pub status_report: Option<OrchestratorStatusReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InterventionValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("expected versions must be greater than zero")]
    InvalidVersion,
}

fn required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, InterventionValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(InterventionValidationError::Required(field));
    }
    if value.len() > max_bytes {
        return Err(InterventionValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{InterventionValidationError, SendAssignmentPrompt, SendOrchestratorPrompt};

    #[test]
    fn normalizes_prompt_command() {
        let prompt = SendAssignmentPrompt {
            command_id: " command-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            attempt_id: " attempt-1 ".to_owned(),
            expected_assignment_version: 2,
            expected_attempt_version: 3,
            text: " Continue with the failing test. ".to_owned(),
        }
        .normalize()
        .unwrap();

        assert_eq!(prompt.command_id, "command-1");
        assert_eq!(prompt.text, "Continue with the failing test.");
    }

    #[test]
    fn rejects_blank_or_stale_prompt_command() {
        let command = SendAssignmentPrompt {
            command_id: "command-1".to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: "attempt-1".to_owned(),
            expected_assignment_version: 0,
            expected_attempt_version: 1,
            text: "Continue.".to_owned(),
        };
        assert_eq!(
            command.normalize().unwrap_err(),
            InterventionValidationError::InvalidVersion
        );
    }

    #[test]
    fn normalizes_orchestrator_prompt_command() {
        let prompt = SendOrchestratorPrompt {
            command_id: " command-2 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_project_version: 2,
            orchestrator_worker_id: " worker-1 ".to_owned(),
            text: " Rebalance the active workers. ".to_owned(),
        }
        .normalize()
        .unwrap();

        assert_eq!(prompt.command_id, "command-2");
        assert_eq!(prompt.orchestrator_worker_id, "worker-1");
        assert_eq!(prompt.text, "Rebalance the active workers.");
    }

    #[test]
    fn orchestrator_prompt_requires_the_current_worker_identity() {
        let error = SendOrchestratorPrompt {
            command_id: "command-2".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: 2,
            orchestrator_worker_id: " ".to_owned(),
            text: "Rebalance the active workers.".to_owned(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(
            error,
            InterventionValidationError::Required("orchestrator_worker_id")
        );
    }
}

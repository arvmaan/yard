use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{OrchestratorStatusReport, Worker};

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_PROMPT_BYTES: usize = 16_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YardOrchestrator {
    pub worker: Option<Worker>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureYardOrchestrator {
    pub command_id: String,
    pub actor: String,
    pub worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionYardOrchestrator {
    pub command_id: String,
    pub actor: String,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverYardOrchestrator {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_version: u64,
}

impl ProvisionYardOrchestrator {
    /// Normalize and validate a dedicated Yard orchestrator provision command.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorValidationError`] for blank or oversized
    /// values and zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, YardOrchestratorValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.profile_id = required("profile_id", &self.profile_id, MAX_COMMAND_ID_BYTES)?;
        if self.expected_profile_version == 0 || self.expected_orchestrator_version == 0 {
            return Err(YardOrchestratorValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

impl RecoverYardOrchestrator {
    /// Normalize and validate a dedicated Yard orchestrator recovery command.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorValidationError`] for blank or oversized
    /// values and a zero optimistic version.
    pub fn normalize(mut self) -> Result<Self, YardOrchestratorValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_orchestrator_version == 0 {
            return Err(YardOrchestratorValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

impl ConfigureYardOrchestrator {
    /// Normalize and validate a Yard orchestrator ownership command.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorValidationError`] for blank or oversized
    /// values and zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, YardOrchestratorValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.worker_id = required("worker_id", &self.worker_id, MAX_COMMAND_ID_BYTES)?;
        if self.expected_worker_version == 0 || self.expected_orchestrator_version == 0 {
            return Err(YardOrchestratorValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfiguredYardOrchestrator {
    pub command_id: String,
    pub orchestrator: YardOrchestrator,
    pub replaced_worker_id: Option<String>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveredYardOrchestrator {
    pub command_id: String,
    pub orchestrator: YardOrchestrator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendYardOrchestratorPrompt {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_version: u64,
    pub orchestrator_worker_id: String,
    pub text: String,
}

impl SendYardOrchestratorPrompt {
    /// Normalize and validate a direct Yard orchestrator prompt.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorValidationError`] for blank or oversized
    /// values and a zero optimistic version.
    pub fn normalize(mut self) -> Result<Self, YardOrchestratorValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.orchestrator_worker_id = required(
            "orchestrator_worker_id",
            &self.orchestrator_worker_id,
            MAX_COMMAND_ID_BYTES,
        )?;
        self.text = required("text", &self.text, MAX_PROMPT_BYTES)?;
        if self.expected_orchestrator_version == 0 {
            return Err(YardOrchestratorValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardOrchestratorPromptAcknowledgement {
    pub command_id: String,
    pub worker_id: String,
    pub runtime_status: String,
    pub submitted_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardOrchestratorTerminalOutput {
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
pub enum YardOrchestratorValidationError {
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
) -> Result<String, YardOrchestratorValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(YardOrchestratorValidationError::Required(field));
    }
    if value.len() > max_bytes {
        return Err(YardOrchestratorValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigureYardOrchestrator, ProvisionYardOrchestrator, RecoverYardOrchestrator,
        SendYardOrchestratorPrompt, YardOrchestratorValidationError,
    };

    #[test]
    fn normalizes_configuration_and_prompt_commands() {
        let configuration = ConfigureYardOrchestrator {
            command_id: " configure-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            worker_id: " worker-1 ".to_owned(),
            expected_worker_version: 2,
            expected_orchestrator_version: 3,
        }
        .normalize()
        .unwrap();
        assert_eq!(configuration.command_id, "configure-1");
        assert_eq!(configuration.worker_id, "worker-1");

        let provision = ProvisionYardOrchestrator {
            command_id: " provision-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            profile_id: " profile-1 ".to_owned(),
            expected_profile_version: 2,
            expected_orchestrator_version: 3,
        }
        .normalize()
        .unwrap();
        assert_eq!(provision.command_id, "provision-1");
        assert_eq!(provision.profile_id, "profile-1");

        let recovery = RecoverYardOrchestrator {
            command_id: " recover-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_orchestrator_version: 3,
        }
        .normalize()
        .unwrap();
        assert_eq!(recovery.command_id, "recover-1");

        let prompt = SendYardOrchestratorPrompt {
            command_id: " prompt-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_orchestrator_version: 3,
            orchestrator_worker_id: " worker-1 ".to_owned(),
            text: " Coordinate the portfolio. ".to_owned(),
        }
        .normalize()
        .unwrap();
        assert_eq!(prompt.text, "Coordinate the portfolio.");
    }

    #[test]
    fn rejects_zero_optimistic_versions() {
        let error = ConfigureYardOrchestrator {
            command_id: "configure-1".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: "worker-1".to_owned(),
            expected_worker_version: 1,
            expected_orchestrator_version: 0,
        }
        .normalize()
        .unwrap_err();
        assert_eq!(error, YardOrchestratorValidationError::InvalidVersion);
    }
}

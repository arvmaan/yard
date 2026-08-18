use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ACTOR_BYTES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticSummaryRequestKind {
    SuperintendentProject,
    ProjectWorker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpendSettings {
    pub superintendent_auto_requests_project_summaries: bool,
    pub project_orchestrators_auto_request_worker_summaries: bool,
    pub scheduled_automatic_summaries: bool,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub updated_by: String,
    pub updated_at_unix_ms: u64,
}

impl TokenSpendSettings {
    #[must_use]
    pub const fn automatic_summary_enabled(&self, kind: AutomaticSummaryRequestKind) -> bool {
        match kind {
            AutomaticSummaryRequestKind::SuperintendentProject => {
                self.superintendent_auto_requests_project_summaries
            }
            AutomaticSummaryRequestKind::ProjectWorker => {
                self.project_orchestrators_auto_request_worker_summaries
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTokenSpendSettings {
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub superintendent_auto_requests_project_summaries: bool,
    pub project_orchestrators_auto_request_worker_summaries: bool,
    pub scheduled_automatic_summaries: bool,
}

impl UpdateTokenSpendSettings {
    /// Normalize and validate a versioned token-spend settings update.
    ///
    /// # Errors
    ///
    /// Returns [`TokenSpendSettingsValidationError`] when the actor is blank
    /// or oversized, or the optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, TokenSpendSettingsValidationError> {
        let actor = self.actor.trim();
        if actor.is_empty() {
            return Err(TokenSpendSettingsValidationError::ActorRequired);
        }
        if actor.len() > MAX_ACTOR_BYTES {
            return Err(TokenSpendSettingsValidationError::ActorTooLong);
        }
        if self.expected_version == 0 {
            return Err(TokenSpendSettingsValidationError::InvalidVersion);
        }
        self.actor = actor.to_owned();
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TokenSpendSettingsValidationError {
    #[error("actor is required")]
    ActorRequired,
    #[error("actor exceeds {MAX_ACTOR_BYTES} bytes")]
    ActorTooLong,
    #[error("expected version must be greater than zero")]
    InvalidVersion,
}

#[cfg(test)]
mod tests {
    use super::{TokenSpendSettingsValidationError, UpdateTokenSpendSettings};

    #[test]
    fn normalizes_settings_update_actor() {
        let update = UpdateTokenSpendSettings {
            actor: " local-user ".to_owned(),
            expected_version: 1,
            superintendent_auto_requests_project_summaries: true,
            project_orchestrators_auto_request_worker_summaries: false,
            scheduled_automatic_summaries: false,
        }
        .normalize()
        .unwrap();

        assert_eq!(update.actor, "local-user");
    }

    #[test]
    fn rejects_zero_settings_version() {
        let error = UpdateTokenSpendSettings {
            actor: "local-user".to_owned(),
            expected_version: 0,
            superintendent_auto_requests_project_summaries: false,
            project_orchestrators_auto_request_worker_summaries: false,
            scheduled_automatic_summaries: false,
        }
        .normalize()
        .unwrap_err();

        assert_eq!(error, TokenSpendSettingsValidationError::InvalidVersion);
    }
}

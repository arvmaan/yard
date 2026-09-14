use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Assignment, Project, WorkerRuntimeBinding};

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_OBJECTIVE_BYTES: usize = 16_000;
const MAX_ROLE_BYTES: usize = 240;
const MAX_ARTIFACT_REF_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OldSessionDisposition {
    #[default]
    RetainForInspection,
    RetireAfterCutover,
    EndAfterCutover,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceProjectOrchestrator {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub expected_orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_worker_version: u64,
    pub expected_orchestrator_runtime: WorkerRuntimeBinding,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    pub objective: String,
    pub role: String,
    #[serde(default)]
    pub old_session_disposition: OldSessionDisposition,
    #[serde(default)]
    pub handoff_artifact_ref: Option<String>,
}

impl ReplaceProjectOrchestrator {
    /// Normalize an explicit project-orchestrator replacement command.
    ///
    /// The expected runtime is the complete binding displayed to the caller,
    /// not a status-derived completion signal.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorReplacementValidationError`] for blank,
    /// oversized, or zero-version input.
    pub fn normalize(mut self) -> Result<Self, OrchestratorReplacementValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.expected_orchestrator_worker_id = required(
            "expected_orchestrator_worker_id",
            &self.expected_orchestrator_worker_id,
            MAX_COMMAND_ID_BYTES,
        )?;
        self.profile_id = required("profile_id", &self.profile_id, MAX_COMMAND_ID_BYTES)?;
        self.objective = required("objective", &self.objective, MAX_OBJECTIVE_BYTES)?;
        self.role = required("role", &self.role, MAX_ROLE_BYTES)?;
        self.handoff_artifact_ref = self
            .handoff_artifact_ref
            .map(|value| required("handoff_artifact_ref", &value, MAX_ARTIFACT_REF_BYTES))
            .transpose()?;
        self.expected_orchestrator_runtime = self
            .expected_orchestrator_runtime
            .normalize()
            .map_err(|error| {
                OrchestratorReplacementValidationError::InvalidRuntime(error.to_string())
            })?;
        if let Some(provider) = self.expected_orchestrator_runtime.provider_session.as_mut() {
            provider.source =
                required("provider_session.source", &provider.source, MAX_ROLE_BYTES)?;
            provider.provider = required(
                "provider_session.provider",
                &provider.provider,
                MAX_ROLE_BYTES,
            )?;
            provider.kind = required("provider_session.kind", &provider.kind, MAX_ROLE_BYTES)?;
            provider.value = required(
                "provider_session.value",
                &provider.value,
                MAX_ARTIFACT_REF_BYTES,
            )?;
        }
        if self.expected_orchestrator_runtime.owns_tab
            && self.expected_orchestrator_runtime.tab_id.is_none()
        {
            return Err(OrchestratorReplacementValidationError::InvalidTabOwnership);
        }
        if self.expected_project_version == 0
            || self.expected_orchestrator_worker_version == 0
            || self.expected_orchestrator_runtime.version == 0
            || self.expected_profile_version == 0
        {
            return Err(OrchestratorReplacementValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplacedProjectOrchestrator {
    pub command_id: String,
    pub project: Project,
    pub assignment: Assignment,
    pub displaced_worker_id: String,
    pub old_session_disposition: OldSessionDisposition,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OrchestratorReplacementValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("expected versions must be greater than zero")]
    InvalidVersion,
    #[error("expected_orchestrator_runtime is invalid: {0}")]
    InvalidRuntime(String),
    #[error("a runtime that owns its tab must include tab_id")]
    InvalidTabOwnership,
}

fn required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, OrchestratorReplacementValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(OrchestratorReplacementValidationError::Required(field));
    }
    if value.len() > max {
        return Err(OrchestratorReplacementValidationError::TooLong { field, max });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use crate::{
        ObservedStatus, ProviderSessionRef, RuntimeObservationState, RuntimeProcessState,
        WorkerRuntimeBinding,
    };

    use super::{
        OldSessionDisposition, OrchestratorReplacementValidationError, ReplaceProjectOrchestrator,
    };

    fn runtime() -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            tab_id: Some("tab-1".to_owned()),
            pane_id: "pane-1".to_owned(),
            provider_session: Some(ProviderSessionRef {
                source: "codex".to_owned(),
                provider: "openai".to_owned(),
                kind: "thread".to_owned(),
                value: "session-1".to_owned(),
            }),
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Working,
            state_change_sequence: 3,
            revision: 4,
            version: 5,
            last_observed_at_unix_ms: 6,
        }
    }

    fn command() -> ReplaceProjectOrchestrator {
        ReplaceProjectOrchestrator {
            command_id: " replace-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_project_version: 2,
            expected_orchestrator_worker_id: " worker-1 ".to_owned(),
            expected_orchestrator_worker_version: 3,
            expected_orchestrator_runtime: runtime(),
            profile_id: " profile-1 ".to_owned(),
            expected_profile_version: 4,
            objective: " Continue orchestration with fresh context. ".to_owned(),
            role: " orchestrator ".to_owned(),
            old_session_disposition: OldSessionDisposition::RetireAfterCutover,
            handoff_artifact_ref: Some(" artifact://handoff ".to_owned()),
        }
    }

    #[test]
    fn normalizes_complete_identity_and_explicit_disposition() {
        let command = command().normalize().unwrap();

        assert_eq!(command.command_id, "replace-1");
        assert_eq!(command.expected_orchestrator_worker_id, "worker-1");
        assert_eq!(
            command.old_session_disposition,
            OldSessionDisposition::RetireAfterCutover
        );
        assert_eq!(
            command.handoff_artifact_ref.as_deref(),
            Some("artifact://handoff")
        );
        assert_eq!(command.expected_orchestrator_runtime.version, 5);
    }

    #[test]
    fn rejects_zero_runtime_version() {
        let mut command = command();
        command.expected_orchestrator_runtime.version = 0;

        assert_eq!(
            command.normalize(),
            Err(OrchestratorReplacementValidationError::InvalidVersion)
        );
    }

    #[test]
    fn accepts_legacy_runtime_without_provider_identity() {
        let mut command = command();
        command.expected_orchestrator_runtime.provider_session = None;

        let command = command.normalize().unwrap();

        assert!(
            command
                .expected_orchestrator_runtime
                .provider_session
                .is_none()
        );
    }

    #[test]
    fn rejects_owned_tab_without_tab_identity() {
        let mut command = command();
        command.expected_orchestrator_runtime.tab_id = None;

        assert_eq!(
            command.normalize(),
            Err(OrchestratorReplacementValidationError::InvalidTabOwnership)
        );
    }
}

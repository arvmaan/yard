use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Project, WorkerRuntimeBinding};

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_ID_BYTES: usize = 120;
const MAX_RUNTIME_IDENTITY_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProjectOrchestrator {
    pub command_id: String,
    pub actor: String,
    pub worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
    pub expected_worker_runtime: WorkerRuntimeBinding,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub expected_orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_worker_version: u64,
    pub expected_orchestrator_runtime: WorkerRuntimeBinding,
}

impl TransferProjectOrchestrator {
    /// Normalize and validate an atomic project-orchestrator role transfer.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorTransferValidationError`] for blank, oversized,
    /// incomplete, or zero-version optimistic input.
    pub fn normalize(mut self) -> Result<Self, OrchestratorTransferValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.worker_id = required("worker_id", &self.worker_id, MAX_ID_BYTES)?;
        self.expected_orchestrator_worker_id = required(
            "expected_orchestrator_worker_id",
            &self.expected_orchestrator_worker_id,
            MAX_ID_BYTES,
        )?;
        if self.worker_id == self.expected_orchestrator_worker_id {
            return Err(OrchestratorTransferValidationError::SameWorker);
        }
        self.expected_worker_runtime =
            normalize_runtime("expected_worker_runtime", self.expected_worker_runtime)?;
        self.expected_orchestrator_runtime = normalize_runtime(
            "expected_orchestrator_runtime",
            self.expected_orchestrator_runtime,
        )?;
        if self.expected_worker_version == 0
            || self.expected_project_version == 0
            || self.expected_orchestrator_worker_version == 0
            || self.expected_worker_runtime.version == 0
            || self.expected_orchestrator_runtime.version == 0
        {
            return Err(OrchestratorTransferValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferredProjectOrchestrator {
    pub command_id: String,
    pub project: Project,
    pub replaced_worker_id: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OrchestratorTransferValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("the replacement worker must differ from the current orchestrator")]
    SameWorker,
    #[error("expected versions must be greater than zero")]
    InvalidVersion,
    #[error("{field} is invalid: {message}")]
    InvalidRuntime {
        field: &'static str,
        message: String,
    },
    #[error("{field} owns its tab but does not include tab_id")]
    InvalidTabOwnership { field: &'static str },
}

fn normalize_runtime(
    field: &'static str,
    mut runtime: WorkerRuntimeBinding,
) -> Result<WorkerRuntimeBinding, OrchestratorTransferValidationError> {
    runtime = runtime.normalize().map_err(|error| {
        OrchestratorTransferValidationError::InvalidRuntime {
            field,
            message: error.to_string(),
        }
    })?;
    if runtime.owns_tab && runtime.tab_id.is_none() {
        return Err(OrchestratorTransferValidationError::InvalidTabOwnership { field });
    }
    if let Some(provider) = runtime.provider_session.as_mut() {
        provider.source = required(
            "provider_session.source",
            &provider.source,
            MAX_RUNTIME_IDENTITY_BYTES,
        )?;
        provider.provider = required(
            "provider_session.provider",
            &provider.provider,
            MAX_RUNTIME_IDENTITY_BYTES,
        )?;
        provider.kind = required(
            "provider_session.kind",
            &provider.kind,
            MAX_RUNTIME_IDENTITY_BYTES,
        )?;
        provider.value = required(
            "provider_session.value",
            &provider.value,
            MAX_RUNTIME_IDENTITY_BYTES,
        )?;
    }
    Ok(runtime)
}

fn required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, OrchestratorTransferValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(OrchestratorTransferValidationError::Required(field));
    }
    if value.len() > max_bytes {
        return Err(OrchestratorTransferValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use crate::{
        ObservedStatus, ProviderSessionRef, RuntimeObservationState, RuntimeProcessState,
        WorkerRuntimeBinding,
    };

    use super::{OrchestratorTransferValidationError, TransferProjectOrchestrator};

    fn runtime(terminal_id: &str, provider_value: &str) -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: terminal_id.to_owned(),
            tab_id: Some(format!("tab-{terminal_id}")),
            pane_id: format!("pane-{terminal_id}"),
            provider_session: Some(ProviderSessionRef {
                source: "codex".to_owned(),
                provider: "openai".to_owned(),
                kind: "thread".to_owned(),
                value: provider_value.to_owned(),
            }),
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Idle,
            state_change_sequence: 1,
            revision: 2,
            version: 3,
            last_observed_at_unix_ms: 4,
        }
    }

    fn command() -> TransferProjectOrchestrator {
        TransferProjectOrchestrator {
            command_id: " transfer-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            worker_id: " candidate ".to_owned(),
            expected_worker_version: 5,
            expected_worker_runtime: runtime("candidate", "session-candidate"),
            expected_project_version: 6,
            expected_orchestrator_worker_id: " current ".to_owned(),
            expected_orchestrator_worker_version: 7,
            expected_orchestrator_runtime: runtime("current", "session-current"),
        }
    }

    #[test]
    fn normalizes_complete_worker_and_orchestrator_snapshots() {
        let command = command().normalize().unwrap();

        assert_eq!(command.command_id, "transfer-1");
        assert_eq!(command.worker_id, "candidate");
        assert_eq!(command.expected_orchestrator_worker_id, "current");
    }

    #[test]
    fn rejects_same_worker_and_incomplete_runtime_topology() {
        let mut same_worker = command();
        same_worker.worker_id = "current".to_owned();
        assert_eq!(
            same_worker.normalize(),
            Err(OrchestratorTransferValidationError::SameWorker)
        );

        let mut incomplete = command();
        incomplete.expected_worker_runtime.tab_id = None;
        assert_eq!(
            incomplete.normalize(),
            Err(OrchestratorTransferValidationError::InvalidTabOwnership {
                field: "expected_worker_runtime"
            })
        );
    }

    #[test]
    fn rejects_zero_runtime_version() {
        let mut command = command();
        command.expected_worker_runtime.version = 0;

        assert_eq!(
            command.normalize(),
            Err(OrchestratorTransferValidationError::InvalidVersion)
        );
    }
}

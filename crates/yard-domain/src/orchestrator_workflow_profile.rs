use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS: u64 = 600_000;
pub const FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS: &str = r#"# Yard Orchestrator Workflow

Apply this workflow to every prompt delivered to the Yard orchestrator.

## Delegate through independent workers

1. Decompose the request into bounded lanes that can run independently.
2. Before creating workers, capture once the workspace ID that owns the central orchestrator pane. Reuse that exact ID for the entire fleet; never infer it from repository identity, current focus, or a `wN` naming convention.
3. Create one Yard/Herdr worker per lane as a separate tab in that captured workspace, and verify its observed workspace ID. Use an isolated git worktree for every write lane and a plain tab for read-only investigation. When the exact source checkout matters, create the worktree with Git and then create the Herdr tab with the captured workspace ID and worktree checkout as its CWD.
4. Give every worker complete context. Include the lane boundary, relevant facts and source paths, explicit exclusions, the expected artifact or commit, required checks, and the stopping condition. Workers start with no conversation context.
5. Require each worker to return conclusions, evidence, files changed, tests run with observed results, unresolved risks, and the recommended next action.

Do not perform the delegated implementation in the central orchestrator. Use the central session to classify, brief, monitor, intervene, collect, and reconcile the worker fleet.

## Monitor and intervene

Monitor the fleet every 10 minutes while the orchestrator is actively running. The configured monitor interval is 600000 milliseconds. This profile prescribes a cadence; it does not create backend wakeups or enable automatic prompts.

On each monitoring pass:

- inspect every worker's durable status and recent output;
- identify stalled work, missing evidence, scope drift, merge conflicts, and decisions the worker cannot make safely;
- send a concrete push-forward prompt that states the observed condition, the decision or next action, and the required artifact;
- avoid generic prompts such as "continue" or "give an update."

Automatic token-spending behavior remains opt-in and backend-enforced. Permission-bypass settings require a separate explicit grant. Never infer token-spend approval, permission to send automatic prompts, or permission-bypass settings from this workflow, its cadence, or the orchestrator's runtime settings.

## Collect deterministic output

Require write lanes to commit coherent changes and report the commit SHA. Require deterministic artifacts at named paths for research, logs, plans, or other non-code output. Do not treat terminal narration as the result.

After all lanes stop:

1. collect and review every artifact and commit;
2. reconcile overlaps and contradictions and choose the integration order;
3. integrate accepted commits in that order, inspect the combined diff, and preserve unrelated user changes;
4. run the final focused and repository-level tests, formatting, linting, and review appropriate to the change;
5. resolve failures or return an exact residual gap;
6. close completed worker lifecycles while retaining branches, worktrees, transcripts, and artifacts unless deletion was explicitly requested.

Return one integrated conclusion with files changed, exact tests and results, commit SHAs, residual risks or gaps, and integration order.
"#;

const MAX_ACTOR_BYTES: usize = 120;
const MAX_INSTRUCTIONS_BYTES: usize = 128 * 1024;
const MIN_MONITOR_INTERVAL_MS: u64 = 60_000;
const MAX_MONITOR_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestratorWorkflowProfileSource {
    Factory,
    User,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorWorkflowProfile {
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub instructions_markdown: String,
    #[serde(with = "crate::serde_u64")]
    pub monitor_interval_ms: u64,
    pub source: OrchestratorWorkflowProfileSource,
    pub updated_by: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOrchestratorWorkflowProfile {
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub instructions_markdown: String,
    #[serde(with = "crate::serde_u64")]
    pub monitor_interval_ms: u64,
}

impl UpdateOrchestratorWorkflowProfile {
    /// Normalize and validate an optimistic workflow-profile update.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileValidationError`] when a field is
    /// blank, oversized, or outside its supported range.
    pub fn normalize(mut self) -> Result<Self, OrchestratorWorkflowProfileValidationError> {
        self.actor = actor(&self.actor)?;
        self.instructions_markdown = instructions(&self.instructions_markdown)?;
        validate_version(self.expected_version)?;
        validate_monitor_interval(self.monitor_interval_ms)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetOrchestratorWorkflowProfile {
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

impl ResetOrchestratorWorkflowProfile {
    /// Normalize and validate an optimistic factory reset.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileValidationError`] when the actor
    /// or expected version is invalid.
    pub fn normalize(mut self) -> Result<Self, OrchestratorWorkflowProfileValidationError> {
        self.actor = actor(&self.actor)?;
        validate_version(self.expected_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OrchestratorWorkflowProfileValidationError {
    #[error("actor is required")]
    ActorRequired,
    #[error("actor exceeds {MAX_ACTOR_BYTES} bytes")]
    ActorTooLong,
    #[error("instructions_markdown is required")]
    InstructionsRequired,
    #[error("instructions_markdown exceeds {MAX_INSTRUCTIONS_BYTES} bytes")]
    InstructionsTooLong,
    #[error("expected_version must be greater than zero")]
    InvalidVersion,
    #[error(
        "monitor_interval_ms must be between {MIN_MONITOR_INTERVAL_MS} and {MAX_MONITOR_INTERVAL_MS}"
    )]
    InvalidMonitorInterval,
}

fn actor(value: &str) -> Result<String, OrchestratorWorkflowProfileValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(OrchestratorWorkflowProfileValidationError::ActorRequired);
    }
    if value.len() > MAX_ACTOR_BYTES {
        return Err(OrchestratorWorkflowProfileValidationError::ActorTooLong);
    }
    Ok(value.to_owned())
}

fn instructions(value: &str) -> Result<String, OrchestratorWorkflowProfileValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(OrchestratorWorkflowProfileValidationError::InstructionsRequired);
    }
    if value.len() > MAX_INSTRUCTIONS_BYTES {
        return Err(OrchestratorWorkflowProfileValidationError::InstructionsTooLong);
    }
    Ok(value.to_owned())
}

fn validate_version(version: u64) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    if version == 0 {
        return Err(OrchestratorWorkflowProfileValidationError::InvalidVersion);
    }
    Ok(())
}

fn validate_monitor_interval(
    interval: u64,
) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    if !(MIN_MONITOR_INTERVAL_MS..=MAX_MONITOR_INTERVAL_MS).contains(&interval) {
        return Err(OrchestratorWorkflowProfileValidationError::InvalidMonitorInterval);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS,
        FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS, UpdateOrchestratorWorkflowProfile,
    };

    #[test]
    fn factory_workflow_covers_the_standard_orchestration_contract() {
        let instructions = FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS;

        assert!(instructions.contains("one Yard/Herdr worker per lane"));
        assert!(instructions.contains("workspace ID that owns the central orchestrator pane"));
        assert!(instructions.contains("separate tab in that captured workspace"));
        assert!(instructions.contains("isolated git worktree for every write lane"));
        assert!(instructions.contains("plain tab for read-only investigation"));
        assert!(
            instructions
                .contains("repository identity, current focus, or a `wN` naming convention")
        );
        assert!(instructions.contains("complete context"));
        assert!(instructions.contains("every 10 minutes"));
        assert!(instructions.contains("concrete push-forward prompt"));
        assert!(instructions.contains("commit SHA"));
        assert!(instructions.contains("reconcile overlaps"));
        assert!(instructions.contains("run the final focused and repository-level tests"));
        assert!(instructions.contains("close completed worker lifecycles"));
        assert!(instructions.contains("does not create backend wakeups"));
        assert!(instructions.contains("Automatic token-spending behavior remains opt-in"));
        assert!(
            instructions.contains("Permission-bypass settings require a separate explicit grant")
        );
        assert!(instructions.contains("Never infer token-spend approval"));
        assert!(instructions.contains("integrate accepted commits"));
        assert_eq!(FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS, 600_000);
    }

    #[test]
    fn normalizes_an_edit_without_changing_its_semantics() {
        let update = UpdateOrchestratorWorkflowProfile {
            actor: " local-user ".to_owned(),
            expected_version: 1,
            instructions_markdown: " # Custom workflow ".to_owned(),
            monitor_interval_ms: 900_000,
        }
        .normalize()
        .unwrap();

        assert_eq!(update.actor, "local-user");
        assert_eq!(update.instructions_markdown, "# Custom workflow");
        assert_eq!(update.monitor_interval_ms, 900_000);
    }
}

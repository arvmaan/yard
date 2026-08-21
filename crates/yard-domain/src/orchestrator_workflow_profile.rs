use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const YARD_STANDARD_ORCHESTRATOR_PROFILE_ID: &str = "yard:standard-orchestrator";
pub const YARD_STANDARD_ORCHESTRATOR_PROFILE_NAME: &str = "Yard Standard Orchestrator";
pub const YARD_STANDARD_ORCHESTRATOR_PROFILE_DESCRIPTION: &str =
    "Provider-neutral orchestration with durable worker outputs and independent quality review.";
pub const FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS: u64 = 600_000;
pub const FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS: &str = r#"# Yard Standard Orchestrator

Apply this workflow to every prompt delivered to a Yard orchestrator through
the runtime adapter's declared capabilities. The workflow contract is
provider- and harness-neutral. Repository prompt files such as `AGENTS.md` and
`CLAUDE.md` may be loaded as optional adapter context when available, but they
are not workflow sources and are never required.

## Delegate through independent workers

1. Decompose the objective into bounded lanes that can run independently, with
   explicit inputs, outputs, exclusions, required checks, and stopping
   conditions.
2. Before creating workers, capture once the workspace ID that owns the
   originating central orchestrator pane. Reuse that exact ID for the entire
   fleet; never infer it from repository identity, current focus, or a `wN`
   naming convention.
3. Allocate independent workers to lanes that can proceed concurrently. With
   the current Herdr capability adapter, create one worker per lane as a
   separate tab in that exact captured orchestrator workspace and verify its
   observed workspace ID. Use an isolated Git worktree for every write lane and
   a plain tab for read-only investigation. When the exact source checkout
   matters, create the worktree with Git and then create the Herdr tab with the
   captured workspace ID and worktree checkout as its CWD.
4. Give every worker complete context. Include the lane boundary, relevant
   facts and source paths, explicit exclusions, the expected named artifact or
   coherent commit IDs, required checks, and the stopping condition. Workers
   start with no conversation context.
5. Require each worker to return conclusions, evidence, files changed, tests
   run with observed results, unresolved risks, and the recommended next
   action.

Do not perform delegated implementation in the central orchestrator. Use the
central session to classify, brief, monitor, intervene, collect, review,
reconcile, integrate, and verify the worker fleet.

## Monitor and intervene

Monitor durable worker status and recent output every 10 minutes at the
configured cadence while the orchestrator is actively running. The configured
monitor interval is 600000 milliseconds. This profile prescribes a cadence; it
does not create backend wakeups, spend tokens, or authorize automatic prompts.

On each monitoring pass:

- inspect every worker's durable status and recent output;
- identify stalled work, missing evidence, scope drift, merge conflicts, and
  decisions the worker cannot make safely;
- send a concrete push-forward prompt that states the concrete observation,
  the decision or next action, and the required artifact;
- avoid generic prompts such as "continue" or "give an update."

All automatic token-spending paths have separate durable settings. These
independent durable settings are backend-enforced and default off. Runtime
permission-bypass settings require a separate explicit grant. Never infer
token-spend approval, permission to send automatic prompts, or permission
bypass from this workflow, its cadence, or runtime settings.
Manual observation, allocation, intervention, collection, review,
reconciliation, integration, and verification remain available.

## Collect deterministic output and review independently

Require write lanes to commit coherent changes and report the commit SHA.
Require deterministic artifacts at named paths for research, logs, plans, or
other non-code output. Do not treat terminal narration as the result.

Obtain independent quality review from a reviewer that did not produce the
work. Give the reviewer the accepted scope, artifacts, commits, checks, and
risks, and require concrete findings with evidence.

After all lanes and the independent review stop:

1. collect and review every artifact, commit, and review finding;
2. reconcile overlaps and contradictions and choose the integration order;
3. integrate accepted commits in that order, inspect the combined diff, and
   preserve unrelated user changes;
4. run the final focused and repository-level tests, formatting, linting, and
   review gates appropriate to the change;
5. resolve failures or return an exact residual gap;
6. close completed worker lifecycles while retaining branches, worktrees,
   transcripts, and artifacts unless deletion was explicitly requested.

Return one integrated conclusion with files changed, exact tests and results,
commit SHAs, residual risks or gaps, and integration order.
"#;

const MAX_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_INSTRUCTIONS_BYTES: usize = 128 * 1024;
const MAX_COMMANDS: usize = 32;
const MAX_CONTEXT_FILES: usize = 32;
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
pub struct OrchestratorWorkflowCommand {
    pub id: String,
    pub capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorWorkflowAdapterContextFile {
    pub adapter_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorWorkflowProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub instructions_markdown: String,
    #[serde(with = "crate::serde_u64")]
    pub monitor_interval_ms: u64,
    pub commands: Vec<OrchestratorWorkflowCommand>,
    #[serde(default)]
    pub adapter_context_files: Vec<OrchestratorWorkflowAdapterContextFile>,
    pub source: OrchestratorWorkflowProfileSource,
    pub updated_by: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorWorkflowProfiles {
    pub profiles: Vec<OrchestratorWorkflowProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOrchestratorWorkflowProfile {
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub instructions_markdown: String,
    #[serde(with = "crate::serde_u64")]
    pub monitor_interval_ms: u64,
    #[serde(default)]
    pub commands: Option<Vec<OrchestratorWorkflowCommand>>,
    #[serde(default)]
    pub adapter_context_files: Option<Vec<OrchestratorWorkflowAdapterContextFile>>,
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
        if let Some(commands) = &mut self.commands {
            normalize_commands(commands)?;
        }
        if let Some(context_files) = &mut self.adapter_context_files {
            normalize_context_files(context_files)?;
        }
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
    #[error("workflow commands must contain between 1 and {MAX_COMMANDS} entries")]
    InvalidCommands,
    #[error(
        "workflow commands must use unique, supported ID and capability pairs with fields no longer than {MAX_ID_BYTES} bytes"
    )]
    InvalidCommand,
    #[error("adapter context files must contain at most {MAX_CONTEXT_FILES} entries")]
    TooManyContextFiles,
    #[error("adapter context file fields must be non-empty and at most {MAX_ID_BYTES} bytes")]
    InvalidContextFile,
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

fn normalize_commands(
    commands: &mut [OrchestratorWorkflowCommand],
) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    if commands.is_empty() || commands.len() > MAX_COMMANDS {
        return Err(OrchestratorWorkflowProfileValidationError::InvalidCommands);
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut capabilities = std::collections::BTreeSet::new();
    for command in &mut *commands {
        command.id = bounded_value(&command.id)
            .ok_or(OrchestratorWorkflowProfileValidationError::InvalidCommand)?;
        command.capability = bounded_value(&command.capability)
            .ok_or(OrchestratorWorkflowProfileValidationError::InvalidCommand)?;
        if !ids.insert(command.id.clone()) || !capabilities.insert(command.capability.clone()) {
            return Err(OrchestratorWorkflowProfileValidationError::InvalidCommand);
        }
    }
    validate_orchestrator_workflow_commands(commands)
}

fn normalize_context_files(
    files: &mut [OrchestratorWorkflowAdapterContextFile],
) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    if files.len() > MAX_CONTEXT_FILES {
        return Err(OrchestratorWorkflowProfileValidationError::TooManyContextFiles);
    }
    for file in files {
        file.adapter_id = bounded_value(&file.adapter_id)
            .ok_or(OrchestratorWorkflowProfileValidationError::InvalidContextFile)?;
        file.path = bounded_value(&file.path)
            .ok_or(OrchestratorWorkflowProfileValidationError::InvalidContextFile)?;
    }
    Ok(())
}

fn bounded_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= MAX_ID_BYTES).then(|| value.to_owned())
}

#[must_use]
pub fn yard_standard_orchestrator_commands() -> Vec<OrchestratorWorkflowCommand> {
    [
        ("work.decompose", "orchestration.work.decompose"),
        ("worker.allocate", "orchestration.worker.allocate"),
        ("worker.observe", "orchestration.worker.observe"),
        ("worker.intervene", "orchestration.worker.prompt"),
        ("result.collect", "orchestration.result.collect"),
        ("quality.review", "orchestration.quality.review"),
        ("result.reconcile", "orchestration.result.reconcile"),
        ("result.integrate", "orchestration.result.integrate"),
        ("result.verify", "orchestration.result.verify"),
    ]
    .into_iter()
    .map(|(id, capability)| OrchestratorWorkflowCommand {
        id: id.to_owned(),
        capability: capability.to_owned(),
    })
    .collect()
}

/// Validate the complete provider-neutral command contract used by runtime
/// adapters and newly written revisions.
///
/// Every canonical command must be present exactly once and in canonical
/// order.
///
/// # Errors
///
/// Returns [`OrchestratorWorkflowProfileValidationError::InvalidCommands`] for
/// an incomplete or oversized list and
/// [`OrchestratorWorkflowProfileValidationError::InvalidCommand`] for an
/// unsupported, mismatched, or duplicate pair.
pub fn validate_orchestrator_workflow_commands(
    commands: &[OrchestratorWorkflowCommand],
) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    validate_stored_orchestrator_workflow_commands(commands)?;
    if commands != yard_standard_orchestrator_commands() {
        return Err(OrchestratorWorkflowProfileValidationError::InvalidCommands);
    }
    Ok(())
}

/// Validate commands read from an immutable historical revision.
///
/// Legacy revisions may have no structured commands or a supported canonical
/// subset. They remain inspectable, but executable paths must additionally
/// call [`validate_orchestrator_workflow_commands`].
///
/// # Errors
///
/// Returns [`OrchestratorWorkflowProfileValidationError::InvalidCommands`] for
/// an oversized list and
/// [`OrchestratorWorkflowProfileValidationError::InvalidCommand`] for an
/// unsupported, mismatched, or duplicate pair.
pub fn validate_stored_orchestrator_workflow_commands(
    commands: &[OrchestratorWorkflowCommand],
) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    if commands.len() > MAX_COMMANDS {
        return Err(OrchestratorWorkflowProfileValidationError::InvalidCommands);
    }
    let supported = yard_standard_orchestrator_commands();
    let mut ids = std::collections::BTreeSet::new();
    let mut capabilities = std::collections::BTreeSet::new();
    for command in commands {
        if !supported.contains(command)
            || !ids.insert(command.id.as_str())
            || !capabilities.insert(command.capability.as_str())
        {
            return Err(OrchestratorWorkflowProfileValidationError::InvalidCommand);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS,
        FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS, UpdateOrchestratorWorkflowProfile,
        validate_orchestrator_workflow_commands, validate_stored_orchestrator_workflow_commands,
        yard_standard_orchestrator_commands,
    };

    #[test]
    fn factory_workflow_covers_the_standard_orchestration_contract() {
        let instructions = FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS;

        assert!(instructions.contains("Decompose the objective"));
        assert!(instructions.contains("Allocate independent workers"));
        assert!(instructions.contains("workspace ID that owns the"));
        assert!(instructions.contains("originating central orchestrator pane"));
        assert!(instructions.contains("exact captured orchestrator workspace"));
        assert!(instructions.contains("observed workspace ID"));
        assert!(instructions.contains("repository identity, current focus, or a `wN`"));
        assert!(instructions.contains("isolated Git worktree for every write lane"));
        assert!(instructions.contains("plain tab for read-only investigation"));
        assert!(instructions.contains("Give every worker complete context"));
        assert!(instructions.contains("Do not perform delegated implementation"));
        assert!(instructions.contains("every 10 minutes"));
        assert!(instructions.contains("concrete push-forward prompt"));
        assert!(instructions.contains("coherent commit IDs"));
        assert!(instructions.contains("deterministic artifacts at named paths"));
        assert!(instructions.contains("independent quality review"));
        assert!(instructions.contains("reconcile overlaps and contradictions"));
        assert!(instructions.contains("integrate accepted commits in that order"));
        assert!(instructions.contains("final focused and repository-level tests"));
        assert!(instructions.contains("retaining branches, worktrees"));
        assert!(instructions.contains("provider- and harness-neutral"));
        assert!(instructions.contains("optional adapter context"));
        assert!(instructions.contains("not workflow sources and are never required"));
        assert!(instructions.contains("does not create backend"));
        assert!(instructions.contains("separate durable"));
        assert!(instructions.contains("independent durable settings"));
        assert!(instructions.contains("backend-enforced"));
        assert!(instructions.contains("default off"));
        assert!(instructions.contains("permission-bypass settings"));
        assert!(instructions.contains("Manual observation"));
        assert_eq!(FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS, 600_000);

        let commands = yard_standard_orchestrator_commands();
        assert_eq!(
            commands
                .iter()
                .map(|command| (command.id.as_str(), command.capability.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("work.decompose", "orchestration.work.decompose"),
                ("worker.allocate", "orchestration.worker.allocate"),
                ("worker.observe", "orchestration.worker.observe"),
                ("worker.intervene", "orchestration.worker.prompt"),
                ("result.collect", "orchestration.result.collect"),
                ("quality.review", "orchestration.quality.review"),
                ("result.reconcile", "orchestration.result.reconcile"),
                ("result.integrate", "orchestration.result.integrate"),
                ("result.verify", "orchestration.result.verify"),
            ]
        );
        assert!(
            commands
                .iter()
                .all(|command| !command.id.contains("herdr") && !command.id.contains("claude"))
        );
    }

    #[test]
    fn normalizes_an_edit_without_changing_its_semantics() {
        let update = UpdateOrchestratorWorkflowProfile {
            actor: " local-user ".to_owned(),
            expected_version: 1,
            instructions_markdown: " # Custom workflow ".to_owned(),
            monitor_interval_ms: 900_000,
            commands: None,
            adapter_context_files: None,
        }
        .normalize()
        .unwrap();

        assert_eq!(update.actor, "local-user");
        assert_eq!(update.instructions_markdown, "# Custom workflow");
        assert_eq!(update.monitor_interval_ms, 900_000);
    }

    #[test]
    fn rejects_unsupported_and_mismatched_runtime_capabilities() {
        let mut commands = yard_standard_orchestrator_commands();
        commands[0].capability = "orchestration.result.verify".to_owned();
        assert!(validate_orchestrator_workflow_commands(&commands).is_err());

        commands[0].capability = "orchestration.unsupported".to_owned();
        assert!(validate_orchestrator_workflow_commands(&commands).is_err());
    }

    #[test]
    fn historical_subsets_are_inspectable_but_not_executable() {
        let mut commands = yard_standard_orchestrator_commands();
        commands.remove(7);

        validate_stored_orchestrator_workflow_commands(&commands).unwrap();
        validate_stored_orchestrator_workflow_commands(&[]).unwrap();
        assert!(validate_orchestrator_workflow_commands(&commands).is_err());
        assert!(validate_orchestrator_workflow_commands(&[]).is_err());
    }
}

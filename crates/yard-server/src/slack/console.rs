//! Production [`AgentConsole`]: Yard's own guarded paths, nothing new.
//!
//! - identity and blocked state: the durable binding via
//!   `InterventionService` / `CoordinationNodeService`, plus the same
//!   binding validation the prompt and terminal routes use;
//! - screen: the bounded `read_*_output` reads (Herdr `pane.read`,
//!   `recent_unwrapped`, ANSI stripped), which re-validate the binding;
//! - keys: `TerminalService::open_*` (the terminal WebSocket's lease path,
//!   which fails if the owner holds control in Yard), the lease identity
//!   compared with the button's, `validate_lease` before every key, one
//!   `terminal.input` per key, then release;
//! - free text: the direct prompt commands (assignment / orchestrator /
//!   Yard orchestrator / workstream node) with optimistic versions, so the
//!   store's pending-prompt and version guards apply;
//! - answers: the same bounded orchestrator output reads, scanned only for
//!   the status report of the prompt's own command id;
//! - refusals before any key or prompt: a managed pane whose Herdr pane
//!   lease needs recovery (as the UI marks it), and isolated
//!   (`system_ephemeral`) summary workers, which take no owner input.

use std::{sync::Arc, time::Duration};
use yard_domain::{TerminalOutputFormat, Worker, WorkerOwnershipKind};

use async_trait::async_trait;
use tokio::time::timeout;
use yard_domain::{
    AssignmentLifecycle, AttemptLifecycle, ObservedStatus, OrchestratorStatusReport,
    ProviderSessionRef, RuntimeObservationState, RuntimeProcessState, SendAssignmentPrompt,
    SendCoordinationNodePrompt, SendOrchestratorPrompt, SendYardOrchestratorPrompt,
    WorkerRuntimeBinding,
};

use super::{
    actions::{AgentConsole, AgentSnapshot, AgentTarget, ConsoleError, KeyPlan, TerminalIdentity},
    prompt::SCREEN_LINES,
    relay::{REPORT_LINES, scan_report},
};
use yard_store::{StoredPaneManagementLease, YardStore};

use crate::{
    coordination_node_service::{CoordinationNodeService, CoordinationNodeServiceError},
    intervention_service::{InterventionService, InterventionServiceError},
    terminal_service::{
        OpenedTerminal, RuntimeTerminalError, TerminalClientMessage, TerminalService,
        TerminalServiceError,
    },
};

/// Controller viewport while typing (Herdr restores the pane afterwards).
const TYPE_COLS: u16 = 120;
const TYPE_ROWS: u16 = 40;
/// Pause after each key, reading frames so Herdr's output never backs up.
const KEY_SPACING: Duration = Duration::from_millis(80);
const TERMINAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the pane may take to show the cursor on the option before
/// Enter is withheld.
const CURSOR_SETTLE: Duration = Duration::from_millis(1_500);

/// The UI's wording for a managed pane whose lease Yard lost
/// (`pane_management_service`).
pub(crate) const LEASE_RECOVERY_REQUIRED: &str = "Yard lost the pane lease; recovery is required.";
pub(crate) const SUMMARY_WORKER_REFUSED: &str =
    "This is an isolated summary worker; Yard does not send it input.";

#[derive(Clone)]
pub struct YardConsole {
    interventions: InterventionService,
    terminals: TerminalService,
    nodes: CoordinationNodeService,
    store: Arc<dyn YardStore>,
}

pub(crate) fn provider_key(session: &ProviderSessionRef) -> String {
    format!(
        "{}:{}:{}:{}",
        session.source, session.provider, session.kind, session.value
    )
}

fn identity_of(worker_id: &str, binding: &WorkerRuntimeBinding) -> TerminalIdentity {
    TerminalIdentity {
        worker_id: worker_id.to_owned(),
        terminal_id: binding.terminal_id.clone(),
        pane_id: binding.pane_id.clone(),
        tab_id: binding.tab_id.clone(),
        provider_session: binding.provider_session.as_ref().map(provider_key),
    }
}

fn is_blocked(binding: &WorkerRuntimeBinding) -> bool {
    binding.status == ObservedStatus::Blocked
        && binding.observation_state == RuntimeObservationState::Observed
        && binding.process_state != RuntimeProcessState::Exited
}

/// Only an active assignment on its active attempt can be answered.
fn require_active(
    assignment: AssignmentLifecycle,
    attempt: AttemptLifecycle,
) -> Result<(), ConsoleError> {
    if assignment == AssignmentLifecycle::Active && attempt == AttemptLifecycle::Active {
        Ok(())
    } else {
        Err(ConsoleError::Gone(
            "the assignment is not active".to_owned(),
        ))
    }
}

fn intervention_error(error: InterventionServiceError) -> ConsoleError {
    use InterventionServiceError as E;
    match error {
        E::AssignmentNotFound | E::AssignmentNotActive | E::AttemptNotCurrent { .. } => {
            ConsoleError::Gone(error.to_string())
        }
        E::RuntimeBindingMissing
        | E::RuntimeBindingStale
        | E::OrchestratorChanged
        | E::YardOrchestratorChanged
        | E::AssignmentVersionConflict { .. }
        | E::AttemptVersionConflict { .. } => ConsoleError::IdentityChanged(error.to_string()),
        other => ConsoleError::Failed(other.to_string()),
    }
}

#[allow(clippy::needless_pass_by_value)] // a `map_err` adapter
fn node_error(error: CoordinationNodeServiceError) -> ConsoleError {
    ConsoleError::Failed(error.to_string())
}

fn terminal_error(error: TerminalServiceError) -> ConsoleError {
    use TerminalServiceError as E;
    match error {
        E::Intervention(error) => intervention_error(error),
        E::Runtime(RuntimeTerminalError::Ownership(message)) => ConsoleError::Busy(message),
        E::AssignmentNotActive
        | E::YardOrchestratorNotConfigured
        | E::CoordinationNodeNotProvisioned => ConsoleError::Gone(error.to_string()),
        E::OrchestratorChanged
        | E::YardOrchestratorChanged
        | E::CoordinationNodeChanged
        | E::RuntimeBindingMissing
        | E::RuntimeBindingChanged => ConsoleError::IdentityChanged(error.to_string()),
        other => ConsoleError::Failed(other.to_string()),
    }
}

fn gone(what: &str) -> ConsoleError {
    ConsoleError::Gone(format!("{what} has no running agent"))
}

impl YardConsole {
    #[must_use]
    pub fn new(
        interventions: InterventionService,
        terminals: TerminalService,
        nodes: CoordinationNodeService,
        store: Arc<dyn YardStore>,
    ) -> Self {
        Self {
            interventions,
            terminals,
            nodes,
            store,
        }
    }
}

/// What the typing refusals need to know about the resolved worker.
struct Guard {
    worker_id: String,
    ownership_kind: WorkerOwnershipKind,
}

impl Guard {
    fn of(worker: &Worker) -> Self {
        Self {
            worker_id: worker.id.clone(),
            ownership_kind: worker.ownership_kind,
        }
    }
}

/// Refuse to type when the UI would: an isolated summary worker, or a
/// managed pane whose lease needs recovery (flagged, or already expired,
/// exactly as `pane_management_service` classifies it).
fn refusal_for(
    guard: &Guard,
    leases: &[StoredPaneManagementLease],
    now_unix_ms: u64,
) -> Result<(), ConsoleError> {
    if guard.ownership_kind == WorkerOwnershipKind::SystemEphemeral {
        return Err(ConsoleError::Unavailable(SUMMARY_WORKER_REFUSED.to_owned()));
    }
    let needs_recovery = leases.iter().any(|lease| {
        lease.worker_id == guard.worker_id
            && (lease.recovery_required || lease.expires_at_unix_ms <= now_unix_ms)
    });
    if needs_recovery {
        return Err(ConsoleError::Unavailable(
            LEASE_RECOVERY_REQUIRED.to_owned(),
        ));
    }
    Ok(())
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The versions a prompt command must carry.
enum PromptRef {
    Assignment {
        attempt_id: String,
        assignment_version: u64,
        attempt_version: u64,
    },
    Orchestrator {
        project_version: u64,
        worker_id: String,
    },
    Yard {
        version: u64,
        worker_id: String,
    },
    Node {
        version: u64,
        worker_id: String,
    },
}

struct Resolved {
    snapshot: AgentSnapshot,
    prompt: PromptRef,
    guard: Guard,
}

impl YardConsole {
    #[allow(clippy::too_many_lines)] // one arm per agent kind
    async fn resolve(&self, target: &AgentTarget) -> Result<Resolved, ConsoleError> {
        match target {
            AgentTarget::Assignment {
                project_id,
                assignment_id,
            } => {
                let assignment = self
                    .interventions
                    .assignment(project_id, assignment_id)
                    .await
                    .map_err(intervention_error)?;
                require_active(assignment.lifecycle, assignment.attempt.lifecycle)?;
                self.interventions
                    .validate_runtime(&assignment)
                    .await
                    .map_err(intervention_error)?;
                let binding = assignment
                    .worker
                    .runtime
                    .as_ref()
                    .ok_or_else(|| gone("the assignment"))?;
                Ok(Resolved {
                    snapshot: AgentSnapshot {
                        identity: identity_of(&assignment.worker.id, binding),
                        blocked: is_blocked(binding),
                    },
                    prompt: PromptRef::Assignment {
                        attempt_id: assignment.attempt.id.clone(),
                        assignment_version: assignment.version,
                        attempt_version: assignment.attempt.version,
                    },
                    guard: Guard::of(&assignment.worker),
                })
            }
            AgentTarget::ProjectOrchestrator { project_id } => {
                let project = self
                    .interventions
                    .project(project_id)
                    .await
                    .map_err(intervention_error)?;
                self.interventions
                    .validate_orchestrator_binding(&project)
                    .await
                    .map_err(intervention_error)?;
                let worker = &project.orchestrator;
                let binding = worker
                    .runtime
                    .as_ref()
                    .ok_or_else(|| gone("the project orchestrator"))?;
                Ok(Resolved {
                    snapshot: AgentSnapshot {
                        identity: identity_of(&worker.id, binding),
                        blocked: is_blocked(binding),
                    },
                    prompt: PromptRef::Orchestrator {
                        project_version: project.version,
                        worker_id: worker.id.clone(),
                    },
                    guard: Guard::of(worker),
                })
            }
            AgentTarget::YardOrchestrator => {
                let orchestrator = self
                    .interventions
                    .yard_orchestrator()
                    .await
                    .map_err(intervention_error)?;
                self.interventions
                    .validate_yard_orchestrator_binding(&orchestrator)
                    .await
                    .map_err(intervention_error)?;
                let worker = orchestrator
                    .worker
                    .as_ref()
                    .ok_or_else(|| gone("the Superintendent"))?;
                let binding = worker
                    .runtime
                    .as_ref()
                    .ok_or_else(|| gone("the Superintendent"))?;
                Ok(Resolved {
                    snapshot: AgentSnapshot {
                        identity: identity_of(&worker.id, binding),
                        blocked: is_blocked(binding),
                    },
                    prompt: PromptRef::Yard {
                        version: orchestrator.version,
                        worker_id: worker.id.clone(),
                    },
                    guard: Guard::of(worker),
                })
            }
            AgentTarget::CoordinationNode { node_id } => {
                let node = self.nodes.get(node_id).await.map_err(node_error)?;
                self.nodes
                    .validate_node_binding(&node)
                    .await
                    .map_err(node_error)?;
                let worker = node.worker.as_ref().ok_or_else(|| gone("the workstream"))?;
                let binding = worker
                    .runtime
                    .as_ref()
                    .ok_or_else(|| gone("the workstream"))?;
                Ok(Resolved {
                    snapshot: AgentSnapshot {
                        identity: identity_of(&worker.id, binding),
                        blocked: is_blocked(binding),
                    },
                    prompt: PromptRef::Node {
                        version: node.version,
                        worker_id: worker.id.clone(),
                    },
                    guard: Guard::of(worker),
                })
            }
        }
    }

    async fn open(&self, target: &AgentTarget) -> Result<OpenedTerminal, ConsoleError> {
        let opened = match target {
            AgentTarget::Assignment {
                project_id,
                assignment_id,
            } => {
                self.terminals
                    .open(project_id, assignment_id, TYPE_COLS, TYPE_ROWS)
                    .await
            }
            AgentTarget::ProjectOrchestrator { project_id } => {
                self.terminals
                    .open_orchestrator(project_id, TYPE_COLS, TYPE_ROWS)
                    .await
            }
            AgentTarget::YardOrchestrator => {
                self.terminals
                    .open_yard_orchestrator(TYPE_COLS, TYPE_ROWS)
                    .await
            }
            AgentTarget::CoordinationNode { node_id } => {
                self.terminals
                    .open_coordination_node(node_id, TYPE_COLS, TYPE_ROWS)
                    .await
            }
        };
        opened.map_err(terminal_error)
    }
}

impl YardConsole {
    async fn timed_resolve(&self, target: &AgentTarget) -> Result<Resolved, ConsoleError> {
        timeout(TERMINAL_TIMEOUT, self.resolve(target))
            .await
            .map_err(|_| ConsoleError::Failed("reading the agent timed out".to_owned()))?
    }

    /// See [`refusal_for`]; reads the managed-pane leases now.
    async fn refuse_unsafe(&self, guard: &Guard) -> Result<(), ConsoleError> {
        let leases = timeout(TERMINAL_TIMEOUT, self.store.list_pane_management_leases())
            .await
            .map_err(|_| ConsoleError::Failed("reading pane leases timed out".to_owned()))?
            .map_err(|error| ConsoleError::Failed(error.to_string()))?;
        refusal_for(guard, &leases, now_unix_ms())
    }

    async fn timed_screen(&self, target: &AgentTarget) -> Result<String, ConsoleError> {
        timeout(TERMINAL_TIMEOUT, self.screen(target))
            .await
            .map_err(|_| ConsoleError::Failed("reading the pane timed out".to_owned()))?
    }

    /// Wait (briefly) until the pane shows the cursor on the option.
    async fn await_cursor(&self, target: &AgentTarget, plan: &KeyPlan) -> Result<(), ConsoleError> {
        let deadline = tokio::time::Instant::now() + CURSOR_SETTLE;
        loop {
            let checked = plan.check_before_enter(&self.timed_screen(target).await?);
            if checked.is_ok() || tokio::time::Instant::now() >= deadline {
                return checked;
            }
            tokio::time::sleep(KEY_SPACING).await;
        }
    }
}

fn lease_matches(opened: &OpenedTerminal, identity: &TerminalIdentity) -> bool {
    let (worker_id, terminal_id, pane_id, tab_id, provider_session) =
        opened.lease.granted_identity();
    worker_id == identity.worker_id
        && terminal_id == identity.terminal_id
        && pane_id == identity.pane_id
        && tab_id == identity.tab_id.as_deref()
        && provider_session.map(provider_key) == identity.provider_session
}

/// Read frames for `period` so the controller's output never backs up.
async fn drain(opened: &mut OpenedTerminal, period: Duration) {
    let deadline = tokio::time::Instant::now() + period;
    while let Ok(Ok(Some(_))) =
        tokio::time::timeout_at(deadline, opened.session.next_message()).await
    {}
}

#[async_trait]
impl AgentConsole for YardConsole {
    async fn snapshot(&self, target: &AgentTarget) -> Result<AgentSnapshot, ConsoleError> {
        Ok(self.resolve(target).await?.snapshot)
    }

    async fn screen(&self, target: &AgentTarget) -> Result<String, ConsoleError> {
        let text = match target {
            AgentTarget::Assignment {
                project_id,
                assignment_id,
            } => {
                self.interventions
                    .read_output(
                        project_id,
                        assignment_id,
                        SCREEN_LINES,
                        TerminalOutputFormat::Text,
                    )
                    .await
                    .map_err(intervention_error)?
                    .text
            }
            AgentTarget::ProjectOrchestrator { project_id } => {
                self.interventions
                    .read_orchestrator_output(project_id, SCREEN_LINES, TerminalOutputFormat::Text)
                    .await
                    .map_err(intervention_error)?
                    .text
            }
            AgentTarget::YardOrchestrator => {
                self.interventions
                    .read_yard_orchestrator_output(SCREEN_LINES, TerminalOutputFormat::Text)
                    .await
                    .map_err(intervention_error)?
                    .text
            }
            AgentTarget::CoordinationNode { node_id } => {
                self.nodes
                    .read_output(node_id, SCREEN_LINES, TerminalOutputFormat::Text)
                    .await
                    .map_err(node_error)?
                    .text
            }
        };
        Ok(text)
    }

    async fn type_keys(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        plan: &KeyPlan,
    ) -> Result<(), ConsoleError> {
        // Refuse before attaching a controller to the pane at all.
        self.refuse_unsafe(&self.timed_resolve(target).await?.guard)
            .await?;
        let mut opened = timeout(TERMINAL_TIMEOUT, self.open(target))
            .await
            .map_err(|_| ConsoleError::Failed("opening the terminal timed out".to_owned()))??;
        let result = async {
            if !lease_matches(&opened, identity) {
                return Err(ConsoleError::IdentityChanged(
                    "the terminal was rebound".to_owned(),
                ));
            }
            // The screen may have changed between the caller's read and the
            // lease: re-read both now, with the lease held.
            let resolved = self.timed_resolve(target).await?;
            if resolved.snapshot.identity != *identity {
                return Err(ConsoleError::IdentityChanged(
                    "the terminal was rebound".to_owned(),
                ));
            }
            self.refuse_unsafe(&resolved.guard).await?;
            plan.check_start(resolved.snapshot.blocked, &self.timed_screen(target).await?)?;
            for (index, key) in plan.keys.iter().enumerate() {
                if plan.is_confirming(index) {
                    self.await_cursor(target, plan).await?;
                }
                timeout(
                    TERMINAL_TIMEOUT,
                    self.terminals.validate_lease(&opened.lease),
                )
                .await
                .map_err(|_| ConsoleError::Failed("lease check timed out".to_owned()))?
                .map_err(terminal_error)?;
                let input = TerminalClientMessage::Input {
                    text: Some((*key).to_owned()),
                    bytes: None,
                };
                timeout(TERMINAL_TIMEOUT, opened.session.send(input))
                    .await
                    .map_err(|_| ConsoleError::Failed("typing timed out".to_owned()))?
                    .map_err(|error| terminal_error(error.into()))?;
                drain(&mut opened, KEY_SPACING).await;
            }
            Ok(())
        }
        .await;
        let _ = timeout(TERMINAL_TIMEOUT, opened.session.release()).await;
        result
    }

    async fn send_prompt(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        text: &str,
        actor: &str,
    ) -> Result<String, ConsoleError> {
        let resolved = self.resolve(target).await?;
        if resolved.snapshot.identity != *identity {
            return Err(ConsoleError::IdentityChanged(
                "the terminal was rebound".to_owned(),
            ));
        }
        self.refuse_unsafe(&resolved.guard).await?;
        let command_id = uuid::Uuid::now_v7().to_string();
        let sent_id = command_id.clone();
        let (actor, text) = (actor.to_owned(), text.to_owned());
        match (target, resolved.prompt) {
            (
                AgentTarget::Assignment {
                    project_id,
                    assignment_id,
                },
                PromptRef::Assignment {
                    attempt_id,
                    assignment_version,
                    attempt_version,
                },
            ) => {
                let command = SendAssignmentPrompt {
                    command_id,
                    actor,
                    attempt_id,
                    expected_assignment_version: assignment_version,
                    expected_attempt_version: attempt_version,
                    text,
                };
                self.interventions
                    .prompt(project_id, assignment_id, command)
                    .await
                    .map(|_| sent_id)
                    .map_err(intervention_error)
            }
            (
                AgentTarget::ProjectOrchestrator { project_id },
                PromptRef::Orchestrator {
                    project_version,
                    worker_id,
                },
            ) => {
                let command = SendOrchestratorPrompt {
                    command_id,
                    actor,
                    expected_project_version: project_version,
                    orchestrator_worker_id: worker_id,
                    text,
                };
                self.interventions
                    .prompt_orchestrator(project_id, command)
                    .await
                    .map(|_| sent_id)
                    .map_err(intervention_error)
            }
            (AgentTarget::YardOrchestrator, PromptRef::Yard { version, worker_id }) => {
                let command = SendYardOrchestratorPrompt {
                    command_id,
                    actor,
                    expected_orchestrator_version: version,
                    orchestrator_worker_id: worker_id,
                    text,
                };
                self.interventions
                    .prompt_yard_orchestrator(command)
                    .await
                    .map(|_| sent_id)
                    .map_err(intervention_error)
            }
            (AgentTarget::CoordinationNode { node_id }, PromptRef::Node { version, worker_id }) => {
                let command = SendCoordinationNodePrompt {
                    command_id,
                    actor,
                    expected_node_version: version,
                    worker_id,
                    text,
                };
                self.nodes
                    .prompt(node_id, command)
                    .await
                    .map(|_| sent_id)
                    .map_err(node_error)
            }
            _ => Err(ConsoleError::Failed("target mismatch".to_owned())),
        }
    }

    async fn status_report(
        &self,
        target: &AgentTarget,
        command_id: &str,
    ) -> Result<Option<OrchestratorStatusReport>, ConsoleError> {
        let text = match target {
            AgentTarget::Assignment { .. } => return Ok(None),
            AgentTarget::ProjectOrchestrator { project_id } => {
                self.interventions
                    .read_orchestrator_output(project_id, REPORT_LINES, TerminalOutputFormat::Text)
                    .await
                    .map_err(intervention_error)?
                    .text
            }
            AgentTarget::YardOrchestrator => {
                self.interventions
                    .read_yard_orchestrator_output(REPORT_LINES, TerminalOutputFormat::Text)
                    .await
                    .map_err(intervention_error)?
                    .text
            }
            AgentTarget::CoordinationNode { node_id } => {
                self.nodes
                    .read_output(node_id, REPORT_LINES, TerminalOutputFormat::Text)
                    .await
                    .map_err(node_error)?
                    .text
            }
        };
        Ok(scan_report(&text, command_id))
    }
}

#[cfg(test)]
mod tests {
    use yard_domain::{
        AssignmentLifecycle, AttemptLifecycle, ObservedStatus, RuntimeObservationState,
        RuntimeProcessState, WorkerRuntimeBinding,
    };

    use super::{is_blocked, require_active};
    use crate::slack::actions::ConsoleError;

    fn binding() -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "s".to_owned(),
            workspace_id: "w".to_owned(),
            terminal_id: "t".to_owned(),
            tab_id: None,
            pane_id: "p".to_owned(),
            provider_session: None,
            owns_tab: false,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Blocked,
            state_change_sequence: 0,
            revision: 0,
            version: 0,
            last_observed_at_unix_ms: 0,
        }
    }

    #[test]
    fn only_an_observed_running_blocked_binding_counts_as_blocked() {
        assert!(is_blocked(&binding()));
        for state in [
            RuntimeObservationState::Missing,
            RuntimeObservationState::Ambiguous,
        ] {
            let mut stale = binding();
            stale.observation_state = state;
            assert!(!is_blocked(&stale), "{state:?}");
        }
        let mut exited = binding();
        exited.process_state = RuntimeProcessState::Exited;
        assert!(!is_blocked(&exited));
        let mut working = binding();
        working.status = ObservedStatus::Working;
        assert!(!is_blocked(&working));
    }

    #[test]
    fn inactive_assignments_or_attempts_are_gone() {
        assert!(require_active(AssignmentLifecycle::Active, AttemptLifecycle::Active).is_ok());
        for (assignment, attempt) in [
            (AssignmentLifecycle::Completed, AttemptLifecycle::Active),
            (AssignmentLifecycle::Active, AttemptLifecycle::HandedOff),
            (AssignmentLifecycle::HandingOff, AttemptLifecycle::Starting),
        ] {
            assert!(matches!(
                require_active(assignment, attempt),
                Err(ConsoleError::Gone(_))
            ));
        }
    }
}

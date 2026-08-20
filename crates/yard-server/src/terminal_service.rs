use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use yard_domain::{
    Assignment, AssignmentLifecycle, AttemptLifecycle, ProviderSessionRef, WorkerRuntimeBinding,
};

use crate::coordination_node_service::{CoordinationNodeService, CoordinationNodeServiceError};
use crate::intervention_service::{InterventionService, InterventionServiceError};

pub const MIN_TERMINAL_COLS: u16 = 20;
pub const MAX_TERMINAL_COLS: u16 = 400;
pub const MIN_TERMINAL_ROWS: u16 = 5;
pub const MAX_TERMINAL_ROWS: u16 = 200;
pub const MAX_TERMINAL_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_TERMINAL_MESSAGE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTerminalRequest {
    pub session: String,
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum TerminalServerMessage {
    #[serde(rename = "terminal.frame")]
    Frame {
        bytes: String,
        encoding: String,
        seq: u64,
        width: u16,
        height: u16,
        full: bool,
    },
    #[serde(rename = "terminal.closed")]
    Closed { reason: String },
}

impl TerminalServerMessage {
    #[must_use]
    pub fn sequence(&self) -> Option<(u64, bool)> {
        match self {
            Self::Frame { seq, full, .. } => Some((*seq, *full)),
            Self::Closed { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
pub enum TerminalClientMessage {
    #[serde(rename = "terminal.input")]
    Input {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        bytes: Option<String>,
    },
    #[serde(rename = "terminal.resize")]
    Resize { cols: u16, rows: u16 },
    #[serde(rename = "terminal.release")]
    Release,
}

impl TerminalClientMessage {
    /// Validate client-controlled terminal data before it reaches Herdr.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalServiceError::InvalidCommand`] for empty, ambiguous,
    /// oversized, or out-of-range commands.
    pub fn validate(&self) -> Result<(), TerminalServiceError> {
        match self {
            Self::Input { text, bytes } => match (text, bytes) {
                (Some(text), None)
                    if !text.is_empty() && text.len() <= MAX_TERMINAL_INPUT_BYTES =>
                {
                    Ok(())
                }
                (None, Some(bytes))
                    if !bytes.is_empty()
                        && bytes.len() <= MAX_TERMINAL_INPUT_BYTES.saturating_mul(4) / 3 + 4 =>
                {
                    Ok(())
                }
                _ => Err(TerminalServiceError::InvalidCommand(
                    "terminal.input requires exactly one non-empty bounded text or bytes field"
                        .to_owned(),
                )),
            },
            Self::Resize { cols, rows } => validate_dimensions(*cols, *rows),
            Self::Release => Ok(()),
        }
    }
}

#[async_trait]
pub trait RuntimeTerminalSession: Send {
    async fn next_message(&mut self)
    -> Result<Option<TerminalServerMessage>, RuntimeTerminalError>;

    async fn send(&mut self, command: TerminalClientMessage) -> Result<(), RuntimeTerminalError>;

    async fn release(&mut self) -> Result<(), RuntimeTerminalError>;
}

#[async_trait]
pub trait RuntimeTerminal: Send + Sync {
    async fn open_terminal(
        &self,
        request: OpenTerminalRequest,
    ) -> Result<Box<dyn RuntimeTerminalSession>, RuntimeTerminalError>;
}

pub struct OpenedTerminal {
    pub lease: TerminalLease,
    pub session: Box<dyn RuntimeTerminalSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLease {
    project_id: String,
    target: TerminalLeaseTarget,
    runtime: TerminalRuntimeIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalLeaseTarget {
    Assignment {
        assignment_id: String,
        attempt_id: String,
        worker_id: String,
    },
    Orchestrator {
        worker_id: String,
    },
    YardOrchestrator {
        worker_id: String,
    },
    CoordinationNode {
        node_id: String,
        worker_id: String,
        node_version: u64,
    },
}

impl TerminalLease {
    #[must_use]
    pub fn revoked_reason(&self) -> &'static str {
        match self.target {
            TerminalLeaseTarget::Assignment { .. } => "assignment_changed",
            TerminalLeaseTarget::Orchestrator { .. } => "orchestrator_changed",
            TerminalLeaseTarget::YardOrchestrator { .. } => "yard_orchestrator_changed",
            TerminalLeaseTarget::CoordinationNode { .. } => "coordination_node_changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalRuntimeIdentity {
    adapter: String,
    session: String,
    workspace_id: String,
    terminal_id: String,
    tab_id: Option<String>,
    pane_id: String,
    provider_session: Option<ProviderSessionRef>,
}

impl From<&WorkerRuntimeBinding> for TerminalRuntimeIdentity {
    fn from(runtime: &WorkerRuntimeBinding) -> Self {
        Self {
            adapter: runtime.adapter.clone(),
            session: runtime.session.clone(),
            workspace_id: runtime.workspace_id.clone(),
            terminal_id: runtime.terminal_id.clone(),
            tab_id: runtime.tab_id.clone(),
            pane_id: runtime.pane_id.clone(),
            provider_session: runtime.provider_session.clone(),
        }
    }
}

#[derive(Clone)]
pub struct TerminalService {
    interventions: InterventionService,
    coordination_nodes: CoordinationNodeService,
    runtime: Arc<dyn RuntimeTerminal>,
}

impl TerminalService {
    #[must_use]
    pub fn new(
        interventions: InterventionService,
        coordination_nodes: CoordinationNodeService,
        runtime: Arc<dyn RuntimeTerminal>,
    ) -> Self {
        Self {
            interventions,
            coordination_nodes,
            runtime,
        }
    }

    /// Validate and acquire one assignment terminal before a WebSocket upgrade.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalServiceError`] when dimensions are invalid, the
    /// assignment is not active, its provider identity changed, or Herdr
    /// cannot grant terminal control.
    pub async fn open(
        &self,
        project_id: &str,
        assignment_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<OpenedTerminal, TerminalServiceError> {
        validate_dimensions(cols, rows)?;
        let assignment = self
            .interventions
            .assignment(project_id, assignment_id)
            .await?;
        if assignment.lifecycle != AssignmentLifecycle::Active
            || assignment.attempt.lifecycle != AttemptLifecycle::Active
        {
            return Err(TerminalServiceError::AssignmentNotActive);
        }
        self.interventions.validate_runtime(&assignment).await?;
        let binding = assignment
            .worker
            .runtime
            .as_ref()
            .ok_or(TerminalServiceError::RuntimeBindingMissing)?;
        let session = self
            .runtime
            .open_terminal(OpenTerminalRequest {
                session: binding.session.clone(),
                terminal_id: binding.terminal_id.clone(),
                cols,
                rows,
            })
            .await?;
        let lease = assignment_terminal_lease(project_id, assignment_id, &assignment, binding);
        if let Err(error) = self.validate_lease(&lease).await {
            let mut session = session;
            let _ = session.release().await;
            return Err(error);
        }
        Ok(OpenedTerminal { lease, session })
    }

    /// Validate and acquire the current project orchestrator terminal.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalServiceError`] when dimensions are invalid, the
    /// orchestrator or provider identity changed, or Herdr cannot grant
    /// terminal control.
    pub async fn open_orchestrator(
        &self,
        project_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<OpenedTerminal, TerminalServiceError> {
        validate_dimensions(cols, rows)?;
        let project = self.interventions.project(project_id).await?;
        self.interventions
            .validate_orchestrator_binding(&project)
            .await?;
        let binding = project
            .orchestrator
            .runtime
            .as_ref()
            .ok_or(TerminalServiceError::RuntimeBindingMissing)?;
        let session = self
            .runtime
            .open_terminal(OpenTerminalRequest {
                session: binding.session.clone(),
                terminal_id: binding.terminal_id.clone(),
                cols,
                rows,
            })
            .await?;
        let lease = orchestrator_terminal_lease(project_id, &project.orchestrator.id, binding);
        if let Err(error) = self.validate_lease(&lease).await {
            let mut session = session;
            let _ = session.release().await;
            return Err(error);
        }
        Ok(OpenedTerminal { lease, session })
    }

    /// Validate and acquire the current Yard orchestrator terminal.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalServiceError`] when dimensions are invalid, the Yard
    /// orchestrator is unconfigured or changes, or Herdr cannot grant control.
    pub async fn open_yard_orchestrator(
        &self,
        cols: u16,
        rows: u16,
    ) -> Result<OpenedTerminal, TerminalServiceError> {
        validate_dimensions(cols, rows)?;
        let orchestrator = self.interventions.yard_orchestrator().await?;
        self.interventions
            .validate_yard_orchestrator_binding(&orchestrator)
            .await?;
        let worker = orchestrator
            .worker
            .as_ref()
            .ok_or(TerminalServiceError::YardOrchestratorNotConfigured)?;
        let binding = worker
            .runtime
            .as_ref()
            .ok_or(TerminalServiceError::RuntimeBindingMissing)?;
        let session = self
            .runtime
            .open_terminal(OpenTerminalRequest {
                session: binding.session.clone(),
                terminal_id: binding.terminal_id.clone(),
                cols,
                rows,
            })
            .await?;
        let lease = yard_orchestrator_terminal_lease(&worker.id, binding);
        if let Err(error) = self.validate_lease(&lease).await {
            let mut session = session;
            let _ = session.release().await;
            return Err(error);
        }
        Ok(OpenedTerminal { lease, session })
    }

    /// Validate and acquire a provisioned workstream-node terminal.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalServiceError`] when dimensions are invalid, the node
    /// is not provisioned, its binding changes, or Herdr cannot grant control.
    pub async fn open_coordination_node(
        &self,
        node_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<OpenedTerminal, TerminalServiceError> {
        validate_dimensions(cols, rows)?;
        let node = self.coordination_nodes.get(node_id).await?;
        self.coordination_nodes.validate_node_binding(&node).await?;
        let worker = node
            .worker
            .as_ref()
            .ok_or(TerminalServiceError::CoordinationNodeNotProvisioned)?;
        let binding = worker
            .runtime
            .as_ref()
            .ok_or(TerminalServiceError::RuntimeBindingMissing)?;
        let session = self
            .runtime
            .open_terminal(OpenTerminalRequest {
                session: binding.session.clone(),
                terminal_id: binding.terminal_id.clone(),
                cols,
                rows,
            })
            .await?;
        let lease = coordination_node_terminal_lease(&node.id, node.version, &worker.id, binding);
        if let Err(error) = self.validate_lease(&lease).await {
            let mut session = session;
            let _ = session.release().await;
            return Err(error);
        }
        Ok(OpenedTerminal { lease, session })
    }

    /// Confirm that an open terminal still belongs to the same active
    /// assignment attempt and durable runtime topology.
    ///
    /// # Errors
    ///
    /// Returns [`TerminalServiceError`] after completion, handoff, or runtime
    /// rebinding so a relay can revoke stale control without waiting for a
    /// Herdr frame.
    pub async fn validate_lease(&self, lease: &TerminalLease) -> Result<(), TerminalServiceError> {
        let runtime = match &lease.target {
            TerminalLeaseTarget::Assignment {
                assignment_id,
                attempt_id,
                worker_id,
            } => {
                let assignment = self
                    .interventions
                    .assignment(&lease.project_id, assignment_id)
                    .await?;
                if assignment.lifecycle != AssignmentLifecycle::Active
                    || assignment.attempt.lifecycle != AttemptLifecycle::Active
                    || assignment.attempt.id != *attempt_id
                    || assignment.worker.id != *worker_id
                {
                    return Err(TerminalServiceError::AssignmentNotActive);
                }
                assignment
                    .worker
                    .runtime
                    .ok_or(TerminalServiceError::RuntimeBindingMissing)?
            }
            TerminalLeaseTarget::Orchestrator { worker_id } => {
                let project = self.interventions.project(&lease.project_id).await?;
                if project.orchestrator.id != *worker_id {
                    return Err(TerminalServiceError::OrchestratorChanged);
                }
                project
                    .orchestrator
                    .runtime
                    .ok_or(TerminalServiceError::RuntimeBindingMissing)?
            }
            TerminalLeaseTarget::YardOrchestrator { worker_id } => {
                let orchestrator = self.interventions.yard_orchestrator().await?;
                let worker = orchestrator
                    .worker
                    .ok_or(TerminalServiceError::YardOrchestratorChanged)?;
                if worker.id != *worker_id {
                    return Err(TerminalServiceError::YardOrchestratorChanged);
                }
                worker
                    .runtime
                    .ok_or(TerminalServiceError::RuntimeBindingMissing)?
            }
            TerminalLeaseTarget::CoordinationNode {
                node_id,
                worker_id,
                node_version,
            } => {
                let node = self.coordination_nodes.get(node_id).await?;
                let worker = node
                    .worker
                    .ok_or(TerminalServiceError::CoordinationNodeChanged)?;
                if node.version != *node_version || worker.id != *worker_id {
                    return Err(TerminalServiceError::CoordinationNodeChanged);
                }
                worker
                    .runtime
                    .ok_or(TerminalServiceError::RuntimeBindingMissing)?
            }
        };
        if TerminalRuntimeIdentity::from(&runtime) != lease.runtime {
            return Err(TerminalServiceError::RuntimeBindingChanged);
        }
        Ok(())
    }
}

fn assignment_terminal_lease(
    project_id: &str,
    assignment_id: &str,
    assignment: &Assignment,
    runtime: &WorkerRuntimeBinding,
) -> TerminalLease {
    TerminalLease {
        project_id: project_id.to_owned(),
        target: TerminalLeaseTarget::Assignment {
            assignment_id: assignment_id.to_owned(),
            attempt_id: assignment.attempt.id.clone(),
            worker_id: assignment.worker.id.clone(),
        },
        runtime: TerminalRuntimeIdentity::from(runtime),
    }
}

fn orchestrator_terminal_lease(
    project_id: &str,
    worker_id: &str,
    runtime: &WorkerRuntimeBinding,
) -> TerminalLease {
    TerminalLease {
        project_id: project_id.to_owned(),
        target: TerminalLeaseTarget::Orchestrator {
            worker_id: worker_id.to_owned(),
        },
        runtime: TerminalRuntimeIdentity::from(runtime),
    }
}

fn yard_orchestrator_terminal_lease(
    worker_id: &str,
    runtime: &WorkerRuntimeBinding,
) -> TerminalLease {
    TerminalLease {
        project_id: String::new(),
        target: TerminalLeaseTarget::YardOrchestrator {
            worker_id: worker_id.to_owned(),
        },
        runtime: TerminalRuntimeIdentity::from(runtime),
    }
}

fn coordination_node_terminal_lease(
    node_id: &str,
    node_version: u64,
    worker_id: &str,
    runtime: &WorkerRuntimeBinding,
) -> TerminalLease {
    TerminalLease {
        project_id: String::new(),
        target: TerminalLeaseTarget::CoordinationNode {
            node_id: node_id.to_owned(),
            worker_id: worker_id.to_owned(),
            node_version,
        },
        runtime: TerminalRuntimeIdentity::from(runtime),
    }
}

fn validate_dimensions(cols: u16, rows: u16) -> Result<(), TerminalServiceError> {
    if !(MIN_TERMINAL_COLS..=MAX_TERMINAL_COLS).contains(&cols)
        || !(MIN_TERMINAL_ROWS..=MAX_TERMINAL_ROWS).contains(&rows)
    {
        return Err(TerminalServiceError::InvalidDimensions);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum RuntimeTerminalError {
    #[error("Herdr terminal control is already owned: {0}")]
    Ownership(String),
    #[error("Herdr terminal control is unavailable: {0}")]
    Unavailable(String),
    #[error("Herdr terminal protocol failed: {0}")]
    Protocol(String),
}

#[derive(Debug, Error)]
pub enum TerminalServiceError {
    #[error(transparent)]
    Intervention(#[from] InterventionServiceError),
    #[error(transparent)]
    CoordinationNode(#[from] CoordinationNodeServiceError),
    #[error(transparent)]
    Runtime(#[from] RuntimeTerminalError),
    #[error("terminal dimensions are outside the supported range")]
    InvalidDimensions,
    #[error("terminal command is invalid: {0}")]
    InvalidCommand(String),
    #[error("only an active assignment attempt has an interactive terminal")]
    AssignmentNotActive,
    #[error("the project's orchestrator changed while terminal control was open")]
    OrchestratorChanged,
    #[error("the Yard orchestrator has not been configured")]
    YardOrchestratorNotConfigured,
    #[error("the Yard orchestrator changed while terminal control was open")]
    YardOrchestratorChanged,
    #[error("the coordination node has not been provisioned")]
    CoordinationNodeNotProvisioned,
    #[error("the coordination node changed while terminal control was open")]
    CoordinationNodeChanged,
    #[error("assignment worker has no runtime binding")]
    RuntimeBindingMissing,
    #[error("assignment worker runtime changed while terminal control was open")]
    RuntimeBindingChanged,
}

impl TerminalServiceError {
    #[must_use]
    pub fn is_lease_revocation(&self) -> bool {
        matches!(
            self,
            Self::AssignmentNotActive
                | Self::OrchestratorChanged
                | Self::YardOrchestratorNotConfigured
                | Self::YardOrchestratorChanged
                | Self::CoordinationNodeNotProvisioned
                | Self::CoordinationNodeChanged
                | Self::RuntimeBindingMissing
                | Self::RuntimeBindingChanged
        )
    }
}

#[must_use]
pub fn sequence_continues(previous: Option<u64>, current: u64, full: bool) -> bool {
    full || previous.is_some_and(|previous| current == previous.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalClientMessage, TerminalServiceError, sequence_continues, validate_dimensions,
    };

    #[test]
    fn validates_terminal_dimensions_and_commands() {
        assert!(validate_dimensions(80, 24).is_ok());
        assert!(matches!(
            validate_dimensions(10, 24),
            Err(TerminalServiceError::InvalidDimensions)
        ));
        assert!(
            TerminalClientMessage::Input {
                text: Some("ls\r".to_owned()),
                bytes: None,
            }
            .validate()
            .is_ok()
        );
        assert!(
            TerminalClientMessage::Input {
                text: Some("ls".to_owned()),
                bytes: Some("bHM=".to_owned()),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn accepts_full_resync_and_rejects_sequence_gaps() {
        assert!(!sequence_continues(None, 10, false));
        assert!(sequence_continues(None, 10, true));
        assert!(sequence_continues(Some(10), 11, false));
        assert!(!sequence_continues(Some(10), 12, false));
        assert!(sequence_continues(Some(10), 20, true));
    }

    #[test]
    fn distinguishes_lease_revocation_from_infrastructure_failure() {
        assert!(TerminalServiceError::AssignmentNotActive.is_lease_revocation());
        assert!(TerminalServiceError::RuntimeBindingChanged.is_lease_revocation());
        assert!(!TerminalServiceError::InvalidDimensions.is_lease_revocation());
        assert!(
            !TerminalServiceError::Runtime(super::RuntimeTerminalError::Protocol(
                "socket unavailable".to_owned()
            ))
            .is_lease_revocation()
        );
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use yard_domain::{RuntimeInventory, RuntimeSessions};
use yard_herdr::{
    BootstrapAgentRequest, HerdrAdapter, HerdrControlError, HerdrError, HerdrTerminal,
    HerdrTerminalError, OpenTerminalRequest as HerdrOpenTerminalRequest, PrepareAgentRequest,
    PromptAgentRequest, ProvisionAgentRequest, ReadPaneRequest,
    RetireRuntimeRequest as HerdrRetireRuntimeRequest, StartPreparedAgentRequest,
    TerminalCommand as HerdrTerminalCommand, TerminalDimensions, TerminalEncoding, TerminalEvent,
    TerminalInput,
};

use crate::allocation_service::{
    RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeRetirementError,
    RuntimeRetirementRequest, RuntimeSessionRequest, RuntimeWorkspaceProvisionRequest,
};
use crate::intervention_service::{
    RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest, RuntimeOutputResult,
    RuntimePromptRequest, RuntimePromptResult,
};
use crate::provider_agents::ProviderAgentObserver;
use crate::terminal_service::{
    OpenTerminalRequest, RuntimeTerminal, RuntimeTerminalError, RuntimeTerminalSession,
    TerminalClientMessage, TerminalServerMessage,
};

#[derive(Debug, Error)]
pub enum InventoryServiceError {
    #[error(transparent)]
    Herdr(#[from] HerdrError),
}

#[async_trait]
pub trait InventorySource: Send + Sync {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError>;
    async fn inventory(
        &self,
        session_name: &str,
    ) -> Result<RuntimeInventory, InventoryServiceError>;
}

#[derive(Debug)]
pub struct HerdrInventorySource {
    adapter: HerdrAdapter,
    provider_agents: Arc<ProviderAgentObserver>,
}

struct HerdrRuntimeTerminalSession {
    terminal: Option<HerdrTerminal>,
    first_message: Option<TerminalServerMessage>,
}

impl HerdrInventorySource {
    #[must_use]
    pub fn new(adapter: HerdrAdapter) -> Self {
        Self {
            adapter,
            provider_agents: Arc::new(ProviderAgentObserver::from_env()),
        }
    }
}

#[async_trait]
impl InventorySource for HerdrInventorySource {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
        self.adapter.sessions().await.map_err(Into::into)
    }

    async fn inventory(
        &self,
        session_name: &str,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        let mut inventory = self
            .adapter
            .inventory(session_name)
            .await
            .map_err(InventoryServiceError::from)?;
        let workers = inventory.workers.clone();
        let observer = Arc::clone(&self.provider_agents);
        match tokio::task::spawn_blocking(move || observer.observe(&workers)).await {
            Ok(children) => inventory.child_agents = children,
            Err(error) => {
                tracing::warn!(%error, "provider child-agent observation task failed");
            }
        }
        Ok(inventory)
    }
}

#[async_trait]
impl RuntimeControl for HerdrInventorySource {
    async fn ensure_session(
        &self,
        request: RuntimeSessionRequest,
    ) -> Result<(), RuntimeProvisionError> {
        self.adapter
            .ensure_session(&request.session, std::path::Path::new(&request.startup_cwd))
            .await
            .map_err(|error| RuntimeProvisionError::BeforeWorker(error.to_string()))
    }

    async fn bootstrap_worker(
        &self,
        request: RuntimeWorkspaceProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        let result = self
            .adapter
            .bootstrap_agent(BootstrapAgentRequest {
                command_id: request.command_id,
                session: request.session,
                workspace_label: request.workspace_label,
                cwd: request.cwd,
                agent_name: request.agent_name,
                kind: request.kind,
                args: request.args,
                prompt: request.prompt,
            })
            .await;
        match result {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed { runtime, source }) => {
                Err(RuntimeProvisionError::PromptDelivery {
                    runtime,
                    message: source.to_string(),
                })
            }
            Err(HerdrControlError::WorkspaceCreateFailed { source, ambiguous }) if ambiguous => {
                Err(RuntimeProvisionError::AfterPreparation {
                    message: source.to_string(),
                    ambiguous: true,
                })
            }
            Err(HerdrControlError::WorkspaceCreateFailed { source, .. }) => {
                Err(RuntimeProvisionError::BeforeWorker(source.to_string()))
            }
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
            }) => Err(RuntimeProvisionError::AfterPreparation {
                message: HerdrControlError::StartFailed {
                    start,
                    rollback,
                    rollback_succeeded,
                }
                .to_string(),
                ambiguous: !rollback_succeeded,
            }),
            Err(HerdrControlError::Runtime(error)) => {
                Err(RuntimeProvisionError::BeforeWorker(error.to_string()))
            }
        }
    }

    async fn provision_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        let request = ProvisionAgentRequest {
            command_id: request.command_id,
            session: request.session,
            workspace_id: request.workspace_id,
            cwd: request.cwd,
            tab_label: request.tab_label,
            agent_name: request.agent_name,
            kind: request.kind,
            args: request.args,
            prompt: request.prompt,
        };
        match self.adapter.provision_agent(request).await {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed { runtime, source }) => {
                Err(RuntimeProvisionError::PromptDelivery {
                    runtime,
                    message: source.to_string(),
                })
            }
            Err(error) => Err(RuntimeProvisionError::BeforeWorker(error.to_string())),
        }
    }

    async fn prepare_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        self.adapter
            .prepare_agent(PrepareAgentRequest {
                command_id: request.command_id,
                session: request.session,
                workspace_id: request.workspace_id,
                cwd: request.cwd,
                tab_label: request.tab_label,
            })
            .await
            .map(|prepared| prepared.runtime)
            .map_err(|error| RuntimeProvisionError::BeforeWorker(error.to_string()))
    }

    async fn start_prepared_worker(
        &self,
        request: RuntimeProvisionRequest,
        prepared: yard_domain::WorkerRuntimeBinding,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .start_prepared_agent(StartPreparedAgentRequest {
                command_id: request.command_id,
                prepared,
                agent_name: request.agent_name,
                kind: request.kind,
                args: request.args,
                prompt: request.prompt,
            })
            .await
        {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed { runtime, source }) => {
                Err(RuntimeProvisionError::PromptDelivery {
                    runtime,
                    message: source.to_string(),
                })
            }
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
            }) => Err(RuntimeProvisionError::AfterPreparation {
                message: HerdrControlError::StartFailed {
                    start,
                    rollback,
                    rollback_succeeded,
                }
                .to_string(),
                ambiguous: !rollback_succeeded,
            }),
            Err(error) => Err(RuntimeProvisionError::AfterPreparation {
                message: error.to_string(),
                ambiguous: true,
            }),
        }
    }

    async fn retire_runtime(
        &self,
        request: RuntimeRetirementRequest,
    ) -> Result<(), RuntimeRetirementError> {
        if request.adapter != "herdr" {
            return Err(RuntimeRetirementError::UnsupportedAdapter(request.adapter));
        }
        let inventory = self
            .adapter
            .inventory(&request.session)
            .await
            .map_err(|error| RuntimeRetirementError::Runtime(error.to_string()))?;
        let Some(target) = retirement_target(&inventory, &request) else {
            return Ok(());
        };
        self.adapter
            .retire_runtime(HerdrRetireRuntimeRequest {
                cleanup_id: request.cleanup_id,
                session: request.session,
                tab_id: Some(target.tab_id),
                pane_id: target.pane_id,
                owns_tab: target.owns_tab,
            })
            .await
            .map_err(|error| RuntimeRetirementError::Runtime(error.to_string()))
    }
}

struct RetirementTarget {
    tab_id: String,
    pane_id: String,
    owns_tab: bool,
}

fn retirement_target(
    inventory: &RuntimeInventory,
    request: &RuntimeRetirementRequest,
) -> Option<RetirementTarget> {
    let worker = inventory
        .workers
        .iter()
        .find(|worker| worker.terminal_id == request.terminal_id);
    let (workspace_id, tab_id, pane_id) = if let Some(worker) = worker {
        if let Some(expected) = request.provider_session.as_ref() {
            if worker.provider_session.as_ref() != Some(expected) {
                return None;
            }
        } else if worker.provider_session.is_some()
            || request.tab_id.as_deref() != Some(worker.tab_id.as_str())
            || request.pane_id != worker.pane_id
        {
            return None;
        }
        (
            worker.workspace_id.as_str(),
            worker.tab_id.as_str(),
            worker.pane_id.as_str(),
        )
    } else {
        if request.provider_session.is_some() {
            return None;
        }
        let pane = inventory
            .panes
            .iter()
            .find(|pane| pane.terminal_id == request.terminal_id)?;
        if request.tab_id.as_deref() != Some(pane.tab_id.as_str())
            || request.pane_id != pane.runtime_id
        {
            return None;
        }
        (
            pane.workspace_id.as_str(),
            pane.tab_id.as_str(),
            pane.runtime_id.as_str(),
        )
    };
    if workspace_id != request.workspace_id {
        return None;
    }
    let owns_tab = request.owns_tab
        && inventory.tabs.iter().any(|tab| {
            tab.runtime_id == tab_id && tab.workspace_id == workspace_id && tab.pane_count == 1
        });
    Some(RetirementTarget {
        tab_id: tab_id.to_owned(),
        pane_id: pane_id.to_owned(),
        owns_tab,
    })
}

#[async_trait]
impl RuntimeIntervention for HerdrInventorySource {
    async fn prompt(
        &self,
        request: RuntimePromptRequest,
    ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
        self.adapter
            .prompt_agent(PromptAgentRequest {
                command_id: request.command_id,
                session: request.session,
                pane_id: request.pane_id,
                text: request.text,
            })
            .await
            .map(|result| RuntimePromptResult {
                status: result.status,
            })
            .map_err(|error| classify_prompt_error(&error))
    }

    async fn read_output(
        &self,
        request: RuntimeOutputRequest,
    ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
        self.adapter
            .read_pane(ReadPaneRequest {
                request_id: request.request_id,
                session: request.session,
                pane_id: request.pane_id,
                lines: request.lines,
            })
            .await
            .map(|output| RuntimeOutputResult {
                pane_id: output.pane_id,
                workspace_id: output.workspace_id,
                tab_id: output.tab_id,
                source: output.source,
                format: output.format,
                text: output.text,
                revision: output.revision,
                truncated: output.truncated,
            })
            .map_err(|error| RuntimeInterventionError::Unavailable(error.to_string()))
    }
}

#[async_trait]
impl RuntimeTerminal for HerdrInventorySource {
    async fn open_terminal(
        &self,
        request: OpenTerminalRequest,
    ) -> Result<Box<dyn RuntimeTerminalSession>, RuntimeTerminalError> {
        let dimensions = TerminalDimensions::new(request.cols, request.rows)
            .map_err(|error| classify_terminal_error(&error))?;
        let request =
            HerdrOpenTerminalRequest::new(request.session, request.terminal_id, dimensions)
                .map_err(|error| classify_terminal_error(&error))?;
        let mut terminal = self
            .adapter
            .open_terminal(request)
            .map_err(|error| classify_terminal_error(&error))?;
        let first_event = terminal
            .next_event()
            .await
            .map_err(|error| classify_terminal_error(&error))?;
        let Some(first_event) = first_event else {
            return Err(RuntimeTerminalError::Unavailable(
                "Herdr terminal control closed before its initial frame".to_owned(),
            ));
        };
        if let TerminalEvent::Closed(closed) = &first_event {
            let reason = closed.reason.clone();
            let _ = terminal.close().await;
            return if is_ownership_failure(&reason) {
                Err(RuntimeTerminalError::Ownership(reason))
            } else {
                Err(RuntimeTerminalError::Unavailable(reason))
            };
        }
        let first_message = terminal_server_message(first_event);
        if !matches!(
            first_message,
            TerminalServerMessage::Frame { full: true, .. }
        ) {
            let _ = terminal.close().await;
            return Err(RuntimeTerminalError::Protocol(
                "Herdr terminal stream did not begin with a full frame".to_owned(),
            ));
        }
        Ok(Box::new(HerdrRuntimeTerminalSession {
            terminal: Some(terminal),
            first_message: Some(first_message),
        }))
    }
}

#[async_trait]
impl RuntimeTerminalSession for HerdrRuntimeTerminalSession {
    async fn next_message(
        &mut self,
    ) -> Result<Option<TerminalServerMessage>, RuntimeTerminalError> {
        if let Some(message) = self.first_message.take() {
            return Ok(Some(message));
        }
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(None);
        };
        match terminal.next_event().await {
            Ok(Some(event)) => Ok(Some(terminal_server_message(event))),
            Ok(None) => {
                self.terminal.take();
                Ok(None)
            }
            Err(error) => {
                self.terminal.take();
                Err(classify_terminal_error(&error))
            }
        }
    }

    async fn send(&mut self, command: TerminalClientMessage) -> Result<(), RuntimeTerminalError> {
        let terminal = self.terminal.as_mut().ok_or_else(|| {
            RuntimeTerminalError::Unavailable("Herdr terminal control is closed".to_owned())
        })?;
        let command = match command {
            TerminalClientMessage::Input {
                text: Some(text),
                bytes: None,
            } => HerdrTerminalCommand::Input(TerminalInput::Text(text)),
            TerminalClientMessage::Input {
                text: None,
                bytes: Some(bytes),
            } => HerdrTerminalCommand::Input(TerminalInput::BytesBase64(bytes)),
            TerminalClientMessage::Resize { cols, rows } => HerdrTerminalCommand::Resize(
                TerminalDimensions::new(cols, rows)
                    .map_err(|error| classify_terminal_error(&error))?,
            ),
            TerminalClientMessage::Release => HerdrTerminalCommand::Release,
            TerminalClientMessage::Input { .. } => {
                return Err(RuntimeTerminalError::Protocol(
                    "terminal.input requires exactly one text or bytes field".to_owned(),
                ));
            }
        };
        terminal
            .send(command)
            .await
            .map_err(|error| classify_terminal_error(&error))
    }

    async fn release(&mut self) -> Result<(), RuntimeTerminalError> {
        let Some(terminal) = self.terminal.take() else {
            return Ok(());
        };
        terminal
            .close()
            .await
            .map_err(|error| classify_terminal_error(&error))
    }
}

fn terminal_server_message(event: TerminalEvent) -> TerminalServerMessage {
    match event {
        TerminalEvent::Frame(frame) => TerminalServerMessage::Frame {
            bytes: frame.bytes,
            encoding: match frame.encoding {
                TerminalEncoding::Ansi => "ansi".to_owned(),
            },
            seq: frame.seq,
            width: frame.width,
            height: frame.height,
            full: frame.full,
        },
        TerminalEvent::Closed(closed) => TerminalServerMessage::Closed {
            reason: closed.reason,
        },
    }
}

fn classify_terminal_error(error: &HerdrTerminalError) -> RuntimeTerminalError {
    let message = error.to_string();
    if let HerdrTerminalError::UnexpectedExit { stderr, .. } = error
        && is_ownership_failure(stderr)
    {
        return RuntimeTerminalError::Ownership(message);
    }
    match error {
        HerdrTerminalError::EventLineTooLarge
        | HerdrTerminalError::UnterminatedEvent
        | HerdrTerminalError::MalformedEvent(_)
        | HerdrTerminalError::InvalidEvent(_)
        | HerdrTerminalError::InvalidBase64Input
        | HerdrTerminalError::CommandEncode(_)
        | HerdrTerminalError::CommandLineTooLarge => RuntimeTerminalError::Protocol(message),
        _ => RuntimeTerminalError::Unavailable(message),
    }
}

fn is_ownership_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("ownership")
        || message.contains("already controlled")
        || message.contains("controller already")
        || message.contains("control conflict")
}

fn classify_prompt_error(error: &HerdrError) -> RuntimeInterventionError {
    match error {
        HerdrError::Api { code, .. }
            if matches!(
                code.as_str(),
                "agent_not_found"
                    | "agent_not_ready"
                    | "empty_agent_prompt"
                    | "agent_prompt_failed"
            ) =>
        {
            RuntimeInterventionError::Rejected(error.to_string())
        }
        HerdrError::DiscoveryIo(_)
        | HerdrError::DiscoveryTimeout
        | HerdrError::DiscoveryResponseTooLarge { .. }
        | HerdrError::DiscoveryFailed { .. }
        | HerdrError::DiscoveryDecode(_)
        | HerdrError::SessionNotFound(_)
        | HerdrError::SessionNotRunning(_)
        | HerdrError::SocketConnect { .. } => {
            RuntimeInterventionError::Unavailable(error.to_string())
        }
        _ => RuntimeInterventionError::Ambiguous(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yard_domain::{
        FocusObservation, ObservedStatus, ObservedWorker, ProviderSessionRef, RuntimeInventory,
        TabObservation,
    };

    use super::retirement_target;
    use crate::allocation_service::RuntimeRetirementRequest;

    fn provider(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn inventory(provider_session: ProviderSessionRef, pane_count: usize) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.8.0".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: vec![TabObservation {
                runtime_id: "tab-current".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                order: 1,
                label: "Worker".to_owned(),
                focused: false,
                pane_count,
                status: ObservedStatus::Idle,
            }],
            panes: Vec::new(),
            workers: vec![ObservedWorker {
                runtime_id: "terminal-1".to_owned(),
                terminal_id: "terminal-1".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                tab_id: "tab-current".to_owned(),
                pane_id: "pane-current".to_owned(),
                name: Some("worker".to_owned()),
                provider: Some("codex".to_owned()),
                display_provider: Some("Codex".to_owned()),
                status: ObservedStatus::Idle,
                focused: false,
                launch_pending: false,
                interactive_ready: true,
                state_change_sequence: 1,
                cwd: Some("/tmp/project".to_owned()),
                foreground_cwd: Some("/tmp/project".to_owned()),
                tokens: BTreeMap::new(),
                provider_session: Some(provider_session),
                revision: 1,
            }],
            child_agents: Vec::new(),
        }
    }

    fn request(provider_session: ProviderSessionRef) -> RuntimeRetirementRequest {
        RuntimeRetirementRequest {
            cleanup_id: "cleanup-1".to_owned(),
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            tab_id: Some("tab-stale".to_owned()),
            pane_id: "pane-stale".to_owned(),
            provider_session: Some(provider_session),
            owns_tab: true,
        }
    }

    #[test]
    fn retirement_follows_the_matching_runtime_identity() {
        let target = retirement_target(
            &inventory(provider("session-1"), 1),
            &request(provider("session-1")),
        )
        .unwrap();

        assert_eq!(target.tab_id, "tab-current");
        assert_eq!(target.pane_id, "pane-current");
        assert!(target.owns_tab);
    }

    #[test]
    fn retirement_converges_without_closing_a_reused_terminal_identity() {
        let target = retirement_target(
            &inventory(provider("replacement-session"), 1),
            &request(provider("original-session")),
        );

        assert!(target.is_none());
    }

    #[test]
    fn retirement_closes_only_the_pane_when_the_tab_is_shared() {
        let target = retirement_target(
            &inventory(provider("session-1"), 2),
            &request(provider("session-1")),
        )
        .unwrap();

        assert!(!target.owns_tab);
        assert_eq!(target.pane_id, "pane-current");
    }

    #[test]
    fn retirement_requires_exact_topology_without_provider_identity() {
        let mut inventory = inventory(provider("session-1"), 1);
        inventory.workers[0].provider_session = None;
        let mut request = request(provider("session-1"));
        request.provider_session = None;

        assert!(retirement_target(&inventory, &request).is_none());

        request.tab_id = Some("tab-current".to_owned());
        request.pane_id = "pane-current".to_owned();
        assert!(retirement_target(&inventory, &request).is_some());
    }

    #[test]
    fn retirement_does_not_upgrade_an_unknown_captured_identity() {
        let mut request = request(provider("session-1"));
        request.provider_session = None;
        request.tab_id = Some("tab-current".to_owned());
        request.pane_id = "pane-current".to_owned();

        assert!(
            retirement_target(&inventory(provider("replacement-session"), 1), &request).is_none()
        );
    }
}

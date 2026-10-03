use axum::{
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, Uri, header},
    response::Response,
};
use serde::Deserialize;
use std::{
    future::{Future, pending},
    time::Duration,
};
use tokio::{
    sync::watch,
    time::{Instant, interval_at, timeout},
};

use super::{ApiError, AppState};
use crate::{
    ConnectionGuard,
    coordination_node_service::CoordinationNodeServiceError,
    intervention_service::InterventionServiceError,
    slack::presence::{PresenceGuard, ViewTarget},
    terminal_service::{
        MAX_TERMINAL_MESSAGE_BYTES, RuntimeTerminalError, TerminalClientMessage,
        TerminalServerMessage, TerminalServiceError, sequence_continues,
    },
};

const TERMINAL_OPERATION_TIMEOUT: Duration = Duration::from_secs(6);
const TERMINAL_SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const TERMINAL_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const TERMINAL_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the relay re-checks its lease while nothing else does.
const TERMINAL_LEASE_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// Scroll commands and outgoing frames reuse a lease confirmation younger
/// than this instead of querying the store for each one. The lease tick runs
/// at the same period, so a revoked lease still closes the terminal within
/// about one tick; input and every other command are always checked.
const TERMINAL_LEASE_REUSE_WINDOW: Duration = Duration::from_millis(250);

/// When the relay last confirmed that its lease is still valid.
#[derive(Debug, Default)]
struct LeaseConfirmation {
    confirmed_at: Option<Instant>,
}

impl LeaseConfirmation {
    fn is_fresh(&self, now: Instant) -> bool {
        self.confirmed_at
            .is_some_and(|at| now.saturating_duration_since(at) < TERMINAL_LEASE_REUSE_WINDOW)
    }
}

/// Whether a relay step may reuse a recent lease confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseCheck {
    /// Scroll commands and outgoing frames: reuse a fresh confirmation.
    Reusable,
    /// The lease tick, input, and every other command: always query.
    Required,
}

/// The lease check a browser command needs: only `terminal.scroll` may reuse
/// a recent confirmation. Input, resize, `scroll_reset`, and everything else
/// always query the store, so a keystroke never reaches a revoked terminal.
fn lease_check_for(command: &TerminalClientMessage) -> LeaseCheck {
    if matches!(command, TerminalClientMessage::Scroll { .. }) {
        LeaseCheck::Reusable
    } else {
        LeaseCheck::Required
    }
}

/// Confirm the lease at `started`, reusing a confirmation from the last
/// 250 ms when `check` allows it. A confirmation is dated from when its query
/// started, and a reused one is never extended, so no step relies on a lease
/// check older than the reuse window.
async fn confirm_lease<F, Validation>(
    confirmation: &mut LeaseConfirmation,
    check: LeaseCheck,
    started: Instant,
    validate: F,
) -> Result<(), &'static str>
where
    F: FnOnce() -> Validation,
    Validation: Future<Output = Result<(), &'static str>>,
{
    if check == LeaseCheck::Reusable && confirmation.is_fresh(started) {
        return Ok(());
    }
    match validate().await {
        Ok(()) => {
            confirmation.confirmed_at = Some(started);
            Ok(())
        }
        Err(reason) => {
            confirmation.confirmed_at = None;
            Err(reason)
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct TerminalQuery {
    cols: u16,
    rows: u16,
}

pub(super) async fn assignment_terminal(
    State(state): State<AppState>,
    Path((project_id, assignment_id)): Path<(String, String)>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let connection = state.connections.track().ok_or_else(server_shutting_down)?;
    let terminal = state
        .terminals
        .open(&project_id, &assignment_id, query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    let shutdown = state.shutdown.clone();
    let viewing = state.presence.open(ViewTarget::Assignment(assignment_id));
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            relay(socket, terminal, terminals, shutdown, connection, viewing)
        }))
}

pub(super) async fn orchestrator_terminal(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let connection = state.connections.track().ok_or_else(server_shutting_down)?;
    let terminal = state
        .terminals
        .open_orchestrator(&project_id, query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    let shutdown = state.shutdown.clone();
    let viewing = state
        .presence
        .open(ViewTarget::ProjectOrchestrator(project_id));
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            relay(socket, terminal, terminals, shutdown, connection, viewing)
        }))
}

pub(super) async fn yard_orchestrator_terminal(
    State(state): State<AppState>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let connection = state.connections.track().ok_or_else(server_shutting_down)?;
    let terminal = state
        .terminals
        .open_yard_orchestrator(query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    let shutdown = state.shutdown.clone();
    let viewing = state.presence.open(ViewTarget::YardOrchestrator);
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            relay(socket, terminal, terminals, shutdown, connection, viewing)
        }))
}

pub(super) async fn coordination_node_terminal(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let connection = state.connections.track().ok_or_else(server_shutting_down)?;
    let terminal = state
        .terminals
        .open_coordination_node(&node_id, query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    let shutdown = state.shutdown.clone();
    let viewing = state.presence.open(ViewTarget::CoordinationNode(node_id));
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            relay(socket, terminal, terminals, shutdown, connection, viewing)
        }))
}

#[allow(clippy::too_many_lines)]
async fn relay(
    mut socket: WebSocket,
    terminal: crate::terminal_service::OpenedTerminal,
    terminals: crate::terminal_service::TerminalService,
    mut shutdown: Option<watch::Receiver<bool>>,
    _connection: ConnectionGuard,
    _viewing: PresenceGuard,
) {
    let crate::terminal_service::OpenedTerminal { lease, mut session } = terminal;
    let mut last_sequence = None;
    // `terminal.scroll` messages written to Herdr so far. Each frame carries
    // the count at the time it was read, which tells the browser whether the
    // frame can already show its latest scroll batch.
    let mut scrolls_forwarded: u64 = 0;
    let mut lease_confirmation = LeaseConfirmation::default();
    // The first tick is one interval after open (the open handler has just
    // validated the lease), so the first scroll or frame after open always
    // runs its own lease query instead of reusing an immediate tick's.
    let mut lease_check = interval_at(
        Instant::now() + TERMINAL_LEASE_CHECK_INTERVAL,
        TERMINAL_LEASE_CHECK_INTERVAL,
    );
    lease_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = interval_at(
        Instant::now() + TERMINAL_HEARTBEAT_INTERVAL,
        TERMINAL_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_client_activity = Instant::now();
    loop {
        tokio::select! {
            () = wait_for_shutdown(&mut shutdown) => {
                send_closed(&mut socket, "server_shutdown").await;
                break;
            }
            _ = heartbeat.tick() => {
                if last_client_activity.elapsed() >= TERMINAL_HEARTBEAT_TIMEOUT {
                    tracing::warn!("interactive terminal client heartbeat timed out");
                    break;
                }
                if send_ping(&mut socket).await.is_err() {
                    break;
                }
            }
            _ = lease_check.tick() => {
                if let Err(reason) = confirm_lease(
                    &mut lease_confirmation,
                    LeaseCheck::Required,
                    Instant::now(),
                    || validate_lease(&terminals, &lease),
                )
                .await
                {
                    send_closed(&mut socket, reason).await;
                    break;
                }
            }
            // Cancel-safe: a partially read Herdr frame is kept by the
            // session when a timer or client branch wins this select.
            runtime_message = session.next_message() => {
                match runtime_message {
                    Ok(Some(message)) => {
                        if let Err(reason) = confirm_lease(
                            &mut lease_confirmation,
                            LeaseCheck::Reusable,
                            Instant::now(),
                            || validate_lease(&terminals, &lease),
                        )
                        .await
                        {
                            send_closed(&mut socket, reason).await;
                            break;
                        }
                        if let Some((sequence, full)) = message.sequence() {
                            if !sequence_continues(last_sequence, sequence, full) {
                                send_closed(&mut socket, "sequence_gap").await;
                                break;
                            }
                            last_sequence = Some(sequence);
                        }
                        let closed = matches!(message, TerminalServerMessage::Closed { .. });
                        let message = message.with_scrolls(scrolls_forwarded);
                        if send_message(&mut socket, &message).await.is_err() || closed {
                            break;
                        }
                    }
                    Ok(None) => {
                        send_closed(&mut socket, "runtime_eof").await;
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "interactive terminal runtime failed");
                        send_closed(&mut socket, "runtime_error").await;
                        break;
                    }
                }
            }
            client_message = socket.recv() => {
                let Some(client_message) = client_message else {
                    break;
                };
                let Ok(client_message) = client_message else {
                    break;
                };
                last_client_activity = Instant::now();
                match client_message {
                    Message::Text(text) => {
                        if text.len() > MAX_TERMINAL_MESSAGE_BYTES {
                            send_closed(&mut socket, "message_too_large").await;
                            break;
                        }
                        let command = serde_json::from_str::<TerminalClientMessage>(&text);
                        let Ok(command) = command else {
                            send_closed(&mut socket, "invalid_command").await;
                            break;
                        };
                        if command.validate().is_err() {
                            send_closed(&mut socket, "invalid_command").await;
                            break;
                        }
                        if matches!(command, TerminalClientMessage::Release) {
                            match timeout(TERMINAL_OPERATION_TIMEOUT, session.release()).await {
                                Ok(Ok(())) => send_closed(&mut socket, "released").await,
                                Ok(Err(error)) => {
                                    tracing::warn!(%error, "interactive terminal release failed");
                                    send_closed(&mut socket, "runtime_error").await;
                                }
                                Err(_) => {
                                    tracing::warn!("interactive terminal release timed out");
                                    send_closed(&mut socket, "runtime_timeout").await;
                                }
                            }
                            break;
                        }
                        let scroll = matches!(command, TerminalClientMessage::Scroll { .. });
                        if let Err(reason) = confirm_lease(
                            &mut lease_confirmation,
                            lease_check_for(&command),
                            Instant::now(),
                            || validate_lease(&terminals, &lease),
                        )
                        .await
                        {
                            send_closed(&mut socket, reason).await;
                            break;
                        }
                        match timeout(TERMINAL_OPERATION_TIMEOUT, session.send(command)).await {
                            Ok(Ok(())) => {
                                if scroll {
                                    scrolls_forwarded += 1;
                                }
                            }
                            Ok(Err(error)) => {
                                tracing::warn!(%error, "interactive terminal command failed");
                                send_closed(&mut socket, "runtime_error").await;
                                break;
                            }
                            Err(_) => {
                                tracing::warn!("interactive terminal command timed out");
                                send_closed(&mut socket, "runtime_timeout").await;
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if send_pong(&mut socket, payload).await.is_err() {
                            break;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Binary(_) => {
                        send_closed(&mut socket, "invalid_command").await;
                        break;
                    }
                }
            }
        }
    }
    let _ = timeout(TERMINAL_OPERATION_TIMEOUT, session.release()).await;
    let _ = timeout(
        TERMINAL_SOCKET_WRITE_TIMEOUT,
        socket.send(Message::Close(None)),
    )
    .await;
}

async fn validate_lease(
    terminals: &crate::terminal_service::TerminalService,
    lease: &crate::terminal_service::TerminalLease,
) -> Result<(), &'static str> {
    match timeout(TERMINAL_OPERATION_TIMEOUT, terminals.validate_lease(lease)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            if error.is_lease_revocation() {
                tracing::debug!(%error, "interactive terminal lease was revoked");
                Err(lease.revoked_reason())
            } else {
                tracing::warn!(%error, "interactive terminal lease validation failed");
                Err("runtime_error")
            }
        }
        Err(_) => {
            tracing::warn!("interactive terminal lease validation timed out");
            Err("runtime_timeout")
        }
    }
}

async fn send_message(socket: &mut WebSocket, message: &TerminalServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    timeout(
        TERMINAL_SOCKET_WRITE_TIMEOUT,
        socket.send(Message::Text(text.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

async fn send_ping(socket: &mut WebSocket) -> Result<(), ()> {
    send_socket_message(socket, Message::Ping(Vec::new().into())).await
}

async fn send_pong(socket: &mut WebSocket, payload: axum::body::Bytes) -> Result<(), ()> {
    send_socket_message(socket, Message::Pong(payload)).await
}

async fn send_socket_message(socket: &mut WebSocket, message: Message) -> Result<(), ()> {
    timeout(TERMINAL_SOCKET_WRITE_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn send_closed(socket: &mut WebSocket, reason: &str) {
    let _ = send_message(
        socket,
        &TerminalServerMessage::Closed {
            reason: reason.to_owned(),
        },
    )
    .await;
}

async fn wait_for_shutdown(shutdown: &mut Option<watch::Receiver<bool>>) {
    let Some(shutdown) = shutdown else {
        pending::<()>().await;
        return;
    };
    if *shutdown.borrow() {
        return;
    }
    loop {
        if shutdown.changed().await.is_err() {
            pending::<()>().await;
        }
        if *shutdown.borrow() {
            return;
        }
    }
}

fn validate_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(origin_forbidden)?;
    if !is_loopback_origin(origin) {
        return Err(origin_forbidden());
    }
    Ok(())
}

/// Whether `origin` is an `http(s)` origin on a loopback host name.
pub(super) fn is_loopback_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    let scheme_allowed = matches!(uri.scheme_str(), Some("http" | "https"));
    let host_allowed = uri
        .authority()
        .is_some_and(|authority| is_loopback_host_name(authority.host()));
    let path_allowed = uri.path_and_query().is_none_or(|path| path.as_str() == "/");
    scheme_allowed && host_allowed && path_allowed
}

pub(super) fn is_loopback_host_name(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

fn origin_forbidden() -> ApiError {
    ApiError {
        status: StatusCode::FORBIDDEN,
        code: "terminal_origin_forbidden",
        message: "Interactive terminal connections require a loopback web origin".to_owned(),
    }
}

fn server_shutting_down() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "server_shutting_down",
        message: "Yard is shutting down".to_owned(),
    }
}

#[allow(clippy::too_many_lines)]
fn terminal_error(error: &TerminalServiceError) -> ApiError {
    match error {
        TerminalServiceError::InvalidDimensions => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_terminal_dimensions",
            message: error.to_string(),
        },
        TerminalServiceError::Runtime(RuntimeTerminalError::Ownership(_)) => ApiError {
            status: StatusCode::CONFLICT,
            code: "terminal_control_unavailable",
            message: error.to_string(),
        },
        TerminalServiceError::Intervention(InterventionServiceError::AssignmentNotFound) => {
            ApiError {
                status: StatusCode::NOT_FOUND,
                code: "assignment_not_found",
                message: error.to_string(),
            }
        }
        TerminalServiceError::Intervention(InterventionServiceError::Store(
            yard_store::ProjectStoreError::ProjectNotFound,
        )) => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "project_not_found",
            message: error.to_string(),
        },
        TerminalServiceError::CoordinationNode(CoordinationNodeServiceError::Store(
            yard_store::ProjectStoreError::CoordinationNodeArchived,
        )) => ApiError {
            status: StatusCode::CONFLICT,
            code: "coordination_node_archived",
            message: error.to_string(),
        },
        TerminalServiceError::CoordinationNode(CoordinationNodeServiceError::Store(
            yard_store::ProjectStoreError::CoordinationNodeNotFound,
        )) => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "coordination_node_not_found",
            message: error.to_string(),
        },
        TerminalServiceError::RuntimeBindingMissing
        | TerminalServiceError::RuntimeBindingChanged
        | TerminalServiceError::Intervention(
            InterventionServiceError::RuntimeBindingMissing
            | InterventionServiceError::RuntimeBindingStale,
        ) => ApiError {
            status: StatusCode::CONFLICT,
            code: "runtime_binding_stale",
            message: error.to_string(),
        },
        TerminalServiceError::OrchestratorChanged
        | TerminalServiceError::Intervention(InterventionServiceError::OrchestratorChanged) => {
            ApiError {
                status: StatusCode::CONFLICT,
                code: "orchestrator_changed",
                message: error.to_string(),
            }
        }
        TerminalServiceError::YardOrchestratorChanged
        | TerminalServiceError::Intervention(InterventionServiceError::YardOrchestratorChanged) => {
            ApiError {
                status: StatusCode::CONFLICT,
                code: "yard_orchestrator_changed",
                message: error.to_string(),
            }
        }
        TerminalServiceError::YardOrchestratorNotConfigured
        | TerminalServiceError::Intervention(InterventionServiceError::Store(
            yard_store::ProjectStoreError::YardOrchestratorNotConfigured,
        )) => ApiError {
            status: StatusCode::CONFLICT,
            code: "yard_orchestrator_not_configured",
            message: error.to_string(),
        },
        TerminalServiceError::CoordinationNodeNotProvisioned
        | TerminalServiceError::CoordinationNode(
            CoordinationNodeServiceError::NodeNotProvisioned,
        ) => ApiError {
            status: StatusCode::CONFLICT,
            code: "coordination_node_not_provisioned",
            message: error.to_string(),
        },
        TerminalServiceError::CoordinationNodeChanged
        | TerminalServiceError::CoordinationNode(CoordinationNodeServiceError::NodeChanged) => {
            ApiError {
                status: StatusCode::CONFLICT,
                code: "coordination_node_changed",
                message: error.to_string(),
            }
        }
        TerminalServiceError::AssignmentNotActive
        | TerminalServiceError::Intervention(
            InterventionServiceError::AssignmentNotActive
            | InterventionServiceError::AssignmentVersionConflict { .. }
            | InterventionServiceError::AttemptVersionConflict { .. }
            | InterventionServiceError::AttemptNotCurrent { .. },
        ) => ApiError {
            status: StatusCode::CONFLICT,
            code: "assignment_not_active",
            message: error.to_string(),
        },
        TerminalServiceError::InvalidCommand(_) => ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_terminal_command",
            message: error.to_string(),
        },
        TerminalServiceError::Runtime(
            RuntimeTerminalError::Unavailable(_) | RuntimeTerminalError::Protocol(_),
        )
        | TerminalServiceError::Intervention(_)
        | TerminalServiceError::CoordinationNode(_) => ApiError {
            status: StatusCode::BAD_GATEWAY,
            code: "runtime_terminal_unavailable",
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::http::{HeaderMap, HeaderValue, header};
    use tokio::time::{Duration, Instant};

    use super::{LeaseCheck, LeaseConfirmation, confirm_lease, lease_check_for, validate_origin};
    use crate::terminal_service::TerminalClientMessage;

    /// A stand-in for the store query that counts calls and fails once the
    /// lease is revoked.
    struct LeaseStore {
        queries: AtomicUsize,
        revoked: AtomicBool,
    }

    impl LeaseStore {
        fn new() -> Self {
            Self {
                queries: AtomicUsize::new(0),
                revoked: AtomicBool::new(false),
            }
        }

        fn queries(&self) -> usize {
            self.queries.load(Ordering::SeqCst)
        }

        async fn confirm(
            &self,
            confirmation: &mut LeaseConfirmation,
            check: LeaseCheck,
            at: Instant,
        ) -> Result<(), &'static str> {
            confirm_lease(confirmation, check, at, || async {
                self.queries.fetch_add(1, Ordering::SeqCst);
                if self.revoked.load(Ordering::SeqCst) {
                    Err("assignment_changed")
                } else {
                    Ok(())
                }
            })
            .await
        }
    }

    #[tokio::test]
    async fn reuses_a_lease_confirmation_for_scrolls_and_frames_only_within_250_ms() {
        let store = LeaseStore::new();
        let mut confirmation = LeaseConfirmation::default();
        let opened = Instant::now();
        let at = |ms| opened + Duration::from_millis(ms);

        // The first scroll or frame after open is always checked.
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Reusable, at(0))
                .await,
            Ok(())
        );
        assert_eq!(store.queries(), 1);
        // Scrolls and frames inside the window reuse it without extending it.
        for ms in [100, 200, 249] {
            assert_eq!(
                store
                    .confirm(&mut confirmation, LeaseCheck::Reusable, at(ms))
                    .await,
                Ok(())
            );
        }
        assert_eq!(store.queries(), 1);
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Reusable, at(250))
                .await,
            Ok(())
        );
        assert_eq!(store.queries(), 2);
        // Input and the lease tick always query the store.
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Required, at(260))
                .await,
            Ok(())
        );
        assert_eq!(store.queries(), 3);
    }

    #[test]
    fn only_scroll_commands_may_reuse_a_lease_confirmation() {
        for (command, check) in [
            (
                serde_json::json!({"type": "terminal.scroll", "direction": "up", "lines": 3, "source": "wheel"}),
                LeaseCheck::Reusable,
            ),
            (
                serde_json::json!({"type": "terminal.input", "text": "x"}),
                LeaseCheck::Required,
            ),
            (
                serde_json::json!({"type": "terminal.resize", "cols": 80, "rows": 24}),
                LeaseCheck::Required,
            ),
            (
                serde_json::json!({"type": "terminal.scroll_reset"}),
                LeaseCheck::Required,
            ),
            (
                serde_json::json!({"type": "terminal.release"}),
                LeaseCheck::Required,
            ),
        ] {
            let message: TerminalClientMessage = serde_json::from_value(command.clone()).unwrap();
            assert_eq!(lease_check_for(&message), check, "{command}");
        }
    }

    #[tokio::test]
    async fn rejects_a_revoked_lease_at_the_next_tick_input_or_expired_window() {
        let store = LeaseStore::new();
        let mut confirmation = LeaseConfirmation::default();
        let opened = Instant::now();
        let at = |ms| opened + Duration::from_millis(ms);
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Required, at(0))
                .await,
            Ok(())
        );
        store.revoked.store(true, Ordering::SeqCst);

        // A scroll inside the window may still pass: the documented bound,
        // which the 250 ms lease tick enforces.
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Reusable, at(100))
                .await,
            Ok(())
        );
        assert_eq!(store.queries(), 1);
        // Input (or the tick) inside the same window is rejected at once.
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Required, at(100))
                .await,
            Err("assignment_changed")
        );
        // A rejection is never reused.
        assert_eq!(
            store
                .confirm(&mut confirmation, LeaseCheck::Reusable, at(101))
                .await,
            Err("assignment_changed")
        );
        assert_eq!(store.queries(), 3);

        // Without the tick, a scroll or frame after the window rejects too.
        let mut stale = LeaseConfirmation::default();
        store.revoked.store(false, Ordering::SeqCst);
        assert_eq!(
            store
                .confirm(&mut stale, LeaseCheck::Reusable, at(200))
                .await,
            Ok(())
        );
        store.revoked.store(true, Ordering::SeqCst);
        assert_eq!(
            store
                .confirm(&mut stale, LeaseCheck::Reusable, at(450))
                .await,
            Err("assignment_changed")
        );
    }

    #[test]
    fn accepts_only_loopback_http_origins() {
        for origin in [
            "http://127.0.0.1:5173",
            "http://localhost:5173",
            "https://localhost",
            "http://[::1]:5173",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            assert!(validate_origin(&headers).is_ok(), "{origin}");
        }

        for origin in [
            "https://yard.example.com",
            "file://localhost",
            "http://localhost/path",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            assert!(validate_origin(&headers).is_err(), "{origin}");
        }
        assert!(validate_origin(&HeaderMap::new()).is_err());
    }
}

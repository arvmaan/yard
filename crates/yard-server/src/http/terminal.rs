use axum::{
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, Uri, header},
    response::Response,
};
use serde::Deserialize;
use std::{future::pending, time::Duration};
use tokio::{sync::watch, time::timeout};

use super::{ApiError, AppState};
use crate::{
    ConnectionGuard,
    coordination_node_service::CoordinationNodeServiceError,
    intervention_service::InterventionServiceError,
    terminal_service::{
        MAX_TERMINAL_MESSAGE_BYTES, RuntimeTerminalError, TerminalClientMessage,
        TerminalServerMessage, TerminalServiceError, sequence_continues,
    },
};

const TERMINAL_OPERATION_TIMEOUT: Duration = Duration::from_secs(6);
const TERMINAL_SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

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
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals, shutdown, connection)))
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
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals, shutdown, connection)))
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
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals, shutdown, connection)))
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
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals, shutdown, connection)))
}

#[allow(clippy::too_many_lines)]
async fn relay(
    mut socket: WebSocket,
    terminal: crate::terminal_service::OpenedTerminal,
    terminals: crate::terminal_service::TerminalService,
    mut shutdown: Option<watch::Receiver<bool>>,
    _connection: ConnectionGuard,
) {
    let crate::terminal_service::OpenedTerminal { lease, mut session } = terminal;
    let mut last_sequence = None;
    let mut lease_check = tokio::time::interval(Duration::from_millis(250));
    lease_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = wait_for_shutdown(&mut shutdown) => {
                send_closed(&mut socket, "server_shutdown").await;
                break;
            }
            _ = lease_check.tick() => {
                if let Err(reason) = validate_lease(&terminals, &lease).await {
                    send_closed(&mut socket, reason).await;
                    break;
                }
            }
            runtime_message = session.next_message() => {
                match runtime_message {
                    Ok(Some(message)) => {
                        if let Err(reason) = validate_lease(&terminals, &lease).await {
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
                        if let Err(reason) = validate_lease(&terminals, &lease).await {
                            send_closed(&mut socket, reason).await;
                            break;
                        }
                        match timeout(TERMINAL_OPERATION_TIMEOUT, session.send(command)).await {
                            Ok(Ok(())) => {}
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
                    Message::Ping(_) | Message::Pong(_) => {}
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
    let uri = origin.parse::<Uri>().map_err(|_| origin_forbidden())?;
    let scheme_allowed = matches!(uri.scheme_str(), Some("http" | "https"));
    let host_allowed = uri.authority().is_some_and(|authority| {
        matches!(
            authority.host(),
            "127.0.0.1" | "localhost" | "[::1]" | "::1"
        )
    });
    let path_allowed = uri.path_and_query().is_none_or(|path| path.as_str() == "/");
    if !scheme_allowed || !host_allowed || !path_allowed {
        return Err(origin_forbidden());
    }
    Ok(())
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
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::validate_origin;

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

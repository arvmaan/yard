use axum::{
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, Uri, header},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;

use super::{ApiError, AppState};
use crate::{
    coordination_node_service::CoordinationNodeServiceError,
    intervention_service::InterventionServiceError,
    terminal_service::{
        MAX_TERMINAL_MESSAGE_BYTES, RuntimeTerminalError, TerminalClientMessage,
        TerminalServerMessage, TerminalServiceError, sequence_continues,
    },
};

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
    let terminal = state
        .terminals
        .open(&project_id, &assignment_id, query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals)))
}

pub(super) async fn orchestrator_terminal(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let terminal = state
        .terminals
        .open_orchestrator(&project_id, query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals)))
}

pub(super) async fn yard_orchestrator_terminal(
    State(state): State<AppState>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let terminal = state
        .terminals
        .open_yard_orchestrator(query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals)))
}

pub(super) async fn coordination_node_terminal(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(query): Query<TerminalQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    validate_origin(&headers)?;
    let terminal = state
        .terminals
        .open_coordination_node(&node_id, query.cols, query.rows)
        .await
        .map_err(|error| terminal_error(&error))?;
    let terminals = state.terminals.clone();
    Ok(websocket
        .max_message_size(MAX_TERMINAL_MESSAGE_BYTES)
        .max_frame_size(MAX_TERMINAL_MESSAGE_BYTES)
        .on_upgrade(move |socket| relay(socket, terminal, terminals)))
}

async fn relay(
    mut socket: WebSocket,
    terminal: crate::terminal_service::OpenedTerminal,
    terminals: crate::terminal_service::TerminalService,
) {
    let crate::terminal_service::OpenedTerminal { lease, mut session } = terminal;
    let mut last_sequence = None;
    let mut lease_check = tokio::time::interval(Duration::from_millis(250));
    lease_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = lease_check.tick() => {
                if terminals.validate_lease(&lease).await.is_err() {
                    send_closed(&mut socket, lease.revoked_reason()).await;
                    break;
                }
            }
            runtime_message = session.next_message() => {
                match runtime_message {
                    Ok(Some(message)) => {
                        if terminals.validate_lease(&lease).await.is_err() {
                            send_closed(&mut socket, lease.revoked_reason()).await;
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
                            let _ = session.release().await;
                            send_closed(&mut socket, "released").await;
                            break;
                        }
                        if terminals.validate_lease(&lease).await.is_err() {
                            send_closed(&mut socket, lease.revoked_reason()).await;
                            break;
                        }
                        if let Err(error) = session.send(command).await {
                            tracing::warn!(%error, "interactive terminal command failed");
                            send_closed(&mut socket, "runtime_error").await;
                            break;
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
    let _ = session.release().await;
    let _ = socket.send(Message::Close(None)).await;
}

async fn send_message(socket: &mut WebSocket, message: &TerminalServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
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

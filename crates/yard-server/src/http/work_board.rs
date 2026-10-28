//! `GET /api/v1/work-board`, `POST /api/v1/work-board/generate` and
//! `POST /api/v1/work-board/triage` (Keep / Skip / Clear on one card).
//!
//! All stay on the single loopback listener and both run the loopback
//! `Host`/`Origin` guard: the board carries terminal-derived text, so even
//! the read is refused to a foreign page. Every response is `no-store`.

use axum::{
    Json,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::{ApiError, AppState, lifecycle_request_origin};
use crate::work_board::{DecideError, StartError, triage};

#[derive(Debug, Deserialize)]
pub(super) struct BoardQuery {
    since: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateRequest {
    #[serde(default)]
    use_model: bool,
    #[serde(default)]
    project_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TriageRequest {
    item_id: String,
    choice: triage::Choice,
}

fn json_content(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &'static str, message: &str) -> Response {
    no_store(
        ApiError {
            status,
            code,
            message: message.to_owned(),
        }
        .into_response(),
    )
}

pub(super) async fn get_work_board(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BoardQuery>,
) -> Response {
    if let Err(denied) = lifecycle_request_origin(&headers) {
        return no_store(denied.into_response());
    }
    let revision = state.work_board.revision();
    if query.since == Some(revision) {
        return no_store(
            Json(serde_json::json!({"unchanged": true, "revision": revision})).into_response(),
        );
    }
    no_store(Json(state.work_board.board()).into_response())
}

pub(super) async fn generate_work_board(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(denied) = lifecycle_request_origin(&headers) {
        return no_store(denied.into_response());
    }
    if !json_content(&headers) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Generate requires Content-Type: application/json",
        );
    }
    let Ok(request) = serde_json::from_slice::<GenerateRequest>(&body) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_work_board_request",
            "Expected {\"use_model\": bool, \"project_ids\"?: [string]}",
        );
    };
    if request
        .project_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > 256 || ids.iter().any(|id| id.is_empty() || id.len() > 120))
    {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_work_board_request",
            "project_ids must be at most 256 non-empty ids",
        );
    }
    match state
        .work_board
        .start(request.use_model, request.project_ids)
    {
        Ok(generation_id) => no_store(
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "generation_id": generation_id,
                    "revision": state.work_board.revision(),
                })),
            )
                .into_response(),
        ),
        Err(StartError::Running) => error(
            StatusCode::CONFLICT,
            "work_board_running",
            "A work board generation is already running",
        ),
        Err(StartError::TooSoon) => error(
            StatusCode::TOO_MANY_REQUESTS,
            "work_board_too_soon",
            "Wait a few seconds before generating the work board again",
        ),
        Err(StartError::CostCap(message)) => no_store(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": {"code": "work_board_cost_cap", "message": message},
                    "reason": "cost_cap",
                })),
            )
                .into_response(),
        ),
        Err(StartError::ModelDisabled) => error(
            StatusCode::BAD_REQUEST,
            "work_board_model_disabled",
            "The model pass is not configured (set YARD_BOARD_MODEL)",
        ),
    }
}

/// Keep (pin to the main board), Skip (reviewed until new activity) or
/// Clear one card. Same guards as Generate: loopback `Host`/`Origin`,
/// `Content-Type: application/json` (forces a CORS preflight), a strict
/// body, a board item id; `no-store` on every answer.
pub(super) async fn triage_work_board(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(denied) = lifecycle_request_origin(&headers) {
        return no_store(denied.into_response());
    }
    if !json_content(&headers) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Triage requires Content-Type: application/json",
        );
    }
    let Ok(request) = serde_json::from_slice::<TriageRequest>(&body) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_work_board_request",
            "Expected {\"item_id\": string, \"choice\": \"keep\"|\"skip\"|\"clear\"}",
        );
    };
    if !triage::valid_item_id(&request.item_id) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_work_board_request",
            "item_id is not a work board card id",
        );
    }
    let service = state.work_board.clone();
    let decided =
        tokio::task::spawn_blocking(move || service.decide(&request.item_id, request.choice)).await;
    match decided {
        Ok(Ok(revision)) => {
            no_store(Json(serde_json::json!({ "revision": revision })).into_response())
        }
        Ok(Err(DecideError::UnknownItem)) => error(
            StatusCode::NOT_FOUND,
            "work_board_item_not_found",
            "That card is not on the current board; refresh it first",
        ),
        Ok(Err(DecideError::Write(message))) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "work_board_triage_unsaved",
            &format!("The decision applies now but could not be saved: {message}"),
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "work_board_triage_failed",
            "The decision could not be recorded",
        ),
    }
}

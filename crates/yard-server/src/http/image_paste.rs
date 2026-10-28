//! `POST <terminal route>/images`: store an image pasted into a web terminal.
//!
//! Each interactive terminal route (`…/terminal`) has an `…/terminal/images`
//! sibling taking the same identifiers. The body is the raw image with its
//! `Content-Type`; the answer is the absolute path the browser then types
//! into the attached pane. See [`crate::image_paste_service`].

use std::time::{Duration, SystemTime};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
};
use futures_util::StreamExt as _;
use serde::Serialize;

use super::{ApiError, AppState, NoStoreJson, lifecycle_request_origin, terminal::terminal_error};
use crate::{
    image_paste_service::{ImagePasteError, PastedImageType, save_pasted_image},
    terminal_service::TerminalTarget,
};

#[derive(Debug, Serialize)]
pub(super) struct PastedImage {
    path: String,
    name: String,
    bytes: u64,
    content_type: &'static str,
}

pub(super) async fn assignment_terminal_image(
    State(state): State<AppState>,
    Path((project_id, assignment_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, NoStoreJson<PastedImage>), ApiError> {
    let target = TerminalTarget::Assignment {
        project_id: &project_id,
        assignment_id: &assignment_id,
    };
    store_image(&state, target, &headers, body).await
}

pub(super) async fn orchestrator_terminal_image(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, NoStoreJson<PastedImage>), ApiError> {
    let target = TerminalTarget::Orchestrator {
        project_id: &project_id,
    };
    store_image(&state, target, &headers, body).await
}

pub(super) async fn yard_orchestrator_terminal_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, NoStoreJson<PastedImage>), ApiError> {
    store_image(&state, TerminalTarget::YardOrchestrator, &headers, body).await
}

pub(super) async fn coordination_node_terminal_image(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, NoStoreJson<PastedImage>), ApiError> {
    let target = TerminalTarget::CoordinationNode { node_id: &node_id };
    store_image(&state, target, &headers, body).await
}

async fn store_image(
    state: &AppState,
    target: TerminalTarget<'_>,
    headers: &HeaderMap,
    body: Body,
) -> Result<(StatusCode, NoStoreJson<PastedImage>), ApiError> {
    lifecycle_request_origin(headers)?;
    let settings = state.image_paste.clone();
    if settings.root().is_none() {
        return Err(paste_error(&ImagePasteError::NotConfigured));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if PastedImageType::from_content_type(&content_type).is_none() {
        return Err(paste_error(&ImagePasteError::UnsupportedType(content_type)));
    }
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(bytes) = declared_too_large(declared, settings.max_bytes) {
        return Err(too_large(body, bytes, 0, settings.max_bytes).await);
    }
    let worker_id = state
        .terminals
        .attachable_worker(target)
        .await
        .map_err(|error| terminal_error(&error))?;
    // Held until the image is on disk, so at most CONCURRENT_IMAGE_UPLOADS
    // bodies (each at most `max_bytes`) are in memory at once.
    // Neither the wait for a slot nor the body read is unbounded, so a slow
    // or stalled sender cannot hold a slot (or the browser's queued input).
    let _permit = tokio::time::timeout(
        IMAGE_UPLOAD_SLOT_WAIT,
        state.image_uploads.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "image_paste_busy",
        message: "Yard is busy storing other pasted images; paste again in a moment".to_owned(),
    })?
    .map_err(|_| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "image_paste_failed",
        message: "Yard is shutting down".to_owned(),
    })?;
    let bytes = read_limited_within(body, settings.max_bytes, IMAGE_BODY_READ_DEADLINE).await?;
    let saved = tokio::task::spawn_blocking(move || {
        save_pasted_image(
            &settings,
            &worker_id,
            &content_type,
            &bytes,
            SystemTime::now(),
        )
    })
    .await
    .map_err(|_| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "image_paste_failed",
        message: "the pasted image could not be stored".to_owned(),
    })?
    .map_err(|error| paste_error(&error))?;
    Ok((
        StatusCode::CREATED,
        NoStoreJson(PastedImage {
            path: saved.path.to_string_lossy().into_owned(),
            name: saved.name,
            bytes: saved.bytes,
            content_type: saved.image_type.mime(),
        }),
    ))
}

/// How many image uploads may be buffered at the same time.
pub(super) const CONCURRENT_IMAGE_UPLOADS: usize = 4;

/// How long an upload waits for one of the [`CONCURRENT_IMAGE_UPLOADS`] slots.
const IMAGE_UPLOAD_SLOT_WAIT: Duration = Duration::from_secs(15);

/// How long the whole body (including draining a refused one) may take to
/// arrive. The browser gives up after 60 s as well.
const IMAGE_BODY_READ_DEADLINE: Duration = Duration::from_secs(60);

/// How much of a refused, oversized body is read and discarded before the
/// 413 goes out. Answering while the browser is still sending makes hyper
/// close the connection, which the browser reports as a network error
/// instead of the 413; draining lets the response arrive. Nothing drained is
/// kept in memory, and a body larger than this still gets the early close.
const OVERSIZED_DRAIN_LIMIT: u64 = 128 * 1024 * 1024;

/// The declared `Content-Length` when it is over `limit`.
pub(super) fn declared_too_large(declared: Option<u64>, limit: u64) -> Option<u64> {
    declared.filter(|bytes| *bytes > limit)
}

/// [`read_limited`] within `deadline`; a body still arriving then is refused.
async fn read_limited_within(
    body: Body,
    limit: u64,
    deadline: Duration,
) -> Result<Vec<u8>, ApiError> {
    tokio::time::timeout(deadline, read_limited(body, limit))
        .await
        .unwrap_or_else(|_| {
            Err(ApiError {
                status: StatusCode::REQUEST_TIMEOUT,
                code: "image_upload_timed_out",
                message: "the image upload took too long".to_owned(),
            })
        })
}

/// Read the body, refusing it as soon as it passes `limit` bytes.
async fn read_limited(body: Body, limit: u64) -> Result<Vec<u8>, ApiError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| interrupted())?;
        bytes.extend_from_slice(&chunk);
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if length > limit {
            drop(bytes);
            return Err(too_large(Body::from_stream(stream), length, length, limit).await);
        }
    }
    Ok(bytes)
}

/// Drain (and discard) what is left of an oversized body, then build its 413.
/// `bytes` is the size to report; `read` is how much was already consumed.
async fn too_large(body: Body, bytes: u64, read: u64, limit: u64) -> ApiError {
    let mut stream = body.into_data_stream();
    let mut drained = read;
    while drained <= OVERSIZED_DRAIN_LIMIT {
        match stream.next().await {
            Some(Ok(chunk)) => {
                drained = drained.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            }
            Some(Err(_)) | None => break,
        }
    }
    paste_error(&ImagePasteError::TooLarge {
        bytes: bytes.max(drained),
        limit,
    })
}

fn interrupted() -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "image_upload_interrupted",
        message: "the image upload did not complete".to_owned(),
    }
}

fn paste_error(error: &ImagePasteError) -> ApiError {
    let (status, code) = match error {
        ImagePasteError::NotConfigured => (
            StatusCode::SERVICE_UNAVAILABLE,
            "image_paste_not_configured",
        ),
        ImagePasteError::Empty => (StatusCode::BAD_REQUEST, "image_empty"),
        ImagePasteError::UnsupportedType(_) => {
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_image_type")
        }
        ImagePasteError::TypeMismatch(_) => {
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "image_type_mismatch")
        }
        ImagePasteError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "image_too_large"),
        ImagePasteError::UnsafeDirectory(_) => {
            tracing::error!(%error, "refused an unsafe image paste directory");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "image_paste_unsafe_directory",
            )
        }
        ImagePasteError::Io(_) => {
            tracing::error!(%error, "could not store a pasted image");
            (StatusCode::INTERNAL_SERVER_ERROR, "image_paste_failed")
        }
    };
    let message = match error {
        // Do not echo server paths to the page.
        ImagePasteError::UnsafeDirectory(_) | ImagePasteError::Io(_) => {
            "the pasted image could not be stored on the Yard host".to_owned()
        }
        other => other.to_string(),
    };
    ApiError {
        status,
        code,
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{body::Body, http::StatusCode};
    use futures_util::StreamExt as _;

    use super::read_limited_within;

    #[tokio::test]
    async fn a_stalled_body_is_refused_at_the_deadline() {
        // One chunk, then nothing more ever arrives.
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(
            axum::body::Bytes::from_static(b"\x89PN"),
        )])
        .chain(futures_util::stream::pending());
        let error = read_limited_within(Body::from_stream(stream), 1024, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(error.code, "image_upload_timed_out");
    }

    #[tokio::test]
    async fn a_complete_body_within_the_deadline_is_read() {
        let read = read_limited_within(Body::from("abc"), 1024, Duration::from_secs(5)).await;
        assert!(matches!(read.as_deref(), Ok(b"abc")));
    }
}

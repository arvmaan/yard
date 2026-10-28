//! Route tests of the work board (child of `http::tests`, so it reuses its
//! fixtures).

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use tower::ServiceExt;
use yard_domain::{RuntimeInventory, RuntimeSessions};

use super::{FakeInventory, response_json, test_router, test_router_with_source};
use crate::inventory_service::{InventoryServiceError, InventorySource};

/// Herdr that answers slowly, so a generation stays running for a while.
/// A Herdr whose session list blocks until the test releases it, so the
/// tests observe the running stages without depending on timing.
#[derive(Clone, Default)]
struct SlowInventory(Arc<tokio::sync::Notify>);

impl SlowInventory {
    fn release(&self) {
        self.0.notify_one();
    }
}

#[async_trait]
impl InventorySource for SlowInventory {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
        self.0.notified().await;
        FakeInventory.sessions().await
    }

    async fn inventory(
        &self,
        session_name: &str,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        FakeInventory.inventory(session_name).await
    }
}

fn get(uri: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn generate(body: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/work-board/generate");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

const JSON: (&str, &str) = ("content-type", "application/json");

async fn send(
    app: &Router,
    request: Request<Body>,
) -> (StatusCode, Option<String>, serde_json::Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .map(|value| value.to_str().unwrap().to_owned());
    (status, cache, response_json(response).await)
}

async fn wait_idle(app: &Router) -> serde_json::Value {
    for _ in 0..200 {
        let (_, _, board) = send(app, get("/api/v1/work-board", &[])).await;
        if board["state"] == "idle" || board["state"] == "failed" {
            return board;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the work board never finished");
}

#[tokio::test]
async fn empty_board_then_generate_then_idle() {
    let (app, _temp) = test_router().await;
    let (status, cache, board) = send(&app, get("/api/v1/work-board", &[])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert_eq!(board["state"], "empty");
    assert_eq!(board["model_available"], false);
    assert_eq!(board["thresholds"]["cold_ms"], 7 * 24 * 60 * 60 * 1000);

    let (status, cache, accepted) = send(&app, generate(r#"{"use_model":false}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert!(accepted["generation_id"].is_string());

    let board = wait_idle(&app).await;
    assert_eq!(board["state"], "idle", "{board}");
    assert_eq!(board["mode"], "deterministic");
    for key in [
        "items",
        "todos",
        "projects",
        "counts",
        "fingerprint",
        "generated_at_unix_ms",
    ] {
        assert!(!board[key].is_null(), "{key} missing: {board}");
    }
    let revision = board["revision"].as_u64().unwrap();
    let (_, _, unchanged) = send(
        &app,
        get(&format!("/api/v1/work-board?since={revision}"), &[]),
    )
    .await;
    assert_eq!(unchanged["unchanged"], true);
    let (_, _, changed) = send(&app, get("/api/v1/work-board?since=0", &[])).await;
    assert!(changed.get("unchanged").is_none());

    // A second generate right away is rate limited.
    let (status, cache, error) = send(&app, generate(r#"{"use_model":false}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert_eq!(error["error"]["code"], "work_board_too_soon");
}

#[tokio::test]
async fn generate_is_single_flight() {
    let herdr = SlowInventory::default();
    let (app, _temp) = test_router_with_source(Arc::new(herdr.clone())).await;
    let (status, _, _) = send(&app, generate("{}", &[JSON])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, _, running) = send(&app, get("/api/v1/work-board", &[])).await;
    assert_eq!(running["state"], "running");
    let (status, _, error) = send(&app, generate("{}", &[JSON])).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "work_board_running");
    herdr.release();
    assert_eq!(wait_idle(&app).await["state"], "idle");
}

#[tokio::test]
async fn generate_validates_content_type_body_and_model() {
    let (app, _temp) = test_router().await;
    let (status, cache, _) = send(&app, generate(r#"{"use_model":false}"#, &[])).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(cache.as_deref(), Some("no-store"));
    let (status, _, _) = send(
        &app,
        generate(r#"{"use_model":false}"#, &[("content-type", "text/plain")]),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (status, _, _) = send(&app, generate("not json", &[JSON])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = send(&app, generate(r#"{"surprise":1}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, error) = send(&app, generate(r#"{"use_model":true}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "work_board_model_disabled");
}

#[tokio::test]
async fn foreign_origin_and_host_are_refused_on_both_routes() {
    let (app, _temp) = test_router().await;
    for headers in [
        &[("origin", "https://example.com")][..],
        &[("origin", "null")][..],
        &[("host", "example.com")][..],
        &[("host", "yard.example.com:4317")][..],
    ] {
        let (status, cache, error) = send(&app, get("/api/v1/work-board", headers)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}");
        assert_eq!(cache.as_deref(), Some("no-store"));
        assert!(
            error["error"]["code"] == "request_origin_forbidden"
                || error["error"]["code"] == "request_host_forbidden"
        );
        let mut with_json = headers.to_vec();
        with_json.push(JSON);
        let (status, _, _) = send(&app, generate("{}", &with_json)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}");
    }
    // The loopback origin is accepted.
    let (status, _, _) = send(
        &app,
        get(
            "/api/v1/work-board",
            &[
                ("origin", "http://127.0.0.1:4317"),
                ("host", "127.0.0.1:4317"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Nothing was started by the refused requests.
    let (_, _, board) = send(&app, get("/api/v1/work-board", &[])).await;
    assert_eq!(board["state"], "empty");
}

/// Herdr is off the critical path: with a slow Herdr the deterministic board
/// is already published while the Herdr stage is still running.
#[tokio::test]
async fn deterministic_board_publishes_before_slow_herdr_reads() {
    let herdr = SlowInventory::default();
    let (app, _temp) = test_router_with_source(Arc::new(herdr.clone())).await;
    let started = std::time::Instant::now();
    let (status, _, _) = send(&app, generate("{}", &[JSON])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // Herdr never answers until released: the board must appear anyway.
    let mut first = None;
    for _ in 0..1_000 {
        let (_, _, board) = send(&app, get("/api/v1/work-board", &[])).await;
        if !board["items"].is_null() {
            first = Some((started.elapsed(), board));
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (latency, board) = first.expect("no deterministic board while Herdr is blocked");
    assert_eq!(board["state"], "running", "{board}");
    assert_eq!(board["stage"], "reports");
    herdr.release();
    let done = wait_idle(&app).await;
    assert_eq!(done["state"], "idle", "{done}");
    // Latency is reported, not asserted against the wall clock.
    eprintln!("first board after {latency:?}; timings {}", done["timings"]);
}

struct NeverCalled;

#[async_trait]
impl crate::model::ConverseRunner for NeverCalled {
    async fn converse(
        &self,
        _request: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::model::ModelError> {
        panic!("the cost cap must refuse before any call");
    }
}

#[tokio::test]
async fn model_generate_over_the_cost_cap_is_422_before_any_call() {
    // An unknown model id is priced at the table's highest rate; with the
    // daily cap nearly spent not even one item fits.
    crate::work_board::TEST_SETTINGS.with(|settings| {
        let mut seeded = crate::work_board::WorkBoardSettings::with_model(
            "mystery-model",
            Arc::new(NeverCalled),
        );
        seeded.spent_today_usd = 0.97;
        *settings.borrow_mut() = Some(seeded);
    });
    let (app, _temp) = test_router().await;
    let (_, _, board) = send(&app, get("/api/v1/work-board", &[])).await;
    assert_eq!(board["model_available"], true);
    let estimate = board["model_estimate_usd"].as_f64().unwrap();
    // Trimmed to the per-generate cap, never above it.
    assert!(estimate > 0.05 && estimate <= 0.10, "{estimate}");
    let (status, cache, error) = send(&app, generate(r#"{"use_model":true}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert_eq!(error["reason"], "cost_cap");
    assert_eq!(error["error"]["code"], "work_board_cost_cap");
    // The deterministic generate still works.
    let (status, _, _) = send(&app, generate(r#"{"use_model":false}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

fn triage(body: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/work-board/triage");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

#[tokio::test]
async fn triage_is_guarded_validated_and_re_places_cards() {
    let (app, _temp) = test_router().await;
    // Foreign pages are refused before anything else, with no-store.
    for headers in [
        &[("origin", "https://example.com"), JSON][..],
        &[("origin", "null"), JSON][..],
        &[("host", "example.com"), JSON][..],
    ] {
        let (status, cache, _) = send(
            &app,
            triage(r#"{"item_id":"w:x","choice":"keep"}"#, headers),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}");
        assert_eq!(cache.as_deref(), Some("no-store"));
    }
    let (status, _, _) = send(&app, triage(r#"{"item_id":"w:x","choice":"keep"}"#, &[])).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    for body in [
        r#"{"item_id":"w:x"}"#,
        r#"{"item_id":"w:x","choice":"archive"}"#,
        r#"{"item_id":"w:x","choice":"keep","extra":1}"#,
        r#"{"item_id":"bad id","choice":"keep"}"#,
        r#"{"item_id":"z:1","choice":"keep"}"#,
    ] {
        let (status, cache, _) = send(&app, triage(body, &[JSON])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(cache.as_deref(), Some("no-store"));
    }
    // A card not on the board is a 404 (Clear is always accepted).
    let (status, _, error) = send(
        &app,
        triage(r#"{"item_id":"w:missing","choice":"keep"}"#, &[JSON]),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "work_board_item_not_found");
    let (status, _, _) = send(
        &app,
        triage(r#"{"item_id":"w:missing","choice":"clear"}"#, &[JSON]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = send(&app, generate(r#"{"use_model":false}"#, &[JSON])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let board = wait_idle(&app).await;
    assert!(board["thresholds"]["recent_ms"].as_u64().unwrap() > 0);
    assert!(board["older"].is_u64() && board["helpers"].is_u64());
    let Some(id) = board["items"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["id"].as_str())
        .map(str::to_owned)
    else {
        return;
    };
    let before = board["revision"].as_u64().unwrap();
    let body = format!(r#"{{"item_id":"{id}","choice":"keep"}}"#);
    let (status, cache, answer) = send(&app, triage(&body, &[JSON])).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert!(answer["revision"].as_u64().unwrap() > before);
    let (_, _, board) = send(&app, get("/api/v1/work-board", &[])).await;
    let item = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id.as_str())
        .unwrap();
    assert_eq!(item["pinned"], true);
}

//! Route tests of the loopback Host/Origin guard on state-changing routes
//! (child of `http::tests`, so it reuses its fixtures).

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use tower::ServiceExt;

use super::{response_json, test_router};

const JSON: (&str, &str) = ("content-type", "application/json");
const LOOPBACK_BROWSER: [(&str, &str); 3] = [
    ("host", "127.0.0.1:7878"),
    ("origin", "http://127.0.0.1:7878"),
    JSON,
];

/// Lifecycle commands whose `{}` body axum rejects as an incomplete command
/// (422) once the guard has let the request through.
const LIFECYCLE_ROUTES: [&str; 5] = [
    "/api/v1/workers/worker-1/end-session",
    "/api/v1/projects/project-1/assignments/assignment-1/completion-receipts",
    "/api/v1/coordination-nodes/node-1/archive",
    "/api/v1/coordination-nodes/node-1/delete",
    "/api/v1/workers/worker-1/delete",
];

/// A sample of the other state-changing routes, across methods. The guard is
/// one router layer, so these prove it covers routes without their own check.
const OTHER_MUTATING_ROUTES: [(Method, &str); 8] = [
    (
        Method::POST,
        "/api/v1/projects/project-1/assignments/assignment-1/prompts",
    ),
    (
        Method::POST,
        "/api/v1/projects/project-1/orchestrator/replace",
    ),
    (Method::POST, "/api/v1/coordination-nodes/node-1/provision"),
    (
        Method::POST,
        "/api/v1/project-relationships/relationship-1/delete",
    ),
    (Method::POST, "/api/v1/workers/cleanup/runs/run-1/cancel"),
    (Method::POST, "/api/v1/yard/orchestrator/prompts"),
    (Method::PUT, "/api/v1/token-spend-settings"),
    (
        Method::DELETE,
        "/api/v1/projects/project-1/repositories/repository-1",
    ),
];

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from("{}")).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = if status == StatusCode::FORBIDDEN {
        assert_eq!(response.headers()["cache-control"], "no-store", "{uri}");
        response_json(response).await
    } else {
        serde_json::Value::Null
    };
    (status, body)
}

async fn assert_refused(app: &Router, method: Method, uri: &str) {
    for (headers, code) in [
        (
            &[("origin", "https://example.com"), JSON][..],
            "request_origin_forbidden",
        ),
        // A DNS-rebinding page: its own host name, served from 127.0.0.1.
        (
            &[("host", "yard.attacker.test"), JSON][..],
            "request_host_forbidden",
        ),
        (
            &[
                ("host", "yard.attacker.test"),
                ("origin", "http://yard.attacker.test"),
                JSON,
            ][..],
            "request_host_forbidden",
        ),
    ] {
        let (status, body) = send(app, method.clone(), uri, headers).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} {headers:?}");
        assert_eq!(body["error"]["code"], code, "{method} {uri} {headers:?}");
    }
}

/// Lifecycle routes refuse a foreign page before they read the body, and let
/// both the loopback web UI and a client without `Origin` (CLI, agent)
/// through to the handler, which rejects the incomplete `{}` command.
#[tokio::test]
async fn lifecycle_routes_refuse_foreign_origins() {
    let (app, _temp) = test_router().await;
    for uri in LIFECYCLE_ROUTES {
        assert_refused(&app, Method::POST, uri).await;
        for headers in [&LOOPBACK_BROWSER[..], &[JSON][..]] {
            let (status, _) = send(&app, Method::POST, uri, headers).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{uri} {headers:?}"
            );
        }
    }
}

/// Every other state-changing route is guarded the same way.
#[tokio::test]
async fn every_mutating_route_refuses_foreign_origins() {
    let (app, _temp) = test_router().await;
    for (method, uri) in OTHER_MUTATING_ROUTES {
        assert_refused(&app, method.clone(), uri).await;
        let (status, body) = send(&app, method.clone(), uri, &LOOPBACK_BROWSER).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{method} {uri} {body}");
    }
    // Reads are not affected by the mutation guard.
    let (status, _) = send(
        &app,
        Method::GET,
        "/api/v1/projects",
        &[("origin", "https://example.com")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

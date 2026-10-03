//! End-to-end notifier tests against a local mock Slack API and a fake `aws`
//! executable. Nothing here reaches slack.com or AWS.

use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Bytes,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use yard_domain::{
    CanvasPlacement, CreateProject, ObservedStatus, ProjectRuntimeBinding, RuntimeObservationState,
    RuntimeProcessState, WorkerRuntimeBinding,
};
use yard_store::{SqliteProjectStore, YardStore};

use super::{
    NotifierParts, SlackNotifier, SlackStatusKind, SlackTestError, client::SlackApi,
    detector::DetectorTiming,
};
use crate::config::{SlackConfig, SlackSettings};

pub(crate) const TOKEN: &str = concat!("xo", "xb-9999-8888-SECRETTOKENVALUE");
const ROTATED_TOKEN: &str = concat!("xo", "xb-9999-8888-ROTATEDTOKENVALUE");
const OWNER: &str = "U01OWNER";
const SANDBOX_ENTERPRISE: &str = "E01SANDBOX0";

#[derive(Debug, Clone)]
pub(crate) struct MockCall {
    pub method: String,
    pub authorization: String,
    pub body: Value,
}

pub(crate) enum MockReply {
    Json(Value),
    RateLimited(u64),
}

#[derive(Default)]
pub(crate) struct MockState {
    pub calls: Vec<MockCall>,
    scripted: HashMap<String, VecDeque<MockReply>>,
    next_ts: u64,
    /// What `apps.connections.open` returns (phase 2).
    socket_url: Option<String>,
}

#[derive(Clone)]
pub(crate) struct MockSlack {
    pub base_url: String,
    state: Arc<Mutex<MockState>>,
}

impl MockSlack {
    pub(crate) async fn start() -> Self {
        let state = Arc::new(Mutex::new(MockState::default()));
        let router = Router::new()
            .route("/api/{method}", post(mock_handler))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base_url: format!("http://{address}/api/"),
            state,
        }
    }

    pub(crate) fn script(&self, method: &str, reply: MockReply) {
        self.state
            .lock()
            .unwrap()
            .scripted
            .entry(method.to_owned())
            .or_default()
            .push_back(reply);
    }

    pub(crate) fn set_socket_url(&self, url: &str) {
        self.state.lock().unwrap().socket_url = Some(url.to_owned());
    }

    pub(crate) fn calls(&self, method: &str) -> Vec<MockCall> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|call| call.method == method)
            .cloned()
            .collect()
    }
}

async fn mock_handler(
    State(state): State<Arc<Mutex<MockState>>>,
    UrlPath(method): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut state = state.lock().unwrap();
    state.calls.push(MockCall {
        method: method.clone(),
        authorization: headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    if let Some(reply) = state
        .scripted
        .get_mut(&method)
        .and_then(VecDeque::pop_front)
    {
        return match reply {
            MockReply::Json(value) => axum::Json(value).into_response(),
            MockReply::RateLimited(seconds) => (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, seconds.to_string())],
                "rate limited",
            )
                .into_response(),
        };
    }
    let reply = match method.as_str() {
        "auth.test" => json!({
            "ok": true, "team": "Acme Sandbox", "team_id": "T01SANDBOX",
            "user_id": "UBOT", "bot_id": "B01BOT", "enterprise_id": SANDBOX_ENTERPRISE
        }),
        "conversations.open" => json!({ "ok": true, "channel": { "id": "D01DM" } }),
        "chat.postMessage" => {
            state.next_ts += 1;
            json!({ "ok": true, "channel": "D01DM", "ts": format!("1700000000.{:06}", state.next_ts) })
        }
        "chat.update" => json!({ "ok": true }),
        "apps.connections.open" => match &state.socket_url {
            Some(url) => json!({ "ok": true, "url": url }),
            None => json!({ "ok": false, "error": "invalid_auth" }),
        },
        _ => json!({ "ok": false, "error": "unknown_method" }),
    };
    axum::Json(reply).into_response()
}

/// A fake `aws` that prints the token in `dir/token` as secret JSON and
/// counts its runs in `dir/runs`.
pub(crate) fn fake_aws(dir: &Path, token: &str) -> PathBuf {
    std::fs::write(dir.join("token"), token).unwrap();
    let script = dir.join("aws");
    let mut file = std::fs::File::create(&script).unwrap();
    writeln!(
        file,
        "#!/bin/sh\necho run >> '{runs}'\nprintf '{{\"bot_token\":\"%s\"}}\\n' \"$(cat '{token}')\"",
        runs = dir.join("runs").display(),
        token = dir.join("token").display(),
    )
    .unwrap();
    drop(file);
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn aws_runs(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("runs")).map_or(0, |runs| runs.lines().count())
}

pub(crate) fn settings(enterprise: Option<&str>) -> SlackSettings {
    SlackSettings {
        secret_id: "yard/slack-bot".to_owned(),
        aws_profile: None,
        aws_region: "us-west-2".to_owned(),
        owner_user_id: OWNER.to_owned(),
        enterprise_id: enterprise.map(str::to_owned),
        inbound: crate::config::SlackInbound::Off,
    }
}

pub(crate) fn instant_timing() -> DetectorTiming {
    DetectorTiming {
        blocked_settle: Duration::ZERO,
        ready_settle: Duration::ZERO,
        command_settle: Duration::ZERO,
    }
}

pub(crate) fn notifier(
    temp: &TempDir,
    store: Arc<dyn YardStore>,
    slack: &MockSlack,
    config: SlackConfig,
) -> SlackNotifier {
    capture_logs();
    SlackNotifier::with_parts(
        config,
        store,
        NotifierParts {
            api: SlackApi::with_base_url(slack.base_url.clone()),
            aws_binary: fake_aws(temp.path(), TOKEN).into(),
            thread_file: Some(temp.path().join("slack-threads.json")),
            timing: instant_timing(),
            min_interval: Duration::ZERO,
            inbound: super::hub::InboundParts::default(),
        },
    )
}

/// A project whose orchestrator runtime is bound and working.
pub(crate) async fn store_with_project(temp: &TempDir) -> (Arc<SqliteProjectStore>, String) {
    capture_logs();
    let store = Arc::new(
        SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
            .await
            .unwrap(),
    );
    let project = store
        .create_project(
            CreateProject {
                name: "Checkout <!channel>".to_owned(),
                runtime: ProjectRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "default".to_owned(),
                    workspace_id: "workspace-1".to_owned(),
                },
                orchestrator_observed_worker_id: "terminal-1".to_owned(),
                placement: CanvasPlacement {
                    x: 0.0,
                    y: 0.0,
                    width: 322.0,
                    height: 240.0,
                },
            },
            WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "default".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                terminal_id: "terminal-1".to_owned(),
                tab_id: Some("tab-1".to_owned()),
                pane_id: "pane-1".to_owned(),
                provider_session: None,
                owns_tab: false,
                observation_state: RuntimeObservationState::Observed,
                process_state: RuntimeProcessState::Running,
                status: ObservedStatus::Working,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 1,
            },
        )
        .await
        .unwrap();
    (store, project.id)
}

pub(crate) fn set_orchestrator_status(temp: &TempDir, status: &str, sequence: u64) {
    rusqlite::Connection::open(temp.path().join("yard.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE worker_runtime_bindings SET observed_status = ?1, state_change_sequence = ?2",
            rusqlite::params![status, i64::try_from(sequence).unwrap()],
        )
        .unwrap();
}

#[tokio::test]
async fn blocked_orchestrator_is_posted_once_then_threaded_per_project() {
    let temp = TempDir::new().unwrap();
    let (store, project_id) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let notifier = notifier(
        &temp,
        store,
        &slack,
        SlackConfig::Enabled(settings(Some(SANDBOX_ENTERPRISE))),
    );

    // Startup: silent baseline plus auth.test and conversations.open.
    notifier.tick().await;
    let status = notifier.status();
    assert_eq!(status.status, SlackStatusKind::Connected, "{status:?}");
    assert_eq!(status.team.as_ref().unwrap().id, "T01SANDBOX");
    assert_eq!(slack.calls("auth.test").len(), 1);
    assert_eq!(
        slack.calls("auth.test")[0].authorization,
        format!("Bearer {TOKEN}")
    );
    assert_eq!(
        slack.calls("conversations.open")[0].body,
        json!({ "users": OWNER })
    );
    assert!(slack.calls("chat.postMessage").is_empty());

    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await; // pending
    notifier.tick().await; // settled and re-checked
    notifier.tick().await; // already claimed
    let posts = slack.calls("chat.postMessage");
    assert_eq!(posts.len(), 1, "{posts:?}");
    let first = &posts[0].body;
    assert_eq!(first["channel"], "D01DM");
    assert!(first.get("thread_ts").is_none());
    assert_eq!(first["unfurl_links"], false);
    let text = first["text"].as_str().unwrap();
    assert!(text.contains("Checkout &lt;!channel&gt;"), "{text}");
    assert!(
        text.contains("The project orchestrator is blocked"),
        "{text}"
    );
    assert!(notifier.status().last_sent_at.is_some());

    // A new blocked episode replies in the project's thread and broadcasts.
    set_orchestrator_status(&temp, "working", 3);
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 4);
    notifier.tick().await;
    notifier.tick().await;
    let posts = slack.calls("chat.postMessage");
    assert_eq!(posts.len(), 2, "{posts:?}");
    assert_eq!(posts[1].body["thread_ts"], "1700000000.000001");
    assert_eq!(posts[1].body["reply_broadcast"], true);

    // The thread survives a restart (same file, same team and channel).
    let threads = std::fs::read_to_string(temp.path().join("slack-threads.json")).unwrap();
    assert!(
        threads.contains(&format!("project:{project_id}")),
        "{threads}"
    );
    assert!(!threads.contains("TOKENVALUE"));
}

#[tokio::test]
async fn enterprise_mismatch_fails_closed() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script(
        "auth.test",
        MockReply::Json(json!({
            "ok": true, "team": "Elsewhere", "team_id": "T02", "enterprise_id": "E0OTHER"
        })),
    );
    let notifier = notifier(
        &temp,
        store,
        &slack,
        SlackConfig::Enabled(settings(Some(SANDBOX_ENTERPRISE))),
    );
    notifier.tick().await;
    let status = notifier.status();
    assert_eq!(status.status, SlackStatusKind::Misconfigured);
    assert!(status.last_error.unwrap().contains("E0OTHER"));
    assert!(slack.calls("conversations.open").is_empty());
    assert!(slack.calls("chat.postMessage").is_empty());
}

#[tokio::test]
async fn rate_limited_posts_wait_for_retry_after() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script("chat.postMessage", MockReply::RateLimited(120));
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    let status = notifier.status();
    assert_eq!(status.status, SlackStatusKind::Error);
    assert!(status.last_error.unwrap().contains("120 s"));
    // Still queued, but not retried before Retry-After, and the error stays
    // visible while the session itself is fine.
    notifier.tick().await;
    notifier.tick().await;
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    assert_eq!(notifier.status().status, SlackStatusKind::Error);
}

#[tokio::test]
async fn invalid_auth_drops_the_token_and_rereads_the_secret() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script(
        "chat.postMessage",
        MockReply::Json(json!({ "ok": false, "error": "token_revoked" })),
    );
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    assert_eq!(aws_runs(temp.path()), 1);
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    let status = notifier.status();
    assert_eq!(status.status, SlackStatusKind::Error);
    assert!(status.last_error.unwrap().contains("token_revoked"));

    // The owner rotated the secret; the test button skips the backoff.
    std::fs::write(temp.path().join("token"), ROTATED_TOKEN).unwrap();
    let status = notifier.send_test().await.unwrap();
    assert_eq!(status.status, SlackStatusKind::Connected);
    assert_eq!(aws_runs(temp.path()), 2);
    let posts = slack.calls("chat.postMessage");
    let last = posts.last().unwrap();
    assert_eq!(last.authorization, format!("Bearer {ROTATED_TOKEN}"));
    assert!(
        last.body["text"]
            .as_str()
            .unwrap()
            .starts_with("Yard test message")
    );
    assert!(last.body.get("thread_ts").is_none());
    assert_eq!(
        notifier.send_test().await.unwrap_err(),
        SlackTestError::RateLimited
    );
}

#[test]
fn failed_posts_back_off_exponentially_and_429_uses_retry_after() {
    let failure = super::client::SlackError::Transport("connection reset".to_owned());
    let delays = (1..=7)
        .map(|failures| super::delivery_retry_delay(&failure, failures).as_secs())
        .collect::<Vec<_>>();
    assert_eq!(delays, [30, 60, 120, 240, 480, 600, 600]);
    assert_eq!(
        super::delivery_retry_delay(
            &super::client::SlackError::RateLimited(Duration::from_secs(7)),
            5
        ),
        Duration::from_secs(7)
    );
}

#[tokio::test]
async fn a_failed_post_is_not_retried_on_the_next_ticks() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script(
        "chat.postMessage",
        MockReply::Json(json!({ "ok": false, "error": "internal_error" })),
    );
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    for _ in 0..3 {
        notifier.tick().await;
    }
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    assert_eq!(notifier.status().status, SlackStatusKind::Error);
}

#[tokio::test]
async fn a_permanent_post_error_reconnects_instead_of_reposting() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script(
        "chat.postMessage",
        MockReply::Json(json!({ "ok": false, "error": "channel_not_found" })),
    );
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    let status = notifier.status();
    assert_eq!(status.status, SlackStatusKind::Misconfigured, "{status:?}");
    assert!(!status.restart_required);
    assert!(status.last_error.unwrap().contains("channel_not_found"));
    for _ in 0..3 {
        notifier.tick().await;
    }
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    assert_eq!(slack.calls("auth.test").len(), 1);
    // The owner fixed it and pressed "Send test message": reconnects now.
    let status = notifier.send_test().await.unwrap();
    assert_eq!(status.status, SlackStatusKind::Connected);
    assert_eq!(slack.calls("auth.test").len(), 2);
    assert_eq!(slack.calls("conversations.open").len(), 2);
}

#[tokio::test]
async fn connect_failures_back_off_instead_of_rerunning_aws_every_tick() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let failing = temp.path().join("failing-aws");
    std::fs::write(
        &failing,
        format!(
            "#!/bin/sh\necho run >> '{}'\necho AccessDenied >&2\nexit 255\n",
            temp.path().join("runs").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o700)).unwrap();
    let notifier = SlackNotifier::with_parts(
        SlackConfig::Enabled(settings(None)),
        store,
        NotifierParts {
            api: SlackApi::with_base_url(slack.base_url.clone()),
            aws_binary: failing.into(),
            thread_file: None,
            timing: instant_timing(),
            min_interval: Duration::ZERO,
            inbound: super::hub::InboundParts::default(),
        },
    );
    let connecting = notifier.status();
    assert_eq!(connecting.status, SlackStatusKind::Connecting);
    assert_eq!(connecting.last_error, None);
    assert!(!connecting.restart_required);
    for _ in 0..4 {
        notifier.tick().await;
    }
    assert_eq!(aws_runs(temp.path()), 1);
    assert_eq!(notifier.status().status, SlackStatusKind::Error);
    assert!(slack.calls("auth.test").is_empty());
}

#[tokio::test]
async fn viewing_the_agent_before_a_queued_send_suppresses_it() {
    let temp = TempDir::new().unwrap();
    let (store, project_id) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script("chat.postMessage", MockReply::RateLimited(1));
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await; // settled, posted, 429: queued for 1 s
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    notifier.presence().touch(
        super::presence::ViewTarget::ProjectOrchestrator(project_id),
        std::time::Instant::now(),
    );
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    notifier.tick().await;
    notifier.tick().await;
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
}

#[tokio::test]
async fn a_deleted_thread_root_starts_a_new_thread() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    slack.script(
        "chat.postMessage",
        MockReply::Json(json!({ "ok": false, "error": "thread_not_found" })),
    );
    set_orchestrator_status(&temp, "working", 3);
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 4);
    notifier.tick().await;
    notifier.tick().await;
    let posts = slack.calls("chat.postMessage");
    assert_eq!(posts.len(), 3, "{posts:?}");
    assert_eq!(posts[1].body["thread_ts"], "1700000000.000001");
    assert!(posts[2].body.get("thread_ts").is_none());
    let threads = std::fs::read_to_string(temp.path().join("slack-threads.json")).unwrap();
    assert!(threads.contains("1700000000.000002"), "{threads}");
    assert!(!threads.contains("1700000000.000001"), "{threads}");
    assert_eq!(notifier.status().status, SlackStatusKind::Connected);
}

#[derive(Clone, Default)]
pub(crate) struct Captured(pub(crate) Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

static LOGS: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();

/// Capture every `yard_server` log line of this test process. A process-wide
/// subscriber, installed before any notifier callsite runs (every Slack test
/// helper calls this first), avoids tracing's per-callsite interest cache
/// racing a thread-scoped subscriber in parallel tests.
pub(crate) fn capture_logs() -> Captured {
    LOGS.get_or_init(|| {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new("yard_server=trace"))
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other test installs a global subscriber");
        captured
    })
    .clone()
}

#[tokio::test]
async fn the_token_never_reaches_logs_status_or_errors() {
    let captured = capture_logs();

    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    slack.script(
        "chat.postMessage",
        MockReply::Json(json!({ "ok": false, "error": "invalid_auth" })),
    );
    let notifier = notifier(&temp, store, &slack, SlackConfig::Enabled(settings(None)));
    notifier.tick().await;
    set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    // The secret now fails with the token echoed on stderr.
    let failing = temp.path().join("aws");
    std::fs::write(
        &failing,
        format!("#!/bin/sh\necho 'AccessDenied for {TOKEN}' >&2\nexit 255\n"),
    )
    .unwrap();
    let error = notifier.send_test().await.unwrap_err();
    let status = serde_json::to_string(&notifier.status()).unwrap();

    // The buffer holds every Slack test's logs; none may carry a token.
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("Slack notifier connected"),
        "expected notifier logs: {logs}"
    );
    assert!(
        logs.contains("invalid_auth"),
        "expected the rejection to be logged: {logs}"
    );
    assert!(!logs.contains("ROTATEDTOKENVALUE"));
    for haystack in [logs.as_str(), status.as_str(), &error.to_string()] {
        assert!(
            !haystack.contains("SECRETTOKENVALUE"),
            "token leaked: {haystack}"
        );
    }
    let database = std::fs::read(temp.path().join("yard.sqlite3")).unwrap();
    assert!(
        !database
            .windows(TOKEN.len())
            .any(|window| window == TOKEN.as_bytes())
    );
}

#[tokio::test]
async fn off_and_misconfigured_notifiers_never_send_and_never_return() {
    let temp = TempDir::new().unwrap();
    let (store, _) = store_with_project(&temp).await;
    let slack = MockSlack::start().await;
    let off = SlackNotifier::off();
    assert_eq!(off.status().status, SlackStatusKind::Off);
    assert!(!off.status().enabled);
    assert_eq!(off.send_test().await.unwrap_err(), SlackTestError::Off);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), off.run())
            .await
            .is_err()
    );

    let misconfigured = notifier(
        &temp,
        store.clone(),
        &slack,
        SlackConfig::Misconfigured("YARD_SLACK_OWNER_USER_ID is required".to_owned()),
    );
    let status = misconfigured.status();
    assert_eq!(status.status, SlackStatusKind::Misconfigured);
    assert!(status.enabled);
    assert!(matches!(
        misconfigured.send_test().await.unwrap_err(),
        SlackTestError::Misconfigured(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), misconfigured.run())
            .await
            .is_err()
    );

    // Enabled but the secret cannot be read: the loop keeps running.
    let broken = SlackNotifier::with_parts(
        SlackConfig::Enabled(settings(None)),
        store,
        NotifierParts {
            api: SlackApi::with_base_url(slack.base_url.clone()),
            aws_binary: temp.path().join("missing-aws").into(),
            thread_file: None,
            timing: instant_timing(),
            min_interval: Duration::ZERO,
            inbound: super::hub::InboundParts::default(),
        },
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), broken.clone().run())
            .await
            .is_err()
    );
    let status = broken.status();
    assert_eq!(status.status, SlackStatusKind::Error);
    assert!(status.last_error.unwrap().contains("AWS CLI"));
    assert!(slack.calls("auth.test").is_empty());
}

#[test]
fn the_checked_in_manifests_request_only_dm_scopes() {
    let inbound: Value =
        serde_json::from_str(include_str!("../../../../docs/slack/manifest.json")).unwrap();
    let outbound: Value = serde_json::from_str(include_str!(
        "../../../../docs/slack/manifest-outbound-only.json"
    ))
    .unwrap();
    // Phase 2: DM read/write for the bot, Socket Mode, one event, buttons.
    assert_eq!(
        inbound["oauth_config"]["scopes"]["bot"],
        json!(["chat:write", "im:write", "im:history"])
    );
    assert_eq!(inbound["settings"]["socket_mode_enabled"], true);
    assert_eq!(inbound["settings"]["interactivity"]["is_enabled"], true);
    assert_eq!(
        inbound["settings"]["event_subscriptions"]["bot_events"],
        json!(["message.im"])
    );
    assert_eq!(
        inbound["features"]["app_home"]["messages_tab_enabled"],
        true
    );
    assert_eq!(
        inbound["features"]["app_home"]["messages_tab_read_only_enabled"],
        false
    );
    // Phase 1 only: post and open the DM; nothing inbound.
    assert_eq!(
        outbound["oauth_config"]["scopes"]["bot"],
        json!(["chat:write", "im:write"])
    );
    assert_eq!(outbound["settings"]["socket_mode_enabled"], false);
    for manifest in [&inbound, &outbound] {
        let body = manifest.to_string();
        // No user-token scopes, public endpoints, slash commands or
        // channel access.
        assert!(manifest["oauth_config"]["scopes"].get("user").is_none());
        for forbidden in [
            "request_url",
            "redirect_urls",
            "slash_commands",
            "channels:",
            "groups:",
            "mpim:",
            "users:read",
            "assistant:write",
            "org_deploy_enabled\":true",
        ] {
            assert!(!body.contains(forbidden), "{forbidden}");
        }
        assert_eq!(
            manifest["display_information"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["description", "name"],
            "minimal display information"
        );
    }
}

#[cfg(debug_assertions)]
#[test]
fn the_rehearsal_endpoint_accepts_only_loopback() {
    use super::loopback_endpoint;
    assert_eq!(
        loopback_endpoint("http://127.0.0.1:8787").as_deref(),
        Some("http://127.0.0.1:8787/api/")
    );
    assert_eq!(
        loopback_endpoint("http://127.0.0.1:8787/").as_deref(),
        Some("http://127.0.0.1:8787/api/")
    );
    for refused in [
        "https://slack.com/api/",
        "http://localhost:8787",
        "http://127.0.0.1.evil.test:8787",
        "http://127.0.0.1:8787@evil.test",
        "http://127.0.0.1:8787/api/",
        "http://127.0.0.1:+80",
        "http://127.0.0.1:0",
        "http://127.0.0.1:",
        "http://10.0.0.1:8787",
        "https://127.0.0.1:8787",
    ] {
        assert_eq!(loopback_endpoint(refused), None, "{refused}");
    }
}

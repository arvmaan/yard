//! Inbound end-to-end tests: a local Socket Mode WebSocket server and the
//! mock Web API from [`super::tests`], a fake `aws` that serves both
//! secrets, and a scripted agent console. Nothing reaches Slack or AWS.

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use super::{
    NotifierParts, SlackNotifier,
    actions::{AgentConsole, AgentSnapshot, AgentTarget, ConsoleError, TerminalIdentity},
    client::SlackApi,
    hub::{InboundParts, InboundStatusKind},
    inbound::tests::{click, message, with},
    prompt::{KEY_DOWN, KEY_ENTER, tests::CLAUDE_PERMISSION},
    socket::UrlPolicy,
    tests::{MockSlack, TOKEN, capture_logs, settings, store_with_project},
};
use crate::config::{SlackConfig, SlackInbound};

pub(crate) const APP_TOKEN: &str = concat!("xa", "pp-1-A01APP-222-SECRETAPPTOKEN");

/// Frames the mock socket received from Yard, with arrival times.
#[derive(Clone, Default)]
struct Received(Arc<Mutex<Vec<(Value, Instant)>>>);

/// A Socket Mode server: every connection gets `hello`, then whatever the
/// test pushes; it records what Yard sends back.
struct MockSocket {
    url: String,
    push: mpsc::UnboundedSender<Option<Value>>,
    received: Received,
    connections: Arc<Mutex<usize>>,
    /// Close frames Yard sent: `(code, reason)`.
    closes: Arc<Mutex<Vec<(u16, String)>>>,
}

impl MockSocket {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "ws://{}/link/?ticket=one-time-ticket",
            listener.local_addr().unwrap()
        );
        let (push, pushed) = mpsc::unbounded_channel::<Option<Value>>();
        let pushed = Arc::new(tokio::sync::Mutex::new(pushed));
        let received = Received::default();
        let connections = Arc::new(Mutex::new(0));
        let closes = Arc::new(Mutex::new(Vec::new()));
        let (frames, count, close_log) = (
            received.clone(),
            Arc::clone(&connections),
            Arc::clone(&closes),
        );
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let Ok(mut socket) = accept_async(stream).await else {
                    continue;
                };
                *count.lock().unwrap() += 1;
                let hello = json!({
                    "type": "hello", "num_connections": 1,
                    "connection_info": { "app_id": "A01APP" }
                });
                socket
                    .send(Message::Text(hello.to_string().into()))
                    .await
                    .unwrap();
                let mut pushed = pushed.lock().await;
                loop {
                    tokio::select! {
                        next = pushed.recv() => match next {
                            Some(Some(frame)) => {
                                if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
                                    break;
                                }
                            }
                            // `None` → drop this connection abruptly.
                            Some(None) | None => break,
                        },
                        frame = socket.next() => match frame {
                            Some(Ok(Message::Text(text))) => frames
                                .0
                                .lock()
                                .unwrap()
                                .push((serde_json::from_str(text.as_str()).unwrap(), Instant::now())),
                            Some(Ok(Message::Close(frame))) => {
                                close_log.lock().unwrap().push(frame.map_or((0, String::new()), |frame| {
                                    (u16::from(frame.code), frame.reason.to_string())
                                }));
                            }
                            Some(Ok(_)) => {}
                            _ => break,
                        },
                    }
                }
            }
        });
        Self {
            url,
            push,
            received,
            connections,
            closes,
        }
    }

    fn send(&self, frame: Value) -> Instant {
        self.push.send(Some(frame)).unwrap();
        Instant::now()
    }

    fn drop_connection(&self) {
        self.push.send(None).unwrap();
    }

    fn acks(&self) -> Vec<(String, Instant)> {
        self.received
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(frame, at)| Some((frame.get("envelope_id")?.as_str()?.to_owned(), *at)))
            .collect()
    }

    fn connections(&self) -> usize {
        *self.connections.lock().unwrap()
    }
}

/// A fake `aws` serving the bot secret and (for `yard/slack-app`) the app
/// secret; `app_exit` non-zero makes the app secret fail.
fn fake_aws_both(dir: &Path, app_exit: i32) -> PathBuf {
    let script = dir.join("aws");
    let mut file = std::fs::File::create(&script).unwrap();
    writeln!(
        file,
        "#!/bin/sh\ncase \"$*\" in\n  *yard/slack-app*) echo app >> '{runs}'; [ {app_exit} -eq 0 ] || {{ echo 'AccessDenied {APP_TOKEN}' >&2; exit {app_exit}; }}; printf '{{\"app_token\":\"{APP_TOKEN}\"}}\\n' ;;\n  *) printf '{{\"bot_token\":\"{TOKEN}\"}}\\n' ;;\nesac",
        runs = dir.join("app-runs").display(),
    )
    .unwrap();
    drop(file);
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn inbound_settings(inbound: SlackInbound) -> SlackConfig {
    let mut settings = settings(Some("E01SANDBOX0"));
    settings.inbound = inbound;
    SlackConfig::Enabled(settings)
}

fn enabled() -> SlackInbound {
    SlackInbound::Enabled {
        app_secret_id: "yard/slack-app".to_owned(),
    }
}

async fn inbound_notifier(
    temp: &TempDir,
    slack: &MockSlack,
    config: SlackConfig,
    app_exit: i32,
) -> SlackNotifier {
    inbound_notifier_timed(
        temp,
        slack,
        config,
        app_exit,
        super::detector::DetectorTiming::default(),
    )
    .await
}

async fn inbound_notifier_timed(
    temp: &TempDir,
    slack: &MockSlack,
    config: SlackConfig,
    app_exit: i32,
    timing: super::detector::DetectorTiming,
) -> SlackNotifier {
    inbound_notifier_relay(
        temp,
        slack,
        config,
        app_exit,
        timing,
        fast_relay(),
        super::policy::QuietParts::always(),
    )
    .await
}

/// Watch timings for tests (Slack posts are still paced 1/s per channel).
fn fast_relay() -> super::relay::RelayTiming {
    super::relay::RelayTiming {
        poll: Duration::from_millis(50),
        wait: Duration::from_millis(400),
        follow_poll: Duration::from_millis(100),
        follow: Duration::from_millis(900),
        progress: Duration::from_millis(150),
        late_poll: Duration::from_millis(100),
        late: Duration::from_millis(1_500),
    }
}

async fn inbound_notifier_relay(
    temp: &TempDir,
    slack: &MockSlack,
    config: SlackConfig,
    app_exit: i32,
    timing: super::detector::DetectorTiming,
    relay: super::relay::RelayTiming,
    quiet: super::policy::QuietParts,
) -> SlackNotifier {
    capture_logs();
    let (store, _) = store_with_project(temp).await;
    SlackNotifier::with_parts(
        config,
        store,
        NotifierParts {
            api: SlackApi::with_base_url(slack.base_url.clone()),
            aws_binary: fake_aws_both(temp.path(), app_exit).into(),
            thread_file: Some(temp.path().join("slack-threads.json")),
            timing,
            min_interval: Duration::ZERO,
            inbound: InboundParts {
                audit_file: Some(temp.path().join("slack-audit.jsonl")),
                url_policy: UrlPolicy::Loopback,
                idle_timeout: Duration::from_secs(30),
                relay,
                settle: Duration::from_millis(250),
                ui_url: "https://yard.example.test/".to_owned(),
            },
            quiet,
        },
    )
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The scripted console, with every `snapshot` waiting for a permit.
struct GatedConsole {
    inner: super::actions::tests::FakeConsole,
    gate: Semaphore,
}

impl GatedConsole {
    fn new(screen: &str, permits: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: super::actions::tests::FakeConsole::blocked_on(screen),
            gate: Semaphore::new(permits),
        })
    }

    fn typed(&self) -> Vec<Vec<&'static str>> {
        self.inner.typed.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentConsole for GatedConsole {
    async fn snapshot(&self, target: &AgentTarget) -> Result<AgentSnapshot, ConsoleError> {
        self.gate.acquire().await.unwrap().forget();
        self.inner.snapshot(target).await
    }

    async fn sendable(&self, target: &AgentTarget) -> Result<(), ConsoleError> {
        self.inner.sendable(target).await
    }

    async fn screen(&self, target: &AgentTarget) -> Result<String, ConsoleError> {
        self.inner.screen(target).await
    }

    async fn type_keys(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        plan: &super::actions::KeyPlan,
    ) -> Result<(), ConsoleError> {
        self.inner.type_keys(target, identity, plan).await
    }

    async fn send_prompt(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        text: &str,
        actor: &str,
    ) -> Result<String, ConsoleError> {
        self.inner.send_prompt(target, identity, text, actor).await
    }

    async fn status_report(
        &self,
        target: &AgentTarget,
        command_id: &str,
    ) -> Result<Option<yard_domain::OrchestratorStatusReport>, ConsoleError> {
        self.inner.status_report(target, command_id).await
    }

    async fn watch(
        &self,
        target: &AgentTarget,
        command_id: &str,
    ) -> Result<super::relay::Watched, ConsoleError> {
        self.inner.watch(target, command_id).await
    }
}

fn posted_texts(slack: &MockSlack) -> Vec<String> {
    slack
        .calls("chat.postMessage")
        .iter()
        .map(|call| call.body["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn audit_lines(temp: &TempDir) -> Vec<Value> {
    // Owner actions and drops live in separate files; merged by time.
    let mut lines = ["slack-audit.jsonl", "slack-audit-dropped.jsonl"]
        .iter()
        .flat_map(|name| {
            std::fs::read_to_string(temp.path().join(name))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    lines.sort_by_key(|line| line["at_unix_ms"].as_u64());
    lines
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn acks_every_envelope_before_work_and_answers_a_click() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    let console = GatedConsole::new(CLAUDE_PERMISSION, 1);
    notifier.attach_console(console.clone());
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    let target = super::actions::tests::target();
    notifier
        .post_prompt_card(&target, "Worker on Fix login", None)
        .await
        .unwrap();
    let card = slack.calls("chat.postMessage").pop().unwrap();
    let buttons = card.body["blocks"][2]["elements"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(buttons.len(), 2, "Allow once + Deny only: {buttons:?}");
    assert_eq!(buttons[0]["text"]["text"], "Allow once");
    let deny = buttons[1]["value"].as_str().unwrap().to_owned();
    let allow = buttons[0]["value"].as_str().unwrap().to_owned();
    assert!(
        !card.body.to_string().contains("abc123"),
        "command redacted"
    );

    // The ack goes out while the answer is still blocked on the console.
    let at = now_s();
    let sent = socket.send(card_click("env-1", &deny, at));
    eventually("ack", || socket.acks().iter().any(|(id, _)| id == "env-1")).await;
    let acked = socket
        .acks()
        .into_iter()
        .find(|(id, _)| id == "env-1")
        .unwrap()
        .1;
    assert!(acked.duration_since(sent) < Duration::from_secs(1));
    assert!(console.typed().is_empty(), "no work before the ack");
    console.gate.add_permits(100);
    eventually("keys", || console.typed().len() == 1).await;
    assert_eq!(console.typed(), [vec![KEY_DOWN, KEY_DOWN, KEY_ENTER]]);
    eventually("reply", || {
        posted_texts(&slack)
            .iter()
            .any(|text| text.starts_with("Sent to"))
    })
    .await;
    let update = slack.calls("chat.update").pop().expect("card collapsed");
    assert_eq!(update.body["ts"], CARD_TS);
    assert!(
        update
            .body
            .get("blocks")
            .is_some_and(|blocks| !blocks.to_string().contains(&deny))
    );

    // A Slack retry of the same click, the sibling button, a stranger.
    socket.send(card_click("env-2", &deny, at));
    socket.send(card_click("env-3", &allow, now_s()));
    socket.send(with(
        card_click("env-4", &allow, now_s()),
        "/payload/user/id",
        json!("U0STRANGER"),
    ));
    eventually("acks", || socket.acks().len() >= 4).await;
    eventually("used reply", || {
        posted_texts(&slack)
            .iter()
            .any(|text| text.contains("already answered"))
    })
    .await;
    eventually("stranger dropped", || {
        audit_lines(&temp)
            .iter()
            .any(|line| line["action"] == "not_the_owner")
    })
    .await;
    assert_eq!(console.typed().len(), 1, "nothing typed twice");
    let audit = audit_lines(&temp);
    let outcomes = audit
        .iter()
        .map(|line| {
            (
                line["kind"].as_str().unwrap(),
                line["outcome"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(outcomes.contains(&("button", "sent")), "{outcomes:?}");
    assert!(
        outcomes.contains(&("button", "refused:already_used")),
        "{outcomes:?}"
    );
    assert!(
        audit
            .iter()
            .any(|line| line["action"] == "not_the_owner" && line["slack_user"] == "U0STRANGER"),
        "{audit:?}"
    );

    // Tokens: each in its own header; the app token nowhere else.
    for call in slack.calls("apps.connections.open") {
        assert_eq!(call.authorization, format!("Bearer {APP_TOKEN}"));
    }
    for call in slack.calls("chat.postMessage") {
        assert_eq!(call.authorization, format!("Bearer {TOKEN}"));
    }
    let logs = String::from_utf8(capture_logs().0.lock().unwrap().clone()).unwrap();
    for haystack in [
        logs.as_str(),
        &std::fs::read_to_string(temp.path().join("slack-audit.jsonl")).unwrap(),
        &std::fs::read_to_string(temp.path().join("slack-audit-dropped.jsonl")).unwrap_or_default(),
        &serde_json::to_string(&notifier.status()).unwrap(),
    ] {
        assert!(!haystack.contains(APP_TOKEN) && !haystack.contains("one-time-ticket"));
    }
}

/// The mock numbers posts; the card is the first post of the test.
const CARD_TS: &str = "1700000000.000001";

/// A click on a button of the card posted top level at [`CARD_TS`].
fn card_click(envelope: &str, action: &str, at: u64) -> Value {
    let value = with(
        click(envelope, action, at),
        "/payload/container/message_ts",
        json!(CARD_TS),
    );
    let value = with(value, "/payload/message/ts", json!(CARD_TS));
    with(value, "/payload/message/thread_ts", Value::Null)
}

fn app_secret_reads(temp: &TempDir) -> usize {
    std::fs::read_to_string(temp.path().join("app-runs")).map_or(0, |runs| runs.lines().count())
}

#[tokio::test]
async fn refreshes_on_disconnect_and_backs_off_after_a_dropped_socket() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    notifier.attach_console(GatedConsole::new(CLAUDE_PERMISSION, 100));
    tokio::spawn(notifier.clone().run_inbound());
    eventually("first connection", || socket.connections() == 1).await;
    let refresh = socket.send(json!({ "type": "disconnect", "reason": "refresh_requested" }));
    eventually("refreshed connection", || socket.connections() == 2).await;
    assert!(
        refresh.elapsed() < Duration::from_millis(1_500),
        "refresh is immediate"
    );
    assert_eq!(
        slack.calls("apps.connections.open").len(),
        2,
        "a new URL per connection"
    );
    let dropped = Instant::now();
    socket.drop_connection();
    eventually("reconnected", || socket.connections() == 3).await;
    assert!(
        dropped.elapsed() >= Duration::from_millis(1_500),
        "a lost socket backs off"
    );
    assert_eq!(app_secret_reads(&temp), 1, "the app token stays in memory");
    // Envelopes on the new connection are still acked.
    socket.send(message("env-9", "Ev9", "status", now_s()));
    eventually("ack after reconnect", || {
        socket.acks().iter().any(|(id, _)| id == "env-9")
    })
    .await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn stale_foreign_and_bot_events_are_acked_but_ignored() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    let console = GatedConsole::new(super::prompt::tests::CLAUDE_QUESTION, 100);
    notifier.attach_console(console.clone());
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    let now = now_s();
    socket.send(with(
        message("env-2", "Ev2", "hi", now),
        "/payload/team_id",
        json!("T0OTHER"),
    ));
    socket.send(with(
        message("env-3", "Ev3", "hi", now),
        "/payload/event/user",
        json!("UBOT"),
    ));
    socket.send(with(
        message("env-4", "Ev4", "hi", now),
        "/payload/event/channel",
        json!("C0PUBLIC"),
    ));
    socket.send(with(
        message("env-5", "Ev5", "hi", now),
        "/payload/event/bot_id",
        json!("B01BOT"),
    ));
    eventually("acks", || socket.acks().len() == 4).await;
    eventually("audit", || audit_lines(&temp).len() == 4).await;
    let reasons = audit_lines(&temp)
        .iter()
        .map(|line| line["action"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [
            "foreign_team",
            "own_echo",
            "not_the_owner_dm",
            "bot_message"
        ]
    );
    assert!(
        slack.calls("chat.postMessage").is_empty(),
        "nothing is answered"
    );

    // A question card, then the owner's reply in its thread reaches the
    // agent through the prompt path, marked as from Slack.
    let target = super::actions::tests::target();
    notifier
        .post_prompt_card(&target, "Worker", None)
        .await
        .unwrap();
    let reply = with(
        message("env-6", "Ev6", "yes, delete it", now_s()),
        "/payload/event/thread_ts",
        json!(CARD_TS),
    );
    socket.send(reply);
    eventually("prompt", || {
        !console.inner.prompts.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        console.inner.prompts.lock().unwrap()[0],
        (
            format!("{} yes, delete it", super::actions::SLACK_PROVENANCE),
            "slack:owner:U01OWNER".to_owned()
        )
    );
    assert!(
        console.typed().is_empty(),
        "free text never becomes keystrokes"
    );
    // A second message in the answered question's thread goes nowhere.
    socket.send(with(
        message("env-7", "Ev7", "and also this", now_s()),
        "/payload/event/thread_ts",
        json!(CARD_TS),
    ));
    eventually("closed reply", || {
        posted_texts(&slack)
            .iter()
            .any(|text| text.starts_with("That question was already answered"))
    })
    .await;
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1, "not sent");
    assert_eq!(console.inner.prompt_targets.lock().unwrap().len(), 1);

    // The owner's message that Slack delivered too late: dropped, and one
    // notice in its thread, even when Slack retries it.
    socket.send(message("env-8", "Ev1", "blocked", now - 400));
    socket.send(message("env-9", "Ev1", "blocked", now - 400));
    let late_thread = format!("{}.000100", now - 400);
    eventually("late notice", || {
        slack
            .calls("chat.postMessage")
            .iter()
            .any(|call| call.body["thread_ts"] == late_thread.as_str())
    })
    .await;
    eventually("both acked", || socket.acks().len() >= 8).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let late = slack
        .calls("chat.postMessage")
        .into_iter()
        .filter(|call| call.body["thread_ts"] == late_thread.as_str())
        .map(|call| call.body)
        .collect::<Vec<_>>();
    assert_eq!(texts(&late).len(), 1, "{late:?}");
    assert!(texts(&late)[0].contains("nothing was done"), "{late:?}");
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1, "not sent");
    assert!(
        audit_lines(&temp)
            .iter()
            .any(|line| line["action"] == "stale")
    );
}

#[tokio::test]
async fn inbound_off_misconfigured_or_without_its_secret_never_opens_a_socket() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    for (config, expected) in [
        (inbound_settings(SlackInbound::Off), InboundStatusKind::Off),
        (
            inbound_settings(SlackInbound::Misconfigured(
                "YARD_SLACK_APP_SECRET_ID is required".to_owned(),
            )),
            InboundStatusKind::Misconfigured,
        ),
        (SlackConfig::Off, InboundStatusKind::Off),
    ] {
        let temp = TempDir::new().unwrap();
        let notifier = inbound_notifier(&temp, &slack, config, 0).await;
        assert_eq!(notifier.inbound_status().status, expected);
        let run =
            tokio::time::timeout(Duration::from_millis(300), notifier.clone().run_inbound()).await;
        assert!(run.is_err(), "run_inbound never returns");
    }
    assert!(slack.calls("apps.connections.open").is_empty());
    assert_eq!(socket.connections(), 0);

    // The app secret can't be read: inbound reports it, nothing connects,
    // and DM notifications still work.
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 255).await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("secret failure", || {
        notifier.inbound_status().status == InboundStatusKind::Error
    })
    .await;
    let error = notifier.inbound_status().last_error.unwrap();
    assert!(error.contains("app-level token"), "{error}");
    assert!(!error.contains(APP_TOKEN), "stderr is redacted: {error}");
    assert!(slack.calls("apps.connections.open").is_empty());
    assert_eq!(socket.connections(), 0);
    notifier.send_test().await.expect("outbound is unaffected");
}

#[tokio::test]
async fn a_rejected_app_token_is_forgotten_and_read_again() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.script(
        "apps.connections.open",
        super::tests::MockReply::Json(json!({ "ok": false, "error": "invalid_auth" })),
    );
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("connected after re-reading", || socket.connections() == 1).await;
    assert_eq!(app_secret_reads(&temp), 2);
    assert_eq!(slack.calls("apps.connections.open").len(), 2);
}

/// Posts in the thread of the owner message sent at `at_s`.
fn thread_posts(slack: &MockSlack, at_s: u64) -> Vec<Value> {
    let root = format!("{at_s}.000100");
    slack
        .calls("chat.postMessage")
        .into_iter()
        .filter(|call| call.body["thread_ts"] == root.as_str())
        .map(|call| call.body)
        .collect()
}

async fn connected(temp: &TempDir, slack: &MockSlack, socket: &MockSocket) -> SlackNotifier {
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(temp, slack, inbound_settings(enabled()), 0).await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    notifier
}

/// [`connected`] with its own watch timings.
async fn connected_relay(
    temp: &TempDir,
    slack: &MockSlack,
    socket: &MockSocket,
    relay: super::relay::RelayTiming,
) -> SlackNotifier {
    slack.set_socket_url(&socket.url);
    let config = inbound_settings(enabled());
    let timing = super::detector::DetectorTiming::default();
    let notifier = inbound_notifier_relay(
        temp,
        slack,
        config,
        0,
        timing,
        relay,
        super::policy::QuietParts::always(),
    )
    .await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    notifier
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn status_commands_answer_from_durable_state_without_touching_agents() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    let console = GatedConsole::new(CLAUDE_PERMISSION, 100);
    notifier.attach_console(console.clone());

    let at = now_s();
    socket.send(message("env-1", "Ev1", "status", at));
    eventually("status", || thread_posts(&slack, at).len() == 1).await;
    let status = thread_posts(&slack, at)[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(status.starts_with("*Yard status*"), "{status}");
    assert!(
        status.contains("• *Checkout &lt;!channel&gt;* — orchestrator working · no workers"),
        "{status}"
    );
    assert!(status.contains("Superintendent: not running"), "{status}");

    let at = at + 1;
    socket.send(message("env-2", "Ev2", "Status checkout", at));
    eventually("project status", || thread_posts(&slack, at).len() == 1).await;
    let project = thread_posts(&slack, at)[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        project.starts_with("*Checkout &lt;!channel&gt;*\n• Orchestrator: working\n• No workers."),
        "{project}"
    );

    let at = at + 1;
    socket.send(message("env-3", "Ev3", "help", at));
    socket.send(message("env-4", "Ev4", "review", at + 1));
    eventually("help", || thread_posts(&slack, at).len() == 1).await;
    eventually("review", || thread_posts(&slack, at + 1).len() == 1).await;
    assert!(
        thread_posts(&slack, at)[0]["text"]
            .as_str()
            .unwrap()
            .contains("`status <project>`")
    );
    assert_eq!(
        thread_posts(&slack, at + 1)[0]["text"],
        "Nothing is waiting for review."
    );

    // Nothing blocked, then the orchestrator blocks: `blocked` posts its
    // card with one-time buttons in the command's thread.
    let at = at + 2;
    socket.send(message("env-5", "Ev5", "blocked", at));
    eventually("nothing blocked", || thread_posts(&slack, at).len() == 1).await;
    assert_eq!(thread_posts(&slack, at)[0]["text"], "Nothing is blocked.");
    super::tests::set_orchestrator_status(&temp, "blocked", 2);
    let at = at + 1;
    socket.send(message("env-6", "Ev6", "blocked", at));
    eventually("blocked card", || thread_posts(&slack, at).len() == 2).await;
    let posts = thread_posts(&slack, at);
    assert_eq!(posts[0]["text"], "1 agent is blocked:");
    let card = posts[1]["blocks"].to_string();
    // Plain text in the header (Slack parses no mentions there); escaped
    // everywhere mrkdwn is parsed.
    assert_eq!(
        posts[1]["blocks"][0]["text"]["text"],
        ":raised_hand: Checkout <!channel> · The project orchestrator"
    );
    assert_eq!(posts[1]["blocks"][0]["text"]["type"], "plain_text");
    assert!(
        posts[1]["text"]
            .as_str()
            .unwrap()
            .contains("Checkout &lt;!channel&gt; · The project orchestrator"),
        "{card}"
    );
    let buttons = posts[1]["blocks"][2]["elements"].as_array().unwrap();
    assert_eq!(buttons.len(), 2, "Allow once + Deny: {buttons:?}");
    let labels = buttons
        .iter()
        .map(|button| button["text"]["text"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(labels, ["Allow once", "Deny"], "never an always-allow");

    assert!(console.typed().is_empty(), "status commands never type");
    assert!(
        console.inner.prompts.lock().unwrap().is_empty(),
        "status commands never prompt"
    );
    let audit = audit_lines(&temp);
    let actions = audit
        .iter()
        .map(|line| {
            format!(
                "{}:{}:{}",
                line["kind"].as_str().unwrap(),
                line["action"].as_str().unwrap(),
                line["outcome"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        "message:status:answered",
        "message:status_project:answered",
        "message:help:answered",
        "message:review:answered",
        "message:blocked:answered:0",
        "message:blocked:answered:1",
    ] {
        assert!(actions.contains(&expected.to_owned()), "{actions:?}");
    }
    assert!(audit.iter().all(|line| line["slack_user"] == "U01OWNER"));
}

fn set_blocked(console: &GatedConsole, blocked: bool) {
    *console.inner.snapshot.lock().unwrap() = Ok(AgentSnapshot {
        identity: super::actions::tests::identity(),
        blocked,
        health: super::actions::AgentHealth::Working,
    });
}

fn texts(posts: &[Value]) -> Vec<String> {
    posts
        .iter()
        .map(|post| post["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn questions_go_to_the_named_orchestrator_and_the_answer_is_relayed() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    let console = GatedConsole::new(super::prompt::tests::WORKING, 10_000);
    set_blocked(&console, false);
    notifier.attach_console(console.clone());
    let project_id: String = rusqlite::Connection::open(temp.path().join("yard.sqlite3"))
        .unwrap()
        .query_row("SELECT id FROM projects", [], |row| row.get(0))
        .unwrap();

    // `<Project>: …` goes to that project's orchestrator, marked as from
    // Slack, with the answer instruction. The thread gets an immediate
    // "Asking …" progress message.
    let at = now_s();
    socket.send(message(
        "env-1",
        "Ev1",
        // As Slack delivers typed `<`, `>` and `&`: entity-encoded.
        "checkout &lt;!channel&gt;: what's left? a &amp;&amp; b token=abc123",
        at,
    ));
    eventually("sent", || !console.inner.prompts.lock().unwrap().is_empty()).await;
    assert_eq!(
        console.inner.prompt_targets.lock().unwrap().clone(),
        [AgentTarget::ProjectOrchestrator {
            project_id: project_id.clone()
        }]
    );
    let (text, actor) = console.inner.prompts.lock().unwrap()[0].clone();
    assert_eq!(
        text,
        super::relay::question_text("what's left? a && b token=abc123")
    );
    assert!(text.contains("do not start long investigations or delegations"));
    assert_eq!(actor, "slack:owner:U01OWNER");
    let posts = thread_posts(&slack, at);
    assert_eq!(posts.len(), 1, "{:?}", texts(&posts));
    assert!(
        texts(&posts)[0].starts_with("Asking the Checkout &lt;!channel&gt; orchestrator…"),
        "{:?}",
        texts(&posts)
    );
    assert!(posts[0]["blocks"].to_string().contains(":thinking_face:"));
    // Another command's report and prose are never relayed; ours is, as a
    // formatted answer, and the progress message turns into "Answered".
    *console.inner.screen.lock().unwrap() = format!(
        "{}\nworking on it…",
        super::relay::tests::report_line("command-0", "not this one")
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(thread_posts(&slack, at).len(), 1, "nothing relayed yet");
    *console.inner.screen.lock().unwrap() = format!(
        "⏺ {}\n",
        super::relay::tests::report_line(
            "command-1",
            concat!(
                "**Two** tasks left; key sk-",
                "ant-api03-SECRETSECRETSECRETSECRET"
            )
        )
    );
    eventually("answer", || thread_posts(&slack, at).len() == 2).await;
    let answer = thread_posts(&slack, at)[1].clone();
    let fallback = answer["text"].as_str().unwrap();
    assert!(
        fallback.starts_with(
            "*Answer from the Checkout &lt;!channel&gt; orchestrator* (working)\n**Two** tasks left"
        ),
        "{fallback}"
    );
    assert_eq!(answer["blocks"][0]["type"], "header");
    let body = answer["blocks"][1]["text"]["text"].as_str().unwrap();
    assert!(body.starts_with("*Two* tasks left"), "{body}");
    assert!(
        !answer.to_string().contains("SECRETSECRET"),
        "redacted: {answer}"
    );
    eventually("answered", || {
        slack.calls("chat.update").iter().any(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .contains("answered in")
        })
    })
    .await;
    assert!(
        console
            .inner
            .report_reads
            .lock()
            .unwrap()
            .iter()
            .all(|id| id == "command-1"),
        "only our command's report is looked for"
    );

    // A blocked orchestrator gets no question: its prompt card instead.
    set_blocked(&console, true);
    *console.inner.screen.lock().unwrap() = CLAUDE_PERMISSION.to_owned();
    let at = at + 1;
    socket.send(message("env-3", "Ev3", "are you done?", at));
    eventually("blocked refusal", || thread_posts(&slack, at).len() == 2).await;
    let posts = thread_posts(&slack, at);
    assert_eq!(
        posts[1]["blocks"][2]["elements"].as_array().unwrap().len(),
        2
    );
    let not_sent = |slack: &MockSlack, why: &str| {
        slack.calls("chat.update").into_iter().find(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("was not sent: {why}"))
        })
    };
    // A card like every other ask outcome: Retry and Open in Yard.
    let card = not_sent(&slack, "it is waiting on a prompt").unwrap().body;
    button_value(&card, "Retry");
    assert!(
        card["blocks"]
            .to_string()
            .contains(super::blocks::OPEN_ACTION_ID)
    );
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1, "not sent");

    // A rejected delivery (terminal open in Yard) becomes an error card
    // with Retry and Open in Yard; nothing is retried by itself.
    set_blocked(&console, false);
    *console.inner.prompt_error.lock().unwrap() =
        Some(ConsoleError::Busy("open in Yard".to_owned()));
    let at = at + 1;
    socket.send(message("env-4", "Ev4", "hello?", at));
    let failed = |slack: &MockSlack| {
        slack.calls("chat.update").into_iter().find(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .contains("its terminal is open in Yard; answer it there")
        })
    };
    eventually("busy", || failed(&slack).is_some()).await;
    let card = failed(&slack).unwrap().body;
    button_value(&card, "Retry");
    assert!(
        card["blocks"]
            .to_string()
            .contains(super::blocks::OPEN_ACTION_ID)
    );
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1);

    // Reading the agent fails (not gone): the same card, with Retry.
    *console.inner.prompt_error.lock().unwrap() = None;
    let running = console.inner.snapshot.lock().unwrap().clone();
    *console.inner.snapshot.lock().unwrap() =
        Err(ConsoleError::Failed("herdr timed out".to_owned()));
    let at = at + 1;
    socket.send(message("env-5", "Ev5", "anyone?", at));
    eventually("read failed", || {
        not_sent(&slack, "Yard could not deliver the question").is_some()
    })
    .await;
    let card = not_sent(&slack, "Yard could not deliver the question")
        .unwrap()
        .body;
    button_value(&card, "Retry");
    assert!(
        card["blocks"]
            .to_string()
            .contains(super::blocks::OPEN_ACTION_ID)
    );
    *console.inner.snapshot.lock().unwrap() = running;
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1);

    let outcomes = outcomes(&temp);
    assert_eq!(
        outcomes,
        [
            format!("ask:orchestrator:{project_id}:sent"),
            format!("relay:orchestrator:{project_id}:relayed"),
            "ask:superintendent:refused:blocked".to_owned(),
            "ask:superintendent:refused:busy".to_owned(),
            "ask:superintendent:refused:failed".to_owned(),
        ]
    );
    let audit = std::fs::read_to_string(temp.path().join("slack-audit.jsonl")).unwrap()
        + &std::fs::read_to_string(temp.path().join("slack-audit-dropped.jsonl"))
            .unwrap_or_default();
    assert!(
        !audit.contains("abc123") && !audit.contains("what's left"),
        "no message text"
    );
}

/// `action:target:outcome` of every non-dropped audit line.
fn outcomes(temp: &TempDir) -> Vec<String> {
    audit_lines(temp)
        .iter()
        .filter(|line| line["kind"] != "dropped")
        .map(|line| {
            format!(
                "{}:{}:{}",
                line["action"].as_str().unwrap(),
                line["target"].as_str().unwrap_or("-"),
                line["outcome"].as_str().unwrap()
            )
        })
        .collect()
}

/// An owner click on `value`, on a message in the thread rooted at `at_s`.
/// Each click gets its own fresh `action_ts` (Slack never repeats one;
/// Yard drops a repeated one as a duplicate).
fn thread_click(envelope: &str, value: &str, at_s: u64) -> Value {
    static CLICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let unique = CLICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let value = with(
        click(envelope, value, now_s()),
        "/payload/actions/0/action_ts",
        json!(format!("{}.{unique:06}", now_s())),
    );
    with(
        value,
        "/payload/message/thread_ts",
        json!(format!("{at_s}.000100")),
    )
}

/// The last chat.update whose text contains `needle`.
fn update_with(slack: &MockSlack, needle: &str) -> Option<Value> {
    slack
        .calls("chat.update")
        .into_iter()
        .rev()
        .find(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .contains(needle)
        })
        .map(|call| call.body)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn an_unanswered_question_gets_throttled_progress_a_timeout_check_again_and_late_answers() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let relay = super::relay::RelayTiming {
        follow: Duration::from_secs(4),
        progress: Duration::from_millis(1_200),
        late: Duration::from_millis(2_500),
        ..fast_relay()
    };
    let notifier = connected_relay(&temp, &slack, &socket, relay).await;
    let console = GatedConsole::new(super::prompt::tests::WORKING, 10_000);
    set_blocked(&console, false);
    notifier.attach_console(console.clone());

    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked right now?", at));
    eventually("timed out", || {
        update_with(&slack, "No answer from the Superintendent after").is_some()
    })
    .await;
    assert_eq!(
        console.inner.prompt_targets.lock().unwrap()[0],
        AgentTarget::YardOrchestrator
    );
    // One progress message, updated in place and throttled (the pane was
    // read far more often than the message was edited).
    let posts = thread_posts(&slack, at);
    assert_eq!(posts.len(), 1, "{:?}", texts(&posts));
    let progress = slack
        .calls("chat.update")
        .iter()
        .filter(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Still waiting for the Superintendent")
        })
        .count();
    let reads = console.inner.report_reads.lock().unwrap().len();
    assert!((2..=3).contains(&progress), "{progress} progress updates");
    assert!(reads > progress * 2, "{reads} reads, {progress} updates");
    let timed_out = update_with(&slack, "No answer from the Superintendent after").unwrap();
    assert!(
        timed_out["text"]
            .as_str()
            .unwrap()
            .contains("still posted here"),
        "{timed_out}"
    );

    // Check again before an answer: says so, with a fresh button.
    let check = button_value(&timed_out, "Check again");
    socket.send(thread_click("env-2", &check, at));
    eventually("not yet", || thread_posts(&slack, at).len() == 2).await;
    let not_yet = thread_posts(&slack, at)[1].clone();
    assert!(
        not_yet["text"]
            .as_str()
            .unwrap()
            .contains("has not answered this question yet"),
        "{not_yet}"
    );
    let fresh = button_value(&not_yet, "Check again");
    assert_ne!(fresh, check);
    // The same Check again id is single-use.
    socket.send(thread_click("env-3", &check, at));
    eventually("reused", || thread_posts(&slack, at).len() == 3).await;
    assert!(texts(&thread_posts(&slack, at))[2].contains("already used"));

    // A late answer for the same command id is still relayed, once.
    *console.inner.screen.lock().unwrap() = format!(
        "⏺ {}\n",
        super::relay::tests::report_line("command-1", "Nothing is blocked.")
    );
    eventually("late answer", || thread_posts(&slack, at).len() == 4).await;
    assert!(texts(&thread_posts(&slack, at))[3].contains("Nothing is blocked."));
    eventually("answered", || update_with(&slack, "answered in").is_some()).await;
    socket.send(thread_click("env-4", &fresh, at));
    eventually("already", || thread_posts(&slack, at).len() == 5).await;
    assert!(texts(&thread_posts(&slack, at))[4].contains("already answered"));

    // After the late window, Check again still relays an answer it finds.
    *console.inner.screen.lock().unwrap() = super::prompt::tests::WORKING.to_owned();
    let at2 = at + 1;
    socket.send(message("env-5", "Ev5", "and now?", at2));
    eventually("second timed out", || {
        slack
            .calls("chat.update")
            .iter()
            .filter(|call| {
                call.body["text"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("No answer from the Superintendent")
            })
            .count()
            == 2
    })
    .await;
    let timed_out = update_with(&slack, "No answer from the Superintendent after").unwrap();
    let check = button_value(&timed_out, "Check again");
    tokio::time::sleep(Duration::from_millis(2_700)).await;
    *console.inner.screen.lock().unwrap() = format!(
        "⏺ {}\n",
        super::relay::tests::report_line("command-2", "Still nothing.")
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(thread_posts(&slack, at2).len(), 1, "late window over");
    socket.send(thread_click("env-6", &check, at2));
    eventually("checked", || thread_posts(&slack, at2).len() == 2).await;
    assert!(texts(&thread_posts(&slack, at2))[1].contains("Still nothing."));

    let outcomes = outcomes(&temp);
    assert_eq!(
        outcomes,
        [
            "ask:superintendent:sent",
            "relay:superintendent:timed_out",
            "nav_check_again:superintendent:answered",
            "check_again:superintendent:not_yet",
            "nav:-:refused:already_used",
            "relay:superintendent:relayed_late",
            "nav_check_again:superintendent:answered",
            "check_again:superintendent:already_relayed",
            "ask:superintendent:sent",
            "relay:superintendent:timed_out",
            "nav_check_again:superintendent:answered",
            "relay:superintendent:relayed_on_check",
        ]
    );
}

/// A Slack API error reply for the next `method` call.
fn fail_next(slack: &MockSlack, method: &str) {
    slack.script(
        method,
        super::tests::MockReply::Json(json!({ "ok": false, "error": "internal_error" })),
    );
}

/// A question whose watch is running on a working Superintendent.
async fn asked_working(
    temp: &TempDir,
    slack: &MockSlack,
    socket: &MockSocket,
    relay: super::relay::RelayTiming,
) -> (SlackNotifier, Arc<GatedConsole>) {
    let notifier = connected_relay(temp, slack, socket, relay).await;
    let console = GatedConsole::new(super::prompt::tests::WORKING, 10_000);
    set_blocked(&console, false);
    notifier.attach_console(console.clone());
    (notifier, console)
}

fn answer_screen(command_id: &str, last: &str) -> String {
    format!("⏺ {}\n", super::relay::tests::report_line(command_id, last))
}

#[tokio::test]
async fn a_failed_answer_post_is_retried_and_not_shown_as_answered() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let relay = super::relay::RelayTiming {
        follow: Duration::from_secs(60),
        progress: Duration::from_secs(60),
        ..fast_relay()
    };
    let (_notifier, console) = asked_working(&temp, &slack, &socket, relay).await;
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("asking", || thread_posts(&slack, at).len() == 1).await;
    eventually("sent", || console.inner.prompts.lock().unwrap().len() == 1).await;
    // The answer's chat.postMessage fails once (429, network blip, …).
    fail_next(&slack, "chat.postMessage");
    *console.inner.screen.lock().unwrap() = answer_screen("command-1", "Nothing is blocked.");
    eventually("posted on the next read", || {
        thread_posts(&slack, at).len() == 3
    })
    .await;
    let posts = texts(&thread_posts(&slack, at));
    assert!(posts[1].contains("Nothing is blocked.") && posts[2].contains("Nothing is blocked."));
    eventually("answered", || update_with(&slack, "answered in").is_some()).await;
    assert_eq!(
        slack.calls("chat.update").len(),
        1,
        "answered once, after the post"
    );
    assert_eq!(
        outcomes(&temp),
        [
            "ask:superintendent:sent",
            "relay:superintendent:post_failed",
            "relay:superintendent:relayed",
        ]
    );
}

#[tokio::test]
async fn without_an_asking_message_progress_is_posted_once_then_updated() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let relay = super::relay::RelayTiming {
        follow: Duration::from_secs(4),
        progress: Duration::from_millis(1_200),
        late: Duration::from_millis(200),
        ..fast_relay()
    };
    let (_notifier, _console) = asked_working(&temp, &slack, &socket, relay).await;
    // The "Asking…" post fails.
    fail_next(&slack, "chat.postMessage");
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("timed out", || {
        update_with(&slack, "No answer from the Superintendent after").is_some()
    })
    .await;
    let posts = texts(&thread_posts(&slack, at));
    assert_eq!(
        posts.len(),
        2,
        "failed Asking… plus one progress message: {posts:?}"
    );
    assert!(
        posts[1].starts_with("Still waiting for the Superintendent"),
        "{posts:?}"
    );
    // The timeout edits that one message (the first successful post).
    let timed_out = update_with(&slack, "No answer from the Superintendent after").unwrap();
    assert_eq!(timed_out["ts"], CARD_TS);
}

#[tokio::test]
async fn a_late_watch_survives_one_failed_pane_read() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let relay = super::relay::RelayTiming {
        follow: Duration::from_millis(600),
        progress: Duration::from_secs(60),
        late: Duration::from_secs(4),
        ..fast_relay()
    };
    let (_notifier, console) = asked_working(&temp, &slack, &socket, relay).await;
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("timed out", || {
        update_with(&slack, "No answer from the Superintendent after").is_some()
    })
    .await;
    // One transient read failure (herdr timeout), then the late answer.
    *console.inner.watch_error.lock().unwrap() =
        Some(ConsoleError::Failed("herdr read timed out".to_owned()));
    tokio::time::sleep(Duration::from_millis(250)).await;
    *console.inner.watch_error.lock().unwrap() = None;
    *console.inner.screen.lock().unwrap() = answer_screen("command-1", "Late but here.");
    eventually("late answer", || thread_posts(&slack, at).len() == 2).await;
    assert!(texts(&thread_posts(&slack, at))[1].contains("Late but here."));
    assert_eq!(
        outcomes(&temp).last().map(String::as_str),
        Some("relay:superintendent:relayed_late")
    );
}

#[tokio::test]
async fn a_question_sent_with_every_watch_slot_busy_offers_check_again() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let (notifier, console) = asked_working(&temp, &slack, &socket, fast_relay()).await;
    notifier.fill_watchers();
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("not watched", || {
        update_with(&slack, "won't post this one by itself").is_some()
    })
    .await;
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1, "still sent");
    let card = update_with(&slack, "won't post this one by itself").unwrap();
    let check = button_value(&card, "Check again");
    *console.inner.screen.lock().unwrap() = answer_screen("command-1", "Nothing is blocked.");
    socket.send(thread_click("env-2", &check, at));
    eventually("checked", || thread_posts(&slack, at).len() == 2).await;
    assert!(texts(&thread_posts(&slack, at))[1].contains("Nothing is blocked."));
}

fn set_health(console: &GatedConsole, health: super::actions::AgentHealth) {
    *console.inner.snapshot.lock().unwrap() = Ok(AgentSnapshot {
        identity: super::actions::tests::identity(),
        blocked: false,
        health,
    });
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn unreachable_or_disconnected_agents_are_reported_fast_with_retry() {
    use super::actions::AgentHealth;
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let relay = super::relay::RelayTiming {
        follow: Duration::from_secs(60),
        progress: Duration::from_secs(60),
        ..fast_relay()
    };
    let notifier = connected_relay(&temp, &slack, &socket, relay).await;
    let console = GatedConsole::new(super::prompt::tests::WORKING, 10_000);
    notifier.attach_console(console.clone());

    // Not running → said at once, nothing sent, Retry + Open in Yard.
    set_health(
        &console,
        AgentHealth::NotRunning("its agent process exited".to_owned()),
    );
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("unreachable", || {
        update_with(&slack, "The Superintendent isn't running").is_some()
    })
    .await;
    let card = update_with(&slack, "The Superintendent isn't running").unwrap();
    assert!(
        card["text"]
            .as_str()
            .unwrap()
            .contains("(its agent process exited)")
    );
    assert!(
        card["blocks"]
            .to_string()
            .contains(super::blocks::OPEN_ACTION_ID)
    );
    assert!(console.inner.prompts.lock().unwrap().is_empty(), "not sent");
    // Retry from a stranger does nothing and keeps the id usable.
    let retry = button_value(&card, "Retry");
    socket.send(with(
        thread_click("env-2", &retry, at),
        "/payload/user/id",
        json!("U02STRANGER"),
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(console.inner.prompts.lock().unwrap().is_empty());
    // The owner's Retry re-sends the same question as a new command.
    set_health(&console, AgentHealth::Working);
    socket.send(thread_click("env-3", &retry, at));
    eventually("retried", || {
        console.inner.prompts.lock().unwrap().len() == 1
    })
    .await;
    assert_eq!(
        console.inner.prompts.lock().unwrap()[0].0,
        super::relay::question_text("what is blocked?")
    );
    assert_eq!(
        thread_posts(&slack, at).len(),
        2,
        "second Asking… in the thread"
    );
    // Reusing the Retry id sends nothing more.
    socket.send(thread_click("env-4", &retry, at));
    eventually("reused", || thread_posts(&slack, at).len() == 3).await;
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 1);

    // The Codex TUI loses its app-server session while answering: the
    // progress message becomes an error card long before the window ends.
    let started = Instant::now();
    *console.inner.screen.lock().unwrap() = [
        "■ Connection lost (an older incident, above the question)",
        "> From Slack (owner): what is blocked?",
        "Status protocol for command \"command-1\": respond with JSON",
        "• Working (12s • esc to interrupt)",
        "■ Connection lost <retrying> & reconnecting. The app-server session could not be restored.",
        "Disconnected from this task.",
    ]
    .join("\n");
    eventually("disconnected", || {
        update_with(&slack, "can't answer right now").is_some()
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "long before the window"
    );
    let card = update_with(&slack, "can't answer right now").unwrap();
    assert!(
        card["text"]
            .as_str()
            .unwrap()
            .contains("Connection lost &lt;retrying&gt; &amp; reconnecting. The app-server session could not be restored."),
        "{card}"
    );
    // Escaped once, in the card and its fallback text alike.
    assert!(!card.to_string().contains("&amp;lt;"), "{card}");
    button_value(&card, "Retry");

    // The agent exits while a question is open: detected from the binding.
    *console.inner.screen.lock().unwrap() = super::prompt::tests::WORKING.to_owned();
    let at = at + 1;
    socket.send(message("env-5", "Ev5", "still there?", at));
    eventually("sent", || console.inner.prompts.lock().unwrap().len() == 2).await;
    set_health(
        &console,
        AgentHealth::NotRunning("its pane is gone or no longer shows an agent".to_owned()),
    );
    eventually("exited", || {
        update_with(&slack, "its pane is gone or no longer shows an agent").is_some()
    })
    .await;

    // A pane read failing for good (binding stale) ends the watch too.
    set_health(&console, AgentHealth::Working);
    let at = at + 1;
    socket.send(message("env-6", "Ev6", "and you?", at));
    eventually("sent", || console.inner.prompts.lock().unwrap().len() == 3).await;
    *console.inner.watch_error.lock().unwrap() = Some(ConsoleError::IdentityChanged(
        "the runtime binding is stale".to_owned(),
    ));
    eventually("stale", || {
        update_with(&slack, "binding is stale").is_some()
    })
    .await;

    // Already disconnected before the question (the process lives on):
    // refused at once with Retry, nothing typed.
    *console.inner.watch_error.lock().unwrap() = None;
    *console.inner.screen.lock().unwrap() = [
        "• Earlier answer",
        "■ The app-server session could not be restored.",
        "  Disconnected from this task. Start a new session to continue <now> & retry.",
        "",
        "› Ask Codex to do anything",
    ]
    .join("\n");
    let at = at + 1;
    socket.send(message("env-7", "Ev7", "are you there?", at));
    eventually("refused disconnected", || {
        update_with(&slack, "isn't running (its terminal shows").is_some()
    })
    .await;
    let card = update_with(&slack, "isn't running (its terminal shows").unwrap();
    assert!(
        card["text"]
            .as_str()
            .unwrap()
            .contains("continue &lt;now&gt; &amp; retry"),
        "{card}"
    );
    assert!(!card.to_string().contains("&amp;lt;"), "{card}");
    button_value(&card, "Retry");
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 3, "not sent");

    assert_eq!(
        outcomes(&temp),
        [
            "ask:superintendent:refused:not_running",
            "nav_retry:superintendent:answered",
            "ask:superintendent:sent",
            "nav:-:refused:already_used",
            "relay:superintendent:lost:disconnected",
            "ask:superintendent:sent",
            "relay:superintendent:lost:not_running",
            "ask:superintendent:sent",
            "relay:superintendent:lost:terminal_changed",
            "ask:superintendent:refused:disconnected",
        ]
    );
}

#[tokio::test]
async fn blocked_notifications_get_the_prompt_card_in_their_thread_only_with_inbound() {
    for inbound in [true, false] {
        let temp = TempDir::new().unwrap();
        let slack = MockSlack::start().await;
        let config = inbound_settings(if inbound {
            enabled()
        } else {
            SlackInbound::Off
        });
        let notifier =
            inbound_notifier_timed(&temp, &slack, config, 0, super::tests::instant_timing()).await;
        let console = GatedConsole::new(CLAUDE_PERMISSION, 100);
        notifier.attach_console(console.clone());
        notifier.tick().await; // silent baseline
        super::tests::set_orchestrator_status(&temp, "blocked", 2);
        notifier.tick().await; // pending
        notifier.tick().await; // settled: the Phase 1 DM
        notifier.tick().await;
        if inbound {
            eventually("card", || slack.calls("chat.postMessage").len() == 2).await;
        } else {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let posts = slack.calls("chat.postMessage");
        assert!(
            posts[0].body["text"]
                .as_str()
                .unwrap()
                .contains("The project orchestrator is blocked"),
            "{:?}",
            posts[0].body
        );
        if !inbound {
            assert_eq!(posts.len(), 1, "no card without inbound");
            continue;
        }
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[1].body["thread_ts"], CARD_TS, "in the DM's thread");
        let labels = posts[1].body["blocks"][2]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|button| button["text"]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["Allow once", "Deny"]);
        assert!(
            posts[1].body["text"]
                .as_str()
                .unwrap()
                .starts_with("Checkout &lt;!channel&gt; · The project orchestrator"),
            "{:?}",
            posts[1].body["text"]
        );
        assert!(console.typed().is_empty());
    }
}

#[tokio::test]
async fn each_question_card_is_its_own_thread_and_unblocked_agents_show_nothing() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    let console = GatedConsole::new(super::prompt::tests::CLAUDE_QUESTION, 100);
    notifier.attach_console(console.clone());
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    let (a, b) = (
        super::actions::tests::target(),
        AgentTarget::Assignment {
            project_id: "project-1".to_owned(),
            assignment_id: "assignment-2".to_owned(),
        },
    );
    // Two question cards for one shared (notification / `blocked`) thread.
    let root = "1700000000.900000";
    notifier
        .post_prompt_card(&a, "Worker A", Some(root))
        .await
        .unwrap();
    notifier
        .post_prompt_card(&b, "Worker B", Some(root))
        .await
        .unwrap();
    let posts = slack.calls("chat.postMessage");
    let cards = &posts[posts.len() - 2..];
    assert!(
        cards
            .iter()
            .all(|card| card.body.get("thread_ts").is_none()),
        "question cards are top level: {cards:?}"
    );
    let card_ts = |index: usize| {
        let ts = 1 + posts.len() - 2 + index;
        format!("1700000000.{ts:06}")
    };
    // The owner answers A in A's own card thread: it reaches A only.
    socket.send(with(
        message("env-1", "Ev1", "yes, drop the table", now_s()),
        "/payload/event/thread_ts",
        json!(card_ts(0)),
    ));
    eventually("prompt", || {
        !console.inner.prompt_targets.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        *console.inner.prompt_targets.lock().unwrap(),
        std::slice::from_ref(&a)
    );
    eventually("reply confirmed", || {
        posted_texts(&slack)
            .iter()
            .any(|text| text.starts_with("Sent your reply to Worker A"))
    })
    .await;

    // An agent that is no longer blocked: no excerpt, no buttons, no thread.
    *console.inner.snapshot.lock().unwrap() = Ok(AgentSnapshot {
        identity: super::actions::tests::identity(),
        blocked: false,
        health: super::actions::AgentHealth::Working,
    });
    *console.inner.screen.lock().unwrap() =
        "cat config.toml\nsecret_line = visible-live-output\n".to_owned();
    let before = slack.calls("chat.postMessage").len();
    notifier
        .post_prompt_card(&a, "Worker A", Some(root))
        .await
        .unwrap();
    let posts = slack.calls("chat.postMessage");
    assert_eq!(posts.len(), before + 1);
    let note = posts.last().unwrap().body.to_string();
    assert!(note.contains("no longer blocked"), "{note}");
    assert!(!note.contains("visible-live-output") && !note.contains("config.toml"));
    assert!(!note.contains("\"actions\""), "no buttons: {note}");
}

#[tokio::test]
async fn a_second_socket_connection_is_reported_in_the_status() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    let hello = |num_connections| super::socket::SocketEvent::Hello {
        num_connections,
        app_id: Some("A01APP".to_owned()),
    };
    notifier.handle_socket_event(hello(2)).await;
    let status = notifier.inbound_status();
    assert_eq!(status.status, InboundStatusKind::Connected);
    assert!(
        status
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("one Yard only")),
        "{status:?}"
    );
    let error = notifier.inbound_status().last_error.unwrap();
    assert!(
        error.contains("Slack reported 2 Socket Mode connections")
            && error.contains("may be stale"),
        "{error}"
    );
    // A later hello with one connection clears it; another stale count
    // shows again and clears again.
    notifier.handle_socket_event(hello(1)).await;
    assert_eq!(notifier.inbound_status().last_error, None);
    notifier.handle_socket_event(hello(3)).await;
    assert!(
        notifier
            .inbound_status()
            .last_error
            .unwrap()
            .contains("reported 3")
    );
    notifier.handle_socket_event(hello(1)).await;
    assert_eq!(notifier.inbound_status().last_error, None);
}

#[tokio::test]
async fn shutdown_closes_the_socket_cleanly_and_does_not_reconnect() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    assert_eq!(socket.connections(), 1);
    assert!(socket.closes.lock().unwrap().is_empty());
    notifier.close_inbound(Duration::from_secs(5)).await;
    eventually("close frame", || !socket.closes.lock().unwrap().is_empty()).await;
    assert_eq!(
        socket.closes.lock().unwrap().clone(),
        [(1000, "yard shutting down".to_owned())]
    );
    eventually("stopped", || {
        notifier.inbound_status().status == InboundStatusKind::Off
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(socket.connections(), 1, "no reconnect after shutdown");
    assert_eq!(slack.calls("apps.connections.open").len(), 1);
}

#[tokio::test]
async fn shutdown_ends_open_questions_instead_of_leaving_them_waiting() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let relay = super::relay::RelayTiming {
        follow: Duration::from_secs(60),
        progress: Duration::from_secs(60),
        ..fast_relay()
    };
    let (notifier, console) = asked_working(&temp, &slack, &socket, relay).await;
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("sent", || console.inner.prompts.lock().unwrap().len() == 1).await;
    eventually("watching", || {
        console.inner.report_reads.lock().unwrap().len() >= 2
    })
    .await;
    notifier.close_inbound(Duration::from_secs(5)).await;
    let card = update_with(&slack, "Yard restarted before the Superintendent answered")
        .expect("progress message ended");
    assert_eq!(card["ts"], CARD_TS);
    assert!(
        card["blocks"]
            .to_string()
            .contains(super::blocks::OPEN_ACTION_ID)
    );
    assert_eq!(
        outcomes(&temp).last().map(String::as_str),
        Some("relay:superintendent:ended:restart")
    );
    // A question that already ended is not touched again.
    let updates = slack.calls("chat.update").len();
    notifier.close_inbound(Duration::from_secs(1)).await;
    assert_eq!(slack.calls("chat.update").len(), updates);
}

/// The same menu with a second question.
fn second_question() -> String {
    super::prompt::tests::CLAUDE_MENU
        .replace(
            "Who should own the Slack socket?",
            "Which port should it use?",
        )
        .replace("1. Yard server", "1. Port 4317")
        .replace("2. Herdr plugin", "2. Port 4318")
}

#[tokio::test]
async fn after_a_click_the_next_prompt_or_a_stuck_one_is_reported() {
    for case in ["next", "stuck", "done"] {
        let temp = TempDir::new().unwrap();
        let slack = MockSlack::start().await;
        let socket = MockSocket::start().await;
        let notifier = connected(&temp, &slack, &socket).await;
        let console = GatedConsole::new(super::prompt::tests::CLAUDE_MENU, 1000);
        notifier.attach_console(console.clone());
        let target = super::actions::tests::target();
        notifier
            .post_prompt_card(&target, "Worker", None)
            .await
            .unwrap();
        let card = slack.calls("chat.postMessage").pop().unwrap();
        let first = card.body["blocks"][2]["elements"][0]["value"]
            .as_str()
            .unwrap()
            .to_owned();
        socket.send(card_click("env-1", &first, now_s()));
        eventually("keys", || console.typed().len() == 1).await;
        // The agent reacts to the keys before the pane is re-read.
        match case {
            "next" => *console.inner.screen.lock().unwrap() = second_question(),
            "done" => set_blocked(&console, false),
            _ => {}
        }
        eventually("sent", || {
            posted_texts(&slack)
                .iter()
                .any(|text| text.starts_with("Sent to"))
        })
        .await;
        let in_thread = || {
            slack
                .calls("chat.postMessage")
                .into_iter()
                .filter(|call| call.body["thread_ts"] == CARD_TS)
                .map(|call| call.body)
                .collect::<Vec<_>>()
        };
        let expected = match case {
            "next" => 3,
            "stuck" => 2,
            _ => 1,
        };
        eventually("follow-up", || in_thread().len() >= expected).await;
        // Posts are paced about 1 s apart: wait past settle + one post.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        let in_thread = in_thread();
        let texts = texts(&in_thread);
        match case {
            "next" => {
                assert_eq!(texts.len(), 3, "{texts:?}");
                assert!(texts[1].contains("waiting on another prompt"), "{texts:?}");
                let next = in_thread[2].to_string();
                assert!(
                    next.contains("Port 4317") && next.contains("Which port"),
                    "{next}"
                );
            }
            "stuck" => {
                assert_eq!(texts.len(), 2, "{texts:?}");
                assert!(texts[1].contains("still shows this prompt"), "{texts:?}");
            }
            _ => assert_eq!(texts.len(), 1, "no longer blocked: {texts:?}"),
        }
    }
}

#[tokio::test]
async fn refused_and_dead_buttons_collapse_to_not_sent() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    let console = GatedConsole::new(super::prompt::tests::CLAUDE_MENU, 1000);
    notifier.attach_console(console.clone());
    let target = super::actions::tests::target();
    notifier
        .post_prompt_card(&target, "Worker", None)
        .await
        .unwrap();
    let card = slack.calls("chat.postMessage").pop().unwrap();
    let first = card.body["blocks"][2]["elements"][0]["value"]
        .as_str()
        .unwrap()
        .to_owned();
    *console.inner.screen.lock().unwrap() = second_question();
    socket.send(card_click("env-1", &first, now_s()));
    eventually("collapsed", || !slack.calls("chat.update").is_empty()).await;
    let update = slack.calls("chat.update").pop().unwrap();
    assert_eq!(
        update.body["text"],
        "Worker: not sent (the prompt changed)."
    );
    // A button Yard does not know (restart) or that expired is removed.
    socket.send(card_click("env-2", &"f".repeat(32), now_s()));
    eventually("dead card collapsed", || {
        slack.calls("chat.update").len() == 2
    })
    .await;
    let update = slack.calls("chat.update").pop().unwrap();
    assert_eq!(update.body["ts"], CARD_TS);
    assert!(
        update.body["text"].as_str().unwrap().contains("not sent"),
        "{update:?}"
    );
    assert!(console.typed().is_empty());
}

#[tokio::test]
async fn an_unknown_navigation_button_leaves_its_card_alone() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    let console = GatedConsole::new(super::prompt::tests::CLAUDE_MENU, 1000);
    notifier.attach_console(console.clone());
    // "Status" on a help card, "Check again" on a timeout card, after a
    // Yard restart: Yard no longer knows the ids.
    for (envelope, action_id) in [
        ("env-1", "yard_nav_status_ffffffff"),
        ("env-2", "yard_nav_check_again_eeeeeeee"),
    ] {
        let value = "e".repeat(31) + &envelope[4..];
        let clicked = with(
            card_click(envelope, &value, now_s()),
            "/payload/actions/0/action_id",
            json!(action_id),
        );
        socket.send(clicked);
    }
    eventually("replies", || slack.calls("chat.postMessage").len() == 2).await;
    for text in posted_texts(&slack) {
        assert!(text.contains("no longer knows that button"), "{text}");
        assert!(text.contains("`help`"), "{text}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(slack.calls("chat.update").is_empty(), "card overwritten");
    assert_eq!(
        outcomes(&temp)
            .iter()
            .filter(|outcome| *outcome == "nav:-:refused:unknown")
            .count(),
        2
    );
    assert!(console.typed().is_empty());
}

#[tokio::test]
async fn a_reply_in_a_menu_or_notification_thread_is_never_a_superintendent_question() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    let console = GatedConsole::new(super::prompt::tests::CLAUDE_MENU, 100);
    notifier.attach_console(console.clone());
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    // A menu card in the project's notification thread.
    let root = "1700000000.900000";
    let target = super::actions::tests::target();
    notifier
        .post_prompt_card(&target, "Implementer", Some(root))
        .await
        .unwrap();
    // The owner types an answer in that thread instead of clicking.
    socket.send(with(
        message(
            "env-1",
            "Ev1",
            "Pick us-west-2 but only after the canary",
            now_s(),
        ),
        "/payload/event/thread_ts",
        json!(root),
    ));
    eventually("refusal", || {
        posted_texts(&slack)
            .iter()
            .any(|text| text.starts_with("Nothing was sent"))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        console.inner.prompt_targets.lock().unwrap().is_empty(),
        "nothing prompted, least of all the Superintendent"
    );
    assert!(console.typed().is_empty());
    let audit = audit_lines(&temp);
    assert!(
        audit
            .iter()
            .any(|line| line["outcome"] == "refused:not_a_question"),
        "{audit:?}"
    );
    // A top-level question still reaches the Superintendent (once it is
    // not itself waiting on a prompt).
    set_blocked(&console, false);
    socket.send(message("env-2", "Ev2", "what is left today?", now_s()));
    eventually("superintendent prompt", || {
        !console.inner.prompt_targets.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        *console.inner.prompt_targets.lock().unwrap(),
        [AgentTarget::YardOrchestrator]
    );
}

#[tokio::test]
async fn an_agent_that_finished_by_asking_gets_a_question_card() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let notifier = inbound_notifier(&temp, &slack, inbound_settings(enabled()), 0).await;
    let console = GatedConsole::new(super::prompt::tests::CLAUDE_QUESTION, 100);
    // Herdr reports a question at the input box as done, not blocked.
    set_blocked(&console, false);
    notifier.attach_console(console.clone());
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    let target = super::actions::tests::target();
    let before = slack.calls("chat.postMessage").len();
    notifier
        .post_question_if_asked(&target, "Implementer")
        .await
        .unwrap();
    let posts = slack.calls("chat.postMessage");
    assert_eq!(posts.len(), before + 1);
    let card = &posts[before];
    assert!(card.body.get("thread_ts").is_none(), "its own thread root");
    assert!(
        card.body["text"]
            .as_str()
            .unwrap()
            .contains("asked a question"),
        "{:?}",
        card.body
    );
    let card_ts = format!("1700000000.{:06}", before + 1);
    socket.send(with(
        message("env-1", "Ev1", "yes, go ahead", now_s()),
        "/payload/event/thread_ts",
        json!(card_ts),
    ));
    eventually("reply", || {
        !console.inner.prompt_targets.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        *console.inner.prompt_targets.lock().unwrap(),
        std::slice::from_ref(&target)
    );

    // A finished agent whose screen is not a question: nothing is posted.
    *console.inner.screen.lock().unwrap() =
        "⏺ Done. Tests pass.\nsecret_line = visible-live-output\n".to_owned();
    let before = slack.calls("chat.postMessage").len();
    notifier
        .post_question_if_asked(&target, "Implementer")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(slack.calls("chat.postMessage").len(), before);
}

/// The value of the button labelled `label` anywhere in `post`.
fn button_value(post: &Value, label: &str) -> String {
    post["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|block| {
            block["elements"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .chain(block.get("accessory").cloned())
        })
        .find(|element| element["text"]["text"] == label)
        .and_then(|element| element["value"].as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("no {label} button in {post}"))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn navigation_buttons_run_commands_through_the_guarded_click_path() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    let console = GatedConsole::new(CLAUDE_PERMISSION, 100);
    notifier.attach_console(console.clone());

    let at = now_s();
    let root = format!("{at}.000100");
    socket.send(message("env-1", "Ev1", "help", at));
    eventually("help card", || thread_posts(&slack, at).len() == 1).await;
    let help = thread_posts(&slack, at).remove(0);
    assert_eq!(help["blocks"][0]["type"], "header");
    let status = button_value(&help, "Status");
    let review = button_value(&help, "Review");
    assert!(help.to_string().contains("https://yard.example.test/"));
    let in_thread = |envelope: &str, value: &str, at_s: u64| {
        with(
            click(envelope, value, at_s),
            "/payload/message/thread_ts",
            json!(root),
        )
    };

    // A stranger's click is dropped and does not use up the button.
    let stranger = with(
        in_thread("env-2", &review, at + 1),
        "/payload/user/id",
        json!("U02STRANGER"),
    );
    socket.send(stranger);
    // "Open in Yard" only opens the browser: dropped, nothing posted.
    let link = with(
        in_thread("env-3", &"0".repeat(32), at + 1),
        "/payload/actions/0",
        json!({
            "type": "button", "action_id": "yard_open", "url": "https://yard.example.test/",
            "action_ts": format!("{}.123456", at + 1),
        }),
    );
    socket.send(link);
    eventually("drops audited", || {
        audit_lines(&temp)
            .iter()
            .filter(|line| line["kind"] == "dropped")
            .count()
            == 2
    })
    .await;
    assert_eq!(thread_posts(&slack, at).len(), 1, "nothing posted");

    // The owner's click runs `status` in the same thread.
    socket.send(in_thread("env-4", &status, at + 2));
    eventually("status card", || thread_posts(&slack, at).len() == 2).await;
    let card = thread_posts(&slack, at).remove(1);
    assert!(
        card["text"].as_str().unwrap().starts_with("*Yard status*"),
        "{card}"
    );
    let details = button_value(&card, "Details");

    // Used once: a second click runs nothing.
    socket.send(in_thread("env-5", &status, at + 3));
    eventually("reused", || thread_posts(&slack, at).len() == 3).await;
    assert!(
        texts(&thread_posts(&slack, at))[2].contains("already used"),
        "{:?}",
        texts(&thread_posts(&slack, at))
    );

    // "Details" runs `status <project>` for that project id.
    socket.send(in_thread("env-6", &details, at + 4));
    eventually("project card", || thread_posts(&slack, at).len() == 4).await;
    let project = thread_posts(&slack, at).remove(3);
    assert!(
        project["text"]
            .as_str()
            .unwrap()
            .starts_with("*Checkout &lt;!channel&gt;*"),
        "{project}"
    );
    assert_eq!(project["blocks"][0]["type"], "header");

    // The review button still works for the owner (the stranger's click
    // did not consume it).
    socket.send(in_thread("env-7", &review, at + 5));
    eventually("review", || thread_posts(&slack, at).len() == 5).await;
    assert_eq!(
        texts(&thread_posts(&slack, at))[4],
        "Nothing is waiting for review."
    );

    // A made-up id is unknown to Yard and runs nothing.
    socket.send(in_thread("env-8", &"e".repeat(32), at + 6));
    eventually("unknown", || thread_posts(&slack, at).len() == 6).await;
    assert!(texts(&thread_posts(&slack, at))[5].contains("does not know that button"));
    assert!(console.typed().is_empty(), "navigation never types keys");

    let audit = audit_lines(&temp)
        .iter()
        .map(|line| {
            format!(
                "{}/{}/{}",
                line["kind"].as_str().unwrap(),
                line["action"].as_str().unwrap(),
                line["outcome"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        "message/help/answered",
        "dropped/not_the_owner/dropped",
        "dropped/link_button/dropped",
        "button/nav_status/answered",
        "button/nav/refused:already_used",
        "button/nav_status_project/answered",
        "button/nav_review/answered",
        "button/unknown/refused:unknown",
    ] {
        assert!(
            audit.contains(&expected.to_owned()),
            "{expected}: {audit:?}"
        );
    }
}

#[tokio::test]
async fn questions_to_agents_yard_refuses_input_for_are_not_sent() {
    use super::actions::AgentHealth;
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected_relay(&temp, &slack, &socket, fast_relay()).await;
    let console = GatedConsole::new(super::prompt::tests::WORKING, 10_000);
    notifier.attach_console(console.clone());
    set_health(&console, AgentHealth::Working);

    // A managed pane whose lease needs recovery: refused before sending,
    // with the reason, Retry and Open in Yard.
    *console.inner.send_refusal.lock().unwrap() = Some(ConsoleError::Unavailable(
        super::console::LEASE_RECOVERY_REQUIRED.to_owned(),
    ));
    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is blocked?", at));
    eventually("refused", || update_with(&slack, "was not sent").is_some()).await;
    let card = update_with(&slack, "was not sent").unwrap();
    let text = card["text"].as_str().unwrap();
    assert!(
        text.contains("Yard lost the pane lease; recovery is required."),
        "{text}"
    );
    assert!(!text.contains("required.."), "{text}");
    assert!(
        card["blocks"]
            .to_string()
            .contains(super::blocks::OPEN_ACTION_ID)
    );
    assert!(console.inner.prompts.lock().unwrap().is_empty(), "not sent");
    let retry = button_value(&card, "Retry");

    // Once recovered, the owner's Retry sends it.
    *console.inner.send_refusal.lock().unwrap() = None;
    socket.send(thread_click("env-2", &retry, at));
    eventually("retried", || {
        console.inner.prompts.lock().unwrap().len() == 1
    })
    .await;
    assert!(
        audit_lines(&temp)
            .iter()
            .any(|line| line["outcome"] == "refused:unavailable"),
        "{:?}",
        audit_lines(&temp)
    );
}

/// Quiet hours never hold or delay the owner's own conversation: a command
/// typed at 20:30 is answered at once while a blocked notification waits.
#[tokio::test]
async fn owner_replies_are_never_held_by_quiet_hours() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let clock = super::policy::tests::TestClock::at(super::policy::tests::WORKDAY_EVENING);
    let notifier = inbound_notifier_relay(
        &temp,
        &slack,
        inbound_settings(enabled()),
        0,
        super::tests::instant_timing(),
        fast_relay(),
        super::policy::tests::parts(&clock, Some(&temp)),
    )
    .await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    notifier.attach_console(GatedConsole::new(CLAUDE_PERMISSION, 100));

    notifier.tick().await; // baseline
    super::tests::set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    let quiet = notifier.status().quiet.unwrap();
    assert!(quiet.active);
    assert_eq!(quiet.held_count, 1);
    assert!(slack.calls("chat.postMessage").is_empty());

    let at = now_s();
    socket.send(message("env-1", "Ev1", "status", at));
    eventually("status reply", || thread_posts(&slack, at).len() == 1).await;
    // Only the reply went out; the notification is still held.
    assert_eq!(slack.calls("chat.postMessage").len(), 1);
    assert_eq!(notifier.status().quiet.unwrap().held_count, 1);
}

/// `settings`, its buttons and the typed `mute …` / `unmute` / `digest`
/// commands run through the owner-only, audited, single-use path.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn quiet_controls_mute_unmute_and_digest_through_the_guarded_path() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let clock = super::policy::tests::TestClock::at(super::policy::tests::WORKDAY_EVENING);
    let notifier = inbound_notifier_relay(
        &temp,
        &slack,
        inbound_settings(enabled()),
        0,
        super::tests::instant_timing(),
        fast_relay(),
        super::policy::tests::parts(&clock, Some(&temp)),
    )
    .await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    notifier.attach_console(GatedConsole::new(CLAUDE_PERMISSION, 100));
    notifier.tick().await; // baseline
    super::tests::set_orchestrator_status(&temp, "blocked", 2);
    notifier.tick().await;
    notifier.tick().await;
    assert_eq!(notifier.status().quiet.unwrap().held_count, 1);

    // The help card offers the settings card.
    let at = now_s();
    socket.send(message("env-0", "Ev0", "help", at - 100));
    eventually("help", || thread_posts(&slack, at - 100).len() == 1).await;
    let help = thread_posts(&slack, at - 100).remove(0);
    assert!(help.to_string().contains("`mute 2h`"), "{help}");
    button_value(&help, "Quiet hours");

    socket.send(message("env-1", "Ev1", "settings", at));
    eventually("settings card", || thread_posts(&slack, at).len() == 1).await;
    let card = thread_posts(&slack, at).remove(0);
    let json = card.to_string();
    assert!(json.contains("Quiet now"), "{json}");
    assert!(json.contains("(1 held)"), "{json}");
    let mute = button_value(&card, "Mute 1h");
    let unmute = button_value(&card, "Unmute");
    assert!(
        !json.contains("Mute(") && !json.contains("For("),
        "opaque values only"
    );
    let root = format!("{at}.000100");
    let in_thread = |envelope: &str, value: &str, at_s: u64| {
        with(
            click(envelope, value, at_s),
            "/payload/message/thread_ts",
            json!(root),
        )
    };

    // A stranger's click is dropped and does not mute or use the button.
    socket.send(with(
        in_thread("env-2", &mute, at + 1),
        "/payload/user/id",
        json!("U02STRANGER"),
    ));
    eventually("stranger dropped", || {
        audit_lines(&temp)
            .iter()
            .any(|line| line["kind"] == "dropped")
    })
    .await;
    assert_eq!(notifier.status().quiet.unwrap().muted_until, None);

    socket.send(in_thread("env-3", &mute, at + 2));
    eventually("muted", || thread_posts(&slack, at).len() == 2).await;
    assert!(texts(&thread_posts(&slack, at))[1].contains("Muted until"));
    // Thursday 20:30 PDT + 1 h.
    let muted = notifier.status().quiet.unwrap().muted_until;
    assert_eq!(muted, Some(1_790_915_400_000));

    socket.send(in_thread("env-4", &mute, at + 3));
    eventually("reused", || thread_posts(&slack, at).len() == 3).await;
    assert!(texts(&thread_posts(&slack, at))[2].contains("already used"));

    socket.send(in_thread("env-5", &unmute, at + 4));
    eventually("unmuted", || thread_posts(&slack, at).len() == 4).await;
    assert!(texts(&thread_posts(&slack, at))[3].contains("outside working hours"));
    assert_eq!(notifier.status().quiet.unwrap().muted_until, None);

    // Typed commands.
    socket.send(message("env-6", "Ev6", "mute until monday", at + 10));
    eventually("mute until", || thread_posts(&slack, at + 10).len() == 1).await;
    assert!(texts(&thread_posts(&slack, at + 10))[0].contains("Mon Oct 5, 9:00 AM PDT"));
    socket.send(message("env-7", "Ev7", "mute forever", at + 20));
    eventually("usage", || thread_posts(&slack, at + 20).len() == 1).await;
    assert!(texts(&thread_posts(&slack, at + 20))[0].contains("mute 30m"));
    assert!(notifier.status().quiet.unwrap().muted_until.is_some());

    // `digest` while muted at night: the owner asked, so it goes out now,
    // as the reply in the command's own thread (no tick needed).
    let before = slack.calls("chat.postMessage").len();
    socket.send(message("env-8", "Ev8", "digest", at + 30));
    eventually("digest in the command thread", || {
        thread_posts(&slack, at + 30).len() == 1
    })
    .await;
    assert!(texts(&thread_posts(&slack, at + 30))[0].contains("Yard digest"));
    eventually("digest delivered", || {
        notifier.status().quiet.unwrap().held_count == 0
    })
    .await;
    notifier.tick().await;
    let posts = slack.calls("chat.postMessage");
    assert_eq!(posts.len(), before + 1, "only the digest, once");

    let audit = audit_lines(&temp)
        .iter()
        .map(|line| {
            format!(
                "{}/{}/{}",
                line["kind"].as_str().unwrap(),
                line["action"].as_str().unwrap(),
                line["outcome"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        "message/settings/answered",
        "dropped/not_the_owner/dropped",
        "button/nav_mute/answered",
        "button/nav/refused:already_used",
        "button/nav_unmute/answered",
        "message/mute/answered",
        "message/mute/refused:invalid",
        "message/digest/requested",
    ] {
        assert!(
            audit.contains(&expected.to_owned()),
            "{expected}: {audit:?}"
        );
    }
}

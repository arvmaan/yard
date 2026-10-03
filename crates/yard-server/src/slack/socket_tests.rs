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
        let (frames, count) = (received.clone(), Arc::clone(&connections));
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
                relay: super::relay::RelayTiming {
                    poll: Duration::from_millis(50),
                    wait: Duration::from_millis(400),
                    follow_poll: Duration::from_millis(100),
                    follow: Duration::from_millis(900),
                },
                settle: Duration::from_millis(250),
            },
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
    let buttons = card.body["blocks"][1]["elements"]
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
    assert!(
        card.contains("Checkout &lt;!channel&gt; · The project orchestrator"),
        "{card}"
    );
    let buttons = posts[1]["blocks"][1]["elements"].as_array().unwrap();
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
    let console = GatedConsole::new(super::prompt::tests::WORKING, 100);
    set_blocked(&console, false);
    notifier.attach_console(console.clone());
    let project_id: String = rusqlite::Connection::open(temp.path().join("yard.sqlite3"))
        .unwrap()
        .query_row("SELECT id FROM projects", [], |row| row.get(0))
        .unwrap();

    // `<Project>: …` goes to that project's orchestrator, marked as from
    // Slack, with the answer instruction; the report is relayed.
    let at = now_s();
    socket.send(message(
        "env-1",
        "Ev1",
        // As Slack delivers typed `<`, `>` and `&`: entity-encoded.
        "checkout &lt;!channel&gt;: what's left? a &amp;&amp; b token=abc123",
        at,
    ));
    eventually("sent", || thread_posts(&slack, at).len() == 1).await;
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
    assert_eq!(actor, "slack:owner:U01OWNER");
    assert!(
        texts(&thread_posts(&slack, at))[0]
            .starts_with("Sent to the Checkout &lt;!channel&gt; orchestrator."),
        "{:?}",
        texts(&thread_posts(&slack, at))
    );
    // Another command's report and prose are never relayed; ours is.
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
                "Two tasks left; key sk-",
                "ant-api03-SECRETSECRETSECRETSECRET"
            )
        )
    );
    eventually("answer", || thread_posts(&slack, at).len() == 2).await;
    let answer = texts(&thread_posts(&slack, at))[1].clone();
    assert!(
        answer.starts_with(
            "*Answer from the Checkout &lt;!channel&gt; orchestrator* (working)\nTwo tasks left"
        ),
        "{answer}"
    );
    assert!(!answer.contains("SECRETSECRET"), "redacted: {answer}");
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

    // Anything else goes to the Superintendent; no answer → a note, then
    // the watch ends with "open Yard".
    *console.inner.screen.lock().unwrap() = super::prompt::tests::WORKING.to_owned();
    let at = at + 1;
    socket.send(message("env-2", "Ev2", "what is blocked right now?", at));
    eventually("timed out", || thread_posts(&slack, at).len() == 3).await;
    let posts = texts(&thread_posts(&slack, at));
    assert_eq!(
        console.inner.prompt_targets.lock().unwrap()[1],
        AgentTarget::YardOrchestrator
    );
    assert!(
        posts[0].starts_with("Sent to the Superintendent."),
        "{posts:?}"
    );
    assert!(
        posts[1].starts_with("Yard has not found an answer from the Superintendent yet"),
        "{posts:?}"
    );
    assert!(
        posts[2].starts_with("Yard found no answer from the Superintendent"),
        "{posts:?}"
    );

    // A blocked orchestrator gets no question: its prompt card instead.
    set_blocked(&console, true);
    *console.inner.screen.lock().unwrap() = CLAUDE_PERMISSION.to_owned();
    let at = at + 1;
    socket.send(message("env-3", "Ev3", "are you done?", at));
    eventually("blocked refusal", || thread_posts(&slack, at).len() == 2).await;
    let posts = thread_posts(&slack, at);
    assert!(
        texts(&posts)[0].contains("waiting on a prompt, so your question was not sent"),
        "{:?}",
        texts(&posts)
    );
    assert_eq!(
        posts[1]["blocks"][1]["elements"].as_array().unwrap().len(),
        2
    );
    assert_eq!(console.inner.prompts.lock().unwrap().len(), 2, "not sent");

    // A refused prompt (terminal open in Yard) is reported, not retried.
    set_blocked(&console, false);
    *console.inner.prompt_error.lock().unwrap() =
        Some(ConsoleError::Busy("open in Yard".to_owned()));
    let at = at + 1;
    socket.send(message("env-4", "Ev4", "hello?", at));
    eventually("busy", || thread_posts(&slack, at).len() == 1).await;
    assert!(
        texts(&thread_posts(&slack, at))[0].contains("open in Yard; answer it there"),
        "{:?}",
        texts(&thread_posts(&slack, at))
    );

    let outcomes = audit_lines(&temp)
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
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes,
        [
            format!("ask:orchestrator:{project_id}:sent"),
            format!("relay:orchestrator:{project_id}:relayed"),
            "ask:superintendent:sent".to_owned(),
            "relay:superintendent:timed_out".to_owned(),
            "ask:superintendent:refused:blocked".to_owned(),
            "ask:superintendent:refused:busy".to_owned(),
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
        let labels = posts[1].body["blocks"][1]["elements"]
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
    notifier.handle_socket_event(hello(1)).await;
    assert_eq!(notifier.inbound_status().last_error, None);
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
        let first = card.body["blocks"][1]["elements"][0]["value"]
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
    let first = card.body["blocks"][1]["elements"][0]["value"]
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

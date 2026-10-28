//! Conversations end to end: thread follow-ups, the dedicated Slack
//! Superintendent, its fallback and answer files, over the same fakes as
//! [`super`] (local Socket Mode server, mock Web API, scripted console).

use super::*;
use crate::slack::{
    actions::SLACK_PROVENANCE,
    superintendent::{SLACK_SUPERINTENDENT_TO, answer_file},
};

const NODE: &str = "0190aaaa-0000-7000-8000-00000000000a";

fn node() -> AgentTarget {
    AgentTarget::CoordinationNode {
        node_id: NODE.to_owned(),
    }
}

/// An owner reply at `at_s` in the thread rooted at `root_s`.
fn reply(envelope: &str, event: &str, text: &str, at_s: u64, root_s: u64) -> Value {
    with(
        message(envelope, event, text, at_s),
        "/payload/event/thread_ts",
        json!(format!("{root_s}.000100")),
    )
}

struct Harness {
    temp: TempDir,
    slack: MockSlack,
    socket: MockSocket,
    console: Arc<GatedConsole>,
    notifier: SlackNotifier,
}

impl Harness {
    async fn start(console: Arc<GatedConsole>) -> Self {
        Self::start_relay(console, None).await
    }

    async fn start_relay(
        console: Arc<GatedConsole>,
        relay: Option<crate::slack::relay::RelayTiming>,
    ) -> Self {
        let temp = TempDir::new().unwrap();
        let slack = MockSlack::start().await;
        let socket = MockSocket::start().await;
        set_blocked(&console, false);
        let notifier = match relay {
            Some(relay) => connected_relay(&temp, &slack, &socket, relay).await,
            None => connected(&temp, &slack, &socket).await,
        };
        notifier.attach_console(console.clone());
        Self {
            temp,
            slack,
            socket,
            console,
            notifier,
        }
    }

    /// A healthy Slack Superintendent at [`NODE`].
    async fn with_superintendent() -> Self {
        Self::with_superintendent_relay(None).await
    }

    /// [`Self::with_superintendent`] with its own watch timings.
    async fn with_superintendent_relay(relay: Option<crate::slack::relay::RelayTiming>) -> Self {
        let console = GatedConsole::new(crate::slack::prompt::tests::WORKING, 10_000);
        *console.inner.superintendent.lock().unwrap() = Some(Ok(node()));
        Self::start_relay(console, relay).await
    }

    fn prompts(&self) -> Vec<String> {
        self.console
            .inner
            .prompts
            .lock()
            .unwrap()
            .iter()
            .map(|(text, _)| text.clone())
            .collect()
    }

    fn targets(&self) -> Vec<AgentTarget> {
        self.console.inner.prompt_targets.lock().unwrap().clone()
    }

    async fn prompted(&self, count: usize) {
        eventually("prompt", || self.prompts().len() == count).await;
    }

    /// The agent reports `command_id` (and writes `file`, when set).
    fn answer(&self, command_id: &str, last: &str, file: Option<&str>) {
        *self.console.inner.answer_file.lock().unwrap() = file.map(str::to_owned);
        *self.console.inner.screen.lock().unwrap() = answer_screen(command_id, last);
    }

    /// Answers posted in the thread rooted at `root_s`.
    fn answers(&self, root_s: u64) -> Vec<String> {
        texts(&thread_posts(&self.slack, root_s))
            .into_iter()
            .filter(|text| text.starts_with("*Answer from"))
            .collect()
    }

    async fn answered(&self, root_s: u64, count: usize) {
        eventually("answer", || self.answers(root_s).len() == count).await;
    }

    fn outcomes(&self) -> Vec<String> {
        outcomes(&self.temp)
    }

    fn posted(&self, root_s: u64, start: &str) -> bool {
        texts(&thread_posts(&self.slack, root_s))
            .iter()
            .any(|text| text.starts_with(start))
    }
}

#[tokio::test]
async fn a_top_level_question_goes_to_the_slack_superintendent_and_relays_its_answer_file() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what is left today?", at));
    harness.prompted(1).await;
    assert_eq!(harness.targets(), [node()], "not the main Superintendent");
    let prompt = harness.prompts()[0].clone();
    assert!(prompt.starts_with(&format!("{SLACK_PROVENANCE} what is left today?")));
    assert!(!prompt.contains("Earlier in this Slack conversation"));
    let key = harness.console.inner.answer_reads.lock().unwrap().clone();
    assert!(key.is_empty(), "nothing read before the report");
    assert!(prompt.contains("`slack-answers/"), "{prompt}");
    assert!(harness.posted(at, &format!("Asking {SLACK_SUPERINTENDENT_TO}…")));

    harness.answer(
        "command-1",
        "answered",
        Some("**Two** tasks left:\n- tests\n- docs"),
    );
    harness.answered(at, 1).await;
    let answer = harness.answers(at)[0].clone();
    assert!(
        answer.starts_with("*Answer from the Slack Superintendent* (working)\n**Two** tasks left"),
        "{answer}"
    );
    assert!(!answer.contains("answered\n"), "the file, not `last`");
    let reads = harness.console.inner.answer_reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 1);
    assert!(
        prompt.contains(&answer_file(&reads[0])),
        "the file named in the prompt"
    );
    eventually("forgotten", || {
        *harness.console.inner.forgotten.lock().unwrap() == reads
    })
    .await;
    let outcomes = harness.outcomes();
    assert!(
        outcomes.contains(&format!("ask:workstream:{NODE}:sent")),
        "{outcomes:?}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn thread_replies_are_follow_ups_with_the_earlier_turns_again_and_again() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what is left today?", at));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("Two tasks: tests and docs."));
    harness.answered(at, 1).await;

    harness
        .socket
        .send(reply("env-2", "Ev2", "which tests?", at + 1, at));
    harness.prompted(2).await;
    let second = harness.prompts()[1].clone();
    assert!(second.starts_with(&format!("{SLACK_PROVENANCE} which tests?")));
    assert!(
        second.contains("Q: what is left today?\nA: Two tasks: tests and docs."),
        "{second}"
    );
    harness.answer("command-2", "answered", Some("The login tests."));
    harness.answered(at, 2).await;

    harness
        .socket
        .send(reply("env-3", "Ev3", "and docs?", at + 2, at));
    harness.prompted(3).await;
    let third = harness.prompts()[2].clone();
    let first_turn = third.find("Q: what is left today?").unwrap();
    let second_turn = third.find("Q: which tests?\nA: The login tests.").unwrap();
    assert!(first_turn < second_turn, "{third}");
    harness.answer("command-3", "answered", Some("The README."));
    harness.answered(at, 3).await;
    assert_eq!(harness.targets(), [node(), node(), node()]);
    assert!(harness.answers(at)[2].contains("The README."));
    // Every follow-up's progress and answer stayed in the thread.
    assert!(
        harness
            .slack
            .calls("chat.postMessage")
            .iter()
            .all(|call| call.body["thread_ts"] == format!("{at}.000100").as_str()),
        "{:?}",
        posted_texts(&harness.slack)
    );
    let outcomes = harness.outcomes();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == format!("follow_up:workstream:{NODE}:sent"))
            .count(),
        2,
        "{outcomes:?}"
    );
}

#[tokio::test]
async fn messages_while_the_answer_is_pending_are_queued_and_sent_in_order() {
    let harness = Harness::with_superintendent_relay(Some(timing(20_000, 20_000, 1_000))).await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "how is CI?", at));
    harness.prompted(1).await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "also the docs?", at + 1, at));
    harness.socket.send(reply(
        "env-3",
        "Ev3",
        "and then tag the release",
        at + 2,
        at,
    ));
    eventually("queued twice", || {
        harness
            .outcomes()
            .iter()
            .filter(|outcome| *outcome == "follow_up:-:queued")
            .count()
            == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let notices = texts(&thread_posts(&harness.slack, at))
        .into_iter()
        .filter(|text| text.starts_with("Queued — I'll send it after the current answer."))
        .count();
    assert_eq!(notices, 1, "told once, no nagging");
    assert_eq!(harness.prompts().len(), 1, "nothing sent while in flight");
    assert!(!harness.posted(at, "Nothing was sent"));
    // The answer arrives: the first queued message is the next turn.
    harness.answer("command-1", "answered", Some("Green."));
    harness.prompted(2).await;
    let second = harness.prompts()[1].clone();
    assert!(
        second.starts_with(&format!("{SLACK_PROVENANCE} also the docs?")),
        "{second}"
    );
    assert!(second.contains("Q: how is CI?\nA: Green."), "{second}");
    harness.answer("command-2", "answered", Some("Docs are current."));
    harness.prompted(3).await;
    let third = harness.prompts()[2].clone();
    assert!(
        third.starts_with(&format!("{SLACK_PROVENANCE} and then tag the release")),
        "{third}"
    );
    assert!(
        third.contains("Q: also the docs?\nA: Docs are current."),
        "{third}"
    );
    assert_eq!(harness.targets(), [node(), node(), node()]);
}

#[tokio::test]
async fn at_most_five_messages_wait_and_the_owner_is_told_when_one_is_not_queued() {
    let harness = Harness::with_superintendent_relay(Some(timing(20_000, 20_000, 1_000))).await;
    let at = now_s();
    harness.socket.send(message("env-0", "Ev0", "first", at));
    harness.prompted(1).await;
    for index in 1..=6 {
        harness.socket.send(reply(
            &format!("env-{index}"),
            &format!("Ev{index}"),
            &format!("message {index}"),
            at + index,
            at,
        ));
    }
    eventually("full", || {
        harness.posted(at, "Not queued: 5 messages already wait in this thread.")
    })
    .await;
    let outcomes = harness.outcomes();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| *outcome == "follow_up:-:queued")
            .count(),
        5,
        "{outcomes:?}"
    );
    assert!(outcomes.contains(&"follow_up:-:refused:queue_full".to_owned()));
    assert_eq!(harness.prompts().len(), 1);
}

#[tokio::test]
async fn the_main_superintendent_answers_when_the_slack_superintendent_cannot_start() {
    let harness = Harness::with_superintendent().await;
    *harness.console.inner.superintendent.lock().unwrap() = Some(Err(ConsoleError::Unavailable(
        "the Superintendent has no worker profile to copy".to_owned(),
    )));
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what is left?", at));
    harness.prompted(1).await;
    assert_eq!(harness.targets(), [AgentTarget::YardOrchestrator]);
    assert_eq!(
        harness.prompts()[0],
        crate::slack::relay::question_text("what is left?"),
        "today's prompt, no answer file"
    );
    harness.answer("command-1", "Two tasks.", None);
    harness.answered(at, 1).await;
    let answer = thread_posts(&harness.slack, at)
        .into_iter()
        .find(|post| {
            post["text"]
                .as_str()
                .unwrap_or_default()
                .starts_with("*Answer from")
        })
        .unwrap();
    assert!(
        answer["text"]
            .as_str()
            .unwrap()
            .starts_with("*Answer from the Superintendent*")
    );
    let blocks = answer["blocks"].to_string();
    assert!(
        blocks.contains("The Slack Superintendent could not take the question (the Superintendent has no worker profile to copy), so the main Superintendent answered."),
        "{blocks}"
    );
    assert!(
        harness
            .outcomes()
            .contains(&"ask:superintendent:sent:fallback".to_owned())
    );
    assert!(
        harness
            .console
            .inner
            .answer_reads
            .lock()
            .unwrap()
            .is_empty(),
        "no answer file for the main Superintendent"
    );
    // The next turn tries the Slack Superintendent again.
    *harness.console.inner.superintendent.lock().unwrap() = Some(Ok(node()));
    harness
        .socket
        .send(reply("env-2", "Ev2", "which ones?", at + 1, at));
    harness.prompted(2).await;
    assert_eq!(harness.targets()[1], node());
    assert!(harness.prompts()[1].contains("Q: what is left?\nA: Two tasks."));
}

#[tokio::test]
async fn without_an_answer_file_the_reports_last_is_relayed() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "is CI green?", at));
    harness.prompted(1).await;
    harness.answer("command-1", "Yes, all green.", None);
    harness.answered(at, 1).await;
    assert!(
        harness.answers(at)[0]
            .starts_with("*Answer from the Slack Superintendent* (working)\nYes, all green."),
        "{:?}",
        harness.answers(at)
    );
    harness
        .socket
        .send(reply("env-2", "Ev2", "and tomorrow?", at + 1, at));
    harness.prompted(2).await;
    assert!(harness.prompts()[1].contains("A: Yes, all green."));
}

fn project_id(harness: &Harness) -> String {
    rusqlite::Connection::open(harness.temp.path().join("yard.sqlite3"))
        .unwrap()
        .query_row("SELECT id FROM projects", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn project_conversations_follow_up_with_that_orchestrator_and_can_be_retargeted() {
    let harness = Harness::with_superintendent().await;
    let orchestrator = AgentTarget::ProjectOrchestrator {
        project_id: project_id(&harness),
    };
    let at = now_s();
    harness.socket.send(message(
        "env-1",
        "Ev1",
        "checkout &lt;!channel&gt;: what's left?",
        at,
    ));
    harness.prompted(1).await;
    assert_eq!(
        harness.prompts()[0],
        crate::slack::relay::question_text("what's left?")
    );
    harness.answer("command-1", "Two tasks.", None);
    harness.answered(at, 1).await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "which first?", at + 1, at));
    harness.prompted(2).await;
    assert_eq!(
        harness.targets(),
        [orchestrator.clone(), orchestrator.clone()]
    );
    assert!(harness.prompts()[1].contains("Q: what's left?\nA: Two tasks."));
    assert!(harness.prompts()[1].ends_with(crate::slack::relay::ANSWER_INSTRUCTION));
    // Naming the same project again is just a follow-up.
    harness.answer("command-2", "Tests.", None);
    harness.answered(at, 2).await;
    harness.socket.send(reply(
        "env-3",
        "Ev3",
        "Checkout &lt;!channel&gt;: and then?",
        at + 2,
        at,
    ));
    harness.prompted(3).await;
    assert!(harness.prompts()[2].contains("Q: which first?\nA: Tests."));
    assert!(!harness.posted(at, "Now talking to"));

    // A Superintendent conversation switched to the project mid-thread.
    harness.answer("command-3", "Docs.", None);
    harness.answered(at, 3).await;
    let root = at + 10;
    harness
        .socket
        .send(message("env-4", "Ev4", "anything blocked?", root));
    harness.prompted(4).await;
    assert_eq!(harness.targets()[3], node());
    harness.answer("command-4", "answered", Some("Nothing is blocked."));
    harness.answered(root, 1).await;
    harness.socket.send(reply(
        "env-5",
        "Ev5",
        "checkout &lt;!channel&gt;: and you?",
        root + 1,
        root,
    ));
    harness.prompted(5).await;
    assert_eq!(harness.targets()[4], orchestrator);
    assert!(harness.posted(
        root,
        "Now talking to the Checkout &lt;!channel&gt; orchestrator in this thread."
    ));
    assert!(
        !harness.prompts()[4].contains("Earlier in this Slack conversation"),
        "history reset"
    );
    assert!(harness.posted(
        root,
        "Now talking to the Checkout &lt;!channel&gt; orchestrator in this thread. Start a message with `Superintendent:` to switch back."
    ));
    // `Superintendent:` switches the thread back; later replies stay there.
    harness.answer("command-5", "Fine.", None);
    harness.answered(root, 2).await;
    harness.socket.send(reply(
        "env-6",
        "Ev6",
        "Superintendent: thanks, anything else?",
        root + 2,
        root,
    ));
    harness.prompted(6).await;
    assert_eq!(harness.targets()[5], node());
    assert!(
        harness.prompts()[5].starts_with(&format!("{SLACK_PROVENANCE} thanks, anything else?")),
        "{:?}",
        harness.prompts()
    );
    assert!(harness.posted(
        root,
        "Now talking to the Slack Superintendent in this thread."
    ));
    harness.answer("command-6", "answered", Some("No."));
    harness.answered(root, 3).await;
    harness
        .socket
        .send(reply("env-7", "Ev7", "ok", root + 3, root));
    harness.prompted(7).await;
    assert_eq!(harness.targets()[6], node());
}

#[tokio::test]
async fn keywords_in_a_conversation_thread_are_commands() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what is left?", at));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("Two tasks."));
    harness.answered(at, 1).await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "status", at + 1, at));
    eventually("status", || harness.posted(at, "*Yard status*")).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(harness.prompts().len(), 1, "a command never prompts");
}

#[tokio::test]
async fn a_reply_in_an_unknown_thread_is_adopted_and_sent() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    // A thread Yard has no conversation for (a status card, or one from
    // before this file existed): adopted, not refused. A statement with
    // no question mark is sent like any other message.
    harness.socket.send(reply(
        "env-1",
        "Ev1",
        "restart the flaky tests",
        at,
        at - 50,
    ));
    harness.prompted(1).await;
    assert_eq!(harness.targets(), [node()]);
    let prompt = harness.prompts()[0].clone();
    assert!(
        prompt.starts_with(&format!("{SLACK_PROVENANCE} restart the flaky tests")),
        "{prompt}"
    );
    assert!(
        prompt.contains("Yard has no record of what started it"),
        "{prompt}"
    );
    let outcomes = harness.outcomes();
    assert!(
        outcomes.contains(&"free_text:superintendent:adopted_thread".to_owned()),
        "{outcomes:?}"
    );
    assert!(
        outcomes.contains(&format!("follow_up:workstream:{NODE}:sent")),
        "{outcomes:?}"
    );
    assert!(
        !posted_texts(&harness.slack)
            .iter()
            .any(|text| text.starts_with("Nothing was sent"))
    );
    // It is a conversation now: the next reply carries the first turn.
    harness.answer("command-1", "answered", Some("Restarted 3 tests."));
    harness.answered(at - 50, 1).await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "thanks, which ones", at + 1, at - 50));
    harness.prompted(2).await;
    assert!(
        harness.prompts()[1].contains("Q: restart the flaky tests\nA: Restarted 3 tests."),
        "{}",
        harness.prompts()[1]
    );
}

#[tokio::test]
async fn someone_else_in_a_conversation_thread_is_dropped_before_routing() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what is left?", at));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("Two tasks."));
    harness.answered(at, 1).await;
    let posts = harness.slack.calls("chat.postMessage").len();
    for (envelope, pointer, value) in [
        ("env-2", "/payload/event/user", json!("U02OTHER")),
        ("env-3", "/payload/team_id", json!("T02OTHER")),
        ("env-4", "/payload/event/channel", json!("D02OTHER")),
    ] {
        let event = format!("Ev-{envelope}");
        harness.socket.send(with(
            reply(envelope, &event, "do something", at + 1, at),
            pointer,
            value,
        ));
    }
    eventually("dropped", || {
        audit_lines(&harness.temp)
            .iter()
            .filter(|line| line["kind"] == "dropped")
            .count()
            == 3
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(harness.prompts().len(), 1);
    assert_eq!(harness.slack.calls("chat.postMessage").len(), posts);
    // The conversation itself is untouched: the owner can still follow up.
    harness
        .socket
        .send(reply("env-5", "Ev5", "which first?", at + 2, at));
    harness.prompted(2).await;
    assert!(harness.prompts()[1].contains("Q: what is left?\nA: Two tasks."));
}

#[tokio::test]
async fn a_slow_start_never_blocks_other_commands() {
    let console =
        GatedConsole::with_superintendent_permits(crate::slack::prompt::tests::WORKING, 10_000, 0);
    *console.inner.superintendent.lock().unwrap() = Some(Ok(node()));
    let harness = Harness::start(console).await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what is left?", at));
    harness.socket.send(message("env-2", "Ev2", "help", at + 1));
    eventually("help", || !thread_posts(&harness.slack, at + 1).is_empty()).await;
    assert!(harness.prompts().is_empty(), "still starting");
    eventually("starting notice", || {
        harness.posted(at, "Starting the Slack Superintendent…")
    })
    .await;
    harness.console.superintendent_gate.add_permits(1);
    harness.prompted(1).await;
    assert_eq!(harness.targets(), [node()]);
    // The notice became the progress message (no second card).
    eventually("updated", || {
        harness.slack.calls("chat.update").iter().any(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Asking the Slack Superintendent…")
        })
    })
    .await;
    assert!(!harness.posted(at, "Asking the Slack Superintendent…"));
}

#[tokio::test]
async fn follow_up_answers_are_never_held_by_quiet_hours() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    slack.set_socket_url(&socket.url);
    let clock =
        crate::slack::policy::tests::TestClock::at(crate::slack::policy::tests::WORKDAY_EVENING);
    let notifier = inbound_notifier_relay(
        &temp,
        &slack,
        inbound_settings(enabled()),
        0,
        crate::slack::tests::instant_timing(),
        fast_relay(),
        crate::slack::policy::tests::parts(&clock, Some(&temp)),
    )
    .await;
    tokio::spawn(notifier.clone().run_inbound());
    eventually("hello", || {
        notifier.inbound_status().status == InboundStatusKind::Connected
    })
    .await;
    let console = GatedConsole::new(crate::slack::prompt::tests::WORKING, 10_000);
    set_blocked(&console, false);
    *console.inner.superintendent.lock().unwrap() = Some(Ok(node()));
    notifier.attach_console(console.clone());
    assert!(notifier.status().quiet.unwrap().active);

    let at = now_s();
    socket.send(message("env-1", "Ev1", "what is left?", at));
    eventually("prompt", || {
        console.inner.prompts.lock().unwrap().len() == 1
    })
    .await;
    *console.inner.answer_file.lock().unwrap() = Some("Two tasks.".to_owned());
    *console.inner.screen.lock().unwrap() = answer_screen("command-1", "answered");
    eventually("answer", || {
        texts(&thread_posts(&slack, at))
            .iter()
            .any(|text| text.starts_with("*Answer from"))
    })
    .await;
    socket.send(reply("env-2", "Ev2", "which first?", at + 1, at));
    eventually("follow-up", || {
        console.inner.prompts.lock().unwrap().len() == 2
    })
    .await;
    *console.inner.answer_file.lock().unwrap() = Some("Tests.".to_owned());
    *console.inner.screen.lock().unwrap() = answer_screen("command-2", "answered");
    eventually("second answer", || {
        texts(&thread_posts(&slack, at))
            .iter()
            .filter(|text| text.starts_with("*Answer from"))
            .count()
            == 2
    })
    .await;
    assert_eq!(
        notifier.status().quiet.unwrap().held_count,
        0,
        "nothing held"
    );
}

#[test]
fn help_and_the_ask_hint_mention_follow_ups() {
    let help = crate::slack::commands::help_text();
    assert!(help.contains("you can reply there to follow up"), "{help}");
    assert!(help.contains("at the top level or in any thread"), "{help}");
    assert!(help.contains("is queued and sent next"), "{help}");
    assert!(!help.contains("not sent"), "{help}");
    let hint = crate::slack::views::ask_hint();
    assert!(hint.text.contains("here or in any thread"), "{}", hint.text);
    assert!(
        hint.text
            .contains("reply there to keep the conversation going")
    );
}

/// Watch timings with a long window and the given busy bound.
fn timing(follow_ms: u64, chat_ms: u64, late_ms: u64) -> crate::slack::relay::RelayTiming {
    crate::slack::relay::RelayTiming {
        follow: Duration::from_millis(follow_ms),
        chat: Duration::from_millis(chat_ms),
        late: Duration::from_millis(late_ms),
        late_poll: Duration::from_millis(50),
        ..fast_relay()
    }
}

#[tokio::test]
async fn an_unanswered_turn_holds_its_thread_only_briefly_and_its_late_answer_is_kept() {
    let harness = Harness::with_superintendent_relay(Some(timing(20_000, 700, 1_000))).await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "Q1 status?", at));
    harness.prompted(1).await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "Q2 while waiting", at + 1, at));
    eventually("queued", || {
        harness.posted(at, "Queued — I'll send it after the current answer.")
    })
    .await;
    // The agent never reports; past the busy bound the queued reply is
    // sent by itself (still inside the 20 s window).
    harness.prompted(2).await;
    let second = harness.prompts()[1].clone();
    assert!(
        second.starts_with(&format!("{SLACK_PROVENANCE} Q2 while waiting")),
        "{second}"
    );
    assert!(
        second.contains("Q: Q1 status?\nA: (no answer relayed)"),
        "{second}"
    );
    // The first answer still arrives: relayed and remembered.
    harness.answer("command-1", "answered", Some("A1 late"));
    harness.answered(at, 1).await;
    harness.answer("command-2", "answered", Some("A2"));
    harness.answered(at, 2).await;
    harness
        .socket
        .send(reply("env-4", "Ev4", "Q4 thanks", at + 3, at));
    harness.prompted(3).await;
    let prompt = harness.prompts()[2].clone();
    assert!(prompt.contains("Q: Q1 status?\nA: A1 late"), "{prompt}");
    assert!(prompt.contains("Q: Q2 while waiting\nA: A2"), "{prompt}");
}

#[tokio::test]
async fn after_the_window_a_follow_up_is_sent_and_a_late_answer_joins_the_history() {
    let harness = Harness::with_superintendent_relay(Some(timing(900, 60_000, 5_000))).await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "Q1 deploy?", at));
    harness.prompted(1).await;
    eventually("timed out", || {
        update_with(
            &harness.slack,
            "No answer from the Slack Superintendent after",
        )
        .is_some()
    })
    .await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "Q2 still there?", at + 1, at));
    harness.prompted(2).await;
    assert!(harness.prompts()[1].contains("Q: Q1 deploy?\nA: (no answer relayed)"));
    harness.answer("command-1", "answered", Some("A1 late"));
    harness.answered(at, 1).await;
    // Audited just after it is posted.
    eventually("relayed late", || {
        harness
            .outcomes()
            .contains(&format!("relay:workstream:{NODE}:relayed_late"))
    })
    .await;
    harness.answer("command-2", "answered", Some("A2"));
    harness.answered(at, 2).await;
    harness
        .socket
        .send(reply("env-3", "Ev3", "Q3 ok", at + 2, at));
    harness.prompted(3).await;
    assert!(harness.prompts()[2].contains("Q: Q1 deploy?\nA: A1 late"));
}

#[tokio::test]
async fn a_blocked_slack_superintendent_keeps_the_message_and_sends_it_once_free() {
    let harness = Harness::with_superintendent_relay(Some(timing(20_000, 20_000, 1_000))).await;
    let at = now_s();
    harness.socket.send(message("env-1", "Ev1", "Q1 plan?", at));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("A1"));
    harness.answered(at, 1).await;
    // On a prompt: said once, with the prompt card; not sent, not refused,
    // no fallback.
    *harness.console.inner.screen.lock().unwrap() =
        crate::slack::prompt::tests::CLAUDE_PERMISSION.to_owned();
    set_blocked(&harness.console, true);
    harness
        .socket
        .send(reply("env-2", "Ev2", "Q2 and next?", at + 1, at));
    eventually("waiting", || {
        update_with(
            &harness.slack,
            "Waiting: The Slack Superintendent is waiting on a prompt",
        )
        .is_some()
    })
    .await;
    eventually("prompt card", || {
        thread_posts(&harness.slack, at).len() >= 4
    })
    .await;
    // A message behind it waits in order.
    harness
        .socket
        .send(reply("env-3", "Ev3", "Q3 after that", at + 2, at));
    eventually("queued", || {
        harness
            .outcomes()
            .contains(&"follow_up:-:queued".to_owned())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let outcomes = harness.outcomes();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.ends_with(":waiting:blocked"))
            .count(),
        1,
        "{outcomes:?}"
    );
    assert!(
        !outcomes
            .iter()
            .any(|outcome| outcome.contains("refused") || outcome.contains("fallback")),
        "{outcomes:?}"
    );
    assert!(!harness.targets().contains(&AgentTarget::YardOrchestrator));
    assert_eq!(harness.prompts().len(), 1);
    let said = texts(&thread_posts(&harness.slack, at));
    assert!(
        !said.iter().any(|text| text.contains("was not sent")),
        "{said:?}"
    );
    let waiting = harness
        .slack
        .calls("chat.update")
        .into_iter()
        .filter(|call| {
            call.body["text"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Waiting:")
        })
        .count();
    assert_eq!(waiting, 1, "told once, though tried again since");

    // The prompt is answered (in Yard here): Q2 goes, once, with the
    // earlier turns, then Q3 after its answer.
    set_blocked(&harness.console, false);
    *harness.console.inner.screen.lock().unwrap() = crate::slack::prompt::tests::WORKING.to_owned();
    harness.prompted(2).await;
    let prompt = harness.prompts()[1].clone();
    assert!(prompt.contains("Q: Q1 plan?\nA: A1"), "{prompt}");
    assert!(
        !prompt.contains("Q: Q2"),
        "the unsent turn is not history: {prompt}"
    );
    assert_eq!(prompt.matches("Q2 and next?").count(), 1, "{prompt}");
    assert!(!prompt.contains("Q3 after that"), "{prompt}");
    assert_eq!(harness.targets()[1], node());
    assert!(
        harness
            .outcomes()
            .contains(&format!("follow_up:workstream:{NODE}:sent"))
    );
    harness.answer("command-2", "answered", Some("A2"));
    harness.prompted(3).await;
    assert!(
        harness.prompts()[2].contains(&format!("{SLACK_PROVENANCE} Q3 after that")),
        "{:?}",
        harness.prompts()
    );
}

#[tokio::test]
async fn an_old_retry_switches_the_thread_back_to_the_cards_agent() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness.socket.send(message("env-1", "Ev1", "Q1 plan?", at));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("A1"));
    harness.answered(at, 1).await;
    // Typing it in failed: the card offers Retry.
    *harness.console.inner.prompt_error.lock().unwrap() =
        Some(ConsoleError::Failed("the pane write failed".to_owned()));
    harness
        .socket
        .send(reply("env-2", "Ev2", "Q2 later?", at + 1, at));
    eventually("failed", || {
        update_with(&harness.slack, "can't answer right now").is_some()
    })
    .await;
    let card = update_with(&harness.slack, "can't answer right now").unwrap();
    *harness.console.inner.prompt_error.lock().unwrap() = None;
    harness.socket.send(reply(
        "env-3",
        "Ev3",
        "checkout &lt;!channel&gt;: what's left?",
        at + 2,
        at,
    ));
    harness.prompted(2).await;
    harness.answer("command-2", "answered", Some("Two tasks."));
    harness.answered(at, 2).await;
    // Retry is not refused: the thread switches back to the card's agent.
    let retry = button_value(&card, "Retry");
    harness.socket.send(thread_click("env-4", &retry, at));
    harness.prompted(3).await;
    assert_eq!(harness.targets()[2], node());
    assert!(harness.posted(
        at,
        "Now talking to the Slack Superintendent in this thread."
    ));
    let third = harness.prompts()[2].clone();
    assert!(
        third.starts_with(&format!("{SLACK_PROVENANCE} Q2 later?")),
        "{third}"
    );
    assert!(
        !third.contains("what's left?"),
        "switching drops the other agent's turns: {third}"
    );
    assert!(!harness.posted(at, "Nothing was sent"));
}

#[tokio::test]
async fn check_again_relays_the_answer_file_and_keeps_the_fallback_note() {
    let harness = Harness::with_superintendent_relay(Some(timing(600, 60_000, 150))).await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "Q1 deploy?", at));
    harness.prompted(1).await;
    let needle = "No answer from the Slack Superintendent after";
    eventually("timed out", || {
        update_with(&harness.slack, needle).is_some()
    })
    .await;
    let timed_out = update_with(&harness.slack, needle).unwrap();
    // The late window ends too; then the agent writes its file and reports.
    tokio::time::sleep(Duration::from_millis(500)).await;
    harness.answer("command-1", "short last", Some("The full answer file."));
    let check = button_value(&timed_out, "Check again");
    harness.socket.send(thread_click("env-2", &check, at));
    harness.answered(at, 1).await;
    assert!(harness.answers(at)[0].contains("The full answer file."));
    let key = harness.console.inner.answer_reads.lock().unwrap()[0].clone();
    assert!(harness.prompts()[0].contains(&answer_file(&key)));
    assert_eq!(*harness.console.inner.forgotten.lock().unwrap(), [key]);
    assert!(
        harness
            .outcomes()
            .contains(&format!("relay:workstream:{NODE}:relayed_on_check"))
    );

    // A fallback turn answered only after its window: the note stays.
    *harness.console.inner.superintendent.lock().unwrap() = Some(Err(ConsoleError::Unavailable(
        "its worker did not start".to_owned(),
    )));
    let root = at + 10;
    harness
        .socket
        .send(message("env-3", "Ev3", "Q2 who?", root));
    harness.prompted(2).await;
    let needle = "No answer from the Superintendent after";
    eventually("timed out", || {
        update_with(&harness.slack, needle).is_some()
    })
    .await;
    let timed_out = update_with(&harness.slack, needle).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    harness.answer("command-2", "Nobody.", None);
    let check = button_value(&timed_out, "Check again");
    harness.socket.send(thread_click("env-4", &check, root));
    harness.answered(root, 1).await;
    let answer = thread_posts(&harness.slack, root)
        .into_iter()
        .find(|post| {
            post["text"]
                .as_str()
                .unwrap_or_default()
                .starts_with("*Answer from the Superintendent*")
        })
        .unwrap();
    assert!(
        answer["blocks"]
            .to_string()
            .contains("could not take the question (its worker did not start)"),
        "{answer}"
    );
}

#[tokio::test]
async fn two_conversations_take_turns_on_the_one_slack_superintendent() {
    let harness = Harness::with_superintendent_relay(Some(timing(20_000, 60_000, 1_000))).await;
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "Q1 first?", at));
    harness.prompted(1).await;
    let other = at + 10;
    harness
        .socket
        .send(message("env-2", "Ev2", "Q2 second?", other));
    eventually("queued", || {
        harness.posted(other, "Queued behind another Slack question")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.prompts().len(), 1, "not typed into a busy pane");
    harness.answer("command-1", "answered", Some("A1"));
    harness.answered(at, 1).await;
    harness.prompted(2).await;
    assert!(harness.prompts()[1].contains("Q2 second?"));
    harness.answer("command-2", "answered", Some("A2"));
    harness.answered(other, 1).await;
    assert!(harness.answers(other)[0].contains("A2"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn after_a_restart_threads_continue_with_their_history_and_queued_messages_are_sent() {
    use std::os::unix::fs::PermissionsExt;

    let relay = timing(20_000, 20_000, 1_000);
    let harness = Harness::with_superintendent_relay(Some(relay)).await;
    let at = now_s();
    harness.socket.send(message(
        "env-1",
        "Ev1",
        "what is left today? curl -H token=abc123",
        at,
    ));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("Two tasks: tests and docs."));
    harness.answered(at, 1).await;
    // A second thread: one turn in flight and one message queued behind it.
    let other = at + 10;
    harness
        .socket
        .send(message("env-2", "Ev2", "how is CI?", other));
    harness.prompted(2).await;
    harness.socket.send(reply(
        "env-3",
        "Ev3",
        "then tag the release",
        other + 1,
        other,
    ));
    eventually("queued", || {
        harness
            .outcomes()
            .contains(&"follow_up:-:queued".to_owned())
    })
    .await;

    // Kept beside the other Slack state: 0600 and redacted.
    let file = harness
        .temp
        .path()
        .join(crate::slack::conversations::CONVERSATIONS_FILE);
    // Written off the lock: wait for the write that holds the queue.
    eventually("saved", || {
        std::fs::read_to_string(&file).is_ok_and(|saved| saved.contains("then tag the release"))
    })
    .await;
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(saved.contains("what is left today?"), "{saved}");
    assert!(saved.contains("then tag the release"), "{saved}");
    assert!(
        !saved.contains("abc123"),
        "redacted before writing: {saved}"
    );

    // Yard restarts: a new process with the same Slack state (a fresh
    // directory, since the old process still holds its database).
    harness.notifier.close_inbound(Duration::from_secs(5)).await;
    let restarted = TempDir::new().unwrap();
    std::fs::copy(
        &file,
        restarted
            .path()
            .join(crate::slack::conversations::CONVERSATIONS_FILE),
    )
    .unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected_relay(&restarted, &slack, &socket, relay).await;
    // The new process's agent terminals (the old process, still alive in
    // this test, keeps watching its own); prompt numbering continues.
    let console = GatedConsole::new(crate::slack::prompt::tests::WORKING, 10_000);
    *console.inner.superintendent.lock().unwrap() = Some(Ok(node()));
    set_blocked(&console, false);
    console.inner.prompts.lock().unwrap().extend(
        harness
            .console
            .inner
            .prompts
            .lock()
            .unwrap()
            .iter()
            .cloned(),
    );
    console
        .inner
        .prompt_targets
        .lock()
        .unwrap()
        .extend(harness.targets());
    let prompts = || {
        console
            .inner
            .prompts
            .lock()
            .unwrap()
            .iter()
            .map(|(text, _)| text.clone())
            .collect::<Vec<_>>()
    };
    let answer = |command_id: &str, file: &str| {
        *console.inner.answer_file.lock().unwrap() = Some(file.to_owned());
        *console.inner.screen.lock().unwrap() = answer_screen(command_id, "answered");
    };
    notifier.attach_console(console.clone());

    // The turn in flight at the restart is watched again: its answer is
    // relayed by the new process, and only then is the queued message
    // sent, as that thread's next turn with that answer.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(prompts().len(), 2, "held by the turn in flight");
    answer("command-2", "CI is green.");
    eventually("rewatched answer", || {
        texts(&thread_posts(&slack, other))
            .iter()
            .any(|text| text.contains("CI is green."))
    })
    .await;
    eventually("resumed", || prompts().len() == 3).await;
    let resumed = prompts()[2].clone();
    assert!(
        resumed.starts_with(&format!("{SLACK_PROVENANCE} then tag the release")),
        "{resumed}"
    );
    assert!(
        resumed.contains("Q: how is CI?\nA: CI is green."),
        "{resumed}"
    );
    answer("command-3", "Tagged.");
    eventually("resumed answer", || {
        texts(&thread_posts(&slack, other))
            .iter()
            .any(|text| text.contains("Tagged."))
    })
    .await;

    // A follow-up in the first thread carries its turn from before.
    socket.send(reply("env-4", "Ev4", "which tests?", at + 30, at));
    eventually("follow-up", || prompts().len() == 4).await;
    let follow_up = prompts()[3].clone();
    assert!(
        follow_up.starts_with(&format!("{SLACK_PROVENANCE} which tests?")),
        "{follow_up}"
    );
    assert!(
        follow_up.contains("A: Two tasks: tests and docs."),
        "{follow_up}"
    );
    assert!(
        !follow_up.contains("Yard has no record of what started it"),
        "a known thread, not adopted: {follow_up}"
    );
    assert_eq!(
        *console.inner.prompt_targets.lock().unwrap(),
        [node(), node(), node(), node()]
    );
    assert!(
        outcomes(&restarted).contains(&format!("relay:workstream:{NODE}:rewatch")),
        "{:?}",
        outcomes(&restarted)
    );
    assert!(
        !posted_texts(&slack)
            .iter()
            .any(|text| text.starts_with("Nothing was sent")),
        "{:?}",
        posted_texts(&slack)
    );
}

#[tokio::test]
async fn a_gone_superintendent_is_started_and_then_the_message_is_sent() {
    use crate::slack::actions::AgentHealth;

    let console = GatedConsole::new(crate::slack::prompt::tests::WORKING, 10_000);
    let harness = Harness::start(console).await;
    set_health(
        &harness.console,
        AgentHealth::NotRunning("its agent process exited".to_owned()),
    );
    *harness.console.inner.recovered.lock().unwrap() = Some(Ok(AgentTarget::YardOrchestrator));
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "assign the docs task to Atlas", at));
    harness.prompted(1).await;
    assert_eq!(
        *harness.console.inner.recover_calls.lock().unwrap(),
        [AgentTarget::YardOrchestrator]
    );
    assert_eq!(harness.targets(), [AgentTarget::YardOrchestrator]);
    assert!(
        harness.prompts()[0]
            .starts_with(&format!("{SLACK_PROVENANCE} assign the docs task to Atlas")),
        "{:?}",
        harness.prompts()
    );
    let said = posted_texts(&harness.slack)
        .into_iter()
        .chain(
            harness
                .slack
                .calls("chat.update")
                .into_iter()
                .filter_map(|call| call.body["text"].as_str().map(str::to_owned)),
        )
        .collect::<Vec<_>>();
    assert!(
        said.iter()
            .any(|text| text.starts_with("Starting the Superintendent…")),
        "{said:?}"
    );
    let outcomes = harness.outcomes();
    let starting = outcomes
        .iter()
        .position(|outcome| outcome == "ask:superintendent:starting");
    let sent = outcomes
        .iter()
        .position(|outcome| outcome == "ask:superintendent:sent");
    assert!(
        matches!((starting, sent), (Some(a), Some(b)) if a < b),
        "{outcomes:?}"
    );
    assert!(
        !outcomes.iter().any(|outcome| outcome.contains("refused")),
        "{outcomes:?}"
    );
}

#[tokio::test]
async fn a_disconnected_slack_superintendent_is_started_again_and_then_asked() {
    let harness = Harness::with_superintendent().await;
    let fresh = AgentTarget::CoordinationNode {
        node_id: "0190aaaa-0000-7000-8000-00000000000b".to_owned(),
    };
    harness.console.inner.screens.lock().unwrap().push((
        node(),
        "■ The app-server session could not be restored.\n  Disconnected from this task."
            .to_owned(),
    ));
    *harness.console.inner.recovered.lock().unwrap() = Some(Ok(fresh.clone()));
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what changed today?", at));
    harness.prompted(1).await;
    assert_eq!(
        *harness.console.inner.recover_calls.lock().unwrap(),
        [node()]
    );
    assert_eq!(harness.targets(), [fresh]);
    let outcomes = harness.outcomes();
    assert!(
        outcomes.contains(&format!("ask:workstream:{NODE}:starting")),
        "{outcomes:?}"
    );
    assert!(
        !outcomes
            .iter()
            .any(|outcome| outcome.contains("refused") || outcome.contains("fallback")),
        "{outcomes:?}"
    );
}

#[tokio::test]
async fn when_the_slack_superintendent_cannot_be_started_again_the_main_one_answers() {
    let harness = Harness::with_superintendent().await;
    // Only the Slack Superintendent's terminal shows a lost session, and
    // Yard can't start it again (this console can't).
    harness.console.inner.screens.lock().unwrap().push((
        node(),
        "■ The app-server session could not be restored.\n  Disconnected from this task."
            .to_owned(),
    ));
    let at = now_s();
    harness
        .socket
        .send(message("env-1", "Ev1", "what changed today?", at));
    harness.prompted(1).await;
    assert_eq!(harness.targets(), [AgentTarget::YardOrchestrator]);
    assert!(
        harness.prompts()[0].starts_with(&format!("{SLACK_PROVENANCE} what changed today?")),
        "{:?}",
        harness.prompts()
    );
    harness.answer("command-1", "Two merges.", None);
    eventually("answer", || {
        texts(&thread_posts(&harness.slack, at))
            .iter()
            .any(|text| text.contains("Two merges."))
    })
    .await;
    let answer = thread_posts(&harness.slack, at)
        .into_iter()
        .find(|post| {
            post["text"]
                .as_str()
                .unwrap_or_default()
                .contains("Two merges.")
        })
        .unwrap();
    let blocks = answer["blocks"].to_string();
    assert!(
        blocks.contains("so the main Superintendent answered"),
        "{blocks}"
    );
    let outcomes = harness.outcomes();
    assert!(
        outcomes.contains(&"ask:superintendent:sent:fallback".to_owned()),
        "{outcomes:?}"
    );
    assert!(
        !outcomes.iter().any(|outcome| outcome.contains("refused")),
        "{outcomes:?}"
    );
    assert!(
        !harness.posted(at, "Your question"),
        "nothing says not sent"
    );
}

#[tokio::test]
async fn a_message_before_the_console_is_attached_is_held_and_then_sent() {
    let temp = TempDir::new().unwrap();
    let slack = MockSlack::start().await;
    let socket = MockSocket::start().await;
    let notifier = connected(&temp, &slack, &socket).await;
    let console = GatedConsole::new(crate::slack::prompt::tests::WORKING, 10_000);
    set_blocked(&console, false);
    let at = now_s();
    socket.send(message("env-1", "Ev1", "are we up?", at));
    eventually("held", || {
        outcomes(&temp).contains(&"not_ready:-:held".to_owned())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    notifier.attach_console(console.clone());
    eventually("sent", || !console.inner.prompts.lock().unwrap().is_empty()).await;
    assert!(
        console.inner.prompts.lock().unwrap()[0]
            .0
            .contains("are we up?")
    );
    assert!(
        !audit_lines(&temp)
            .iter()
            .any(|line| line["outcome"] == "dropped" && line["action"] == "not_ready")
    );
}

#[tokio::test]
async fn sentences_that_start_with_a_command_word_are_messages() {
    let harness = Harness::with_superintendent().await;
    let at = now_s();
    harness.socket.send(message(
        "env-1",
        "Ev1",
        "Status of the deploy after the restart?",
        at,
    ));
    harness.prompted(1).await;
    harness.answer("command-1", "answered", Some("Deployed."));
    harness.answered(at, 1).await;
    harness.socket.send(reply(
        "env-2",
        "Ev2",
        "Mute the noisy CI alerts tonight",
        at + 1,
        at,
    ));
    harness.prompted(2).await;
    assert!(harness.prompts()[1].contains("Mute the noisy CI alerts tonight"));
    assert!(!harness.posted(at, "No project matches"));
    assert!(
        !harness
            .outcomes()
            .iter()
            .any(|outcome| outcome.contains("refused")),
        "{:?}",
        harness.outcomes()
    );
}

#[tokio::test]
async fn an_unwatched_turn_still_holds_its_thread_for_a_while() {
    let harness = Harness::with_superintendent_relay(Some(timing(20_000, 4_000, 1_000))).await;
    harness.notifier.fill_watchers();
    let at = now_s();
    harness.socket.send(message("env-1", "Ev1", "Q1?", at));
    harness.prompted(1).await;
    harness
        .socket
        .send(reply("env-2", "Ev2", "Q2?", at + 1, at));
    eventually("queued", || {
        harness
            .outcomes()
            .contains(&"follow_up:-:queued".to_owned())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        harness.prompts().len(),
        1,
        "not typed into it while it answers"
    );
    // After its busy bound the next message goes.
    harness.prompted(2).await;
}

#[tokio::test]
async fn a_reply_under_an_open_prompt_card_tells_the_superintendent_about_it() {
    let harness = Harness::with_superintendent().await;
    set_blocked(&harness.console, true);
    *harness.console.inner.screen.lock().unwrap() =
        crate::slack::prompt::tests::CLAUDE_PERMISSION.to_owned();
    let target = crate::slack::actions::tests::target();
    harness
        .notifier
        .post_prompt_card(&target, "Worker on Fix login", None)
        .await
        .unwrap();
    let card = harness
        .slack
        .calls("chat.postMessage")
        .last()
        .map(|call| call.body.clone())
        .unwrap();
    assert!(
        card["text"]
            .as_str()
            .unwrap()
            .contains("Worker on Fix login")
    );
    // The first post: the card is its own thread root.
    let card_ts = "1700000000.000001";
    set_blocked(&harness.console, false);
    *harness.console.inner.screen.lock().unwrap() = crate::slack::prompt::tests::WORKING.to_owned();
    harness.socket.send(with(
        message("env-1", "Ev1", "go ahead with it", now_s()),
        "/payload/event/thread_ts",
        json!(card_ts),
    ));
    harness.prompted(1).await;
    let prompt = harness.prompts()[0].clone();
    assert!(
        prompt.contains("It holds a prompt card for Worker on Fix login, still open"),
        "{prompt}"
    );
    assert_eq!(harness.targets(), [node()]);
}

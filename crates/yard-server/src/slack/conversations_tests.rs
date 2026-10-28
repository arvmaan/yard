use super::{
    ConversationAgent, Conversations, IDLE_TTL, KEEP_TURNS, MAX_CONVERSATIONS, NotBegun, TURN_CHARS,
};
use crate::slack::actions::AgentTarget;

const NOW: u64 = 1_800_000_000_000;

fn superintendent(conversations: &mut Conversations, root: &str, at: u64) {
    conversations.start(
        root,
        ConversationAgent::SlackSuperintendent,
        "the Slack Superintendent",
        at,
    );
}

#[test]
fn a_started_conversation_is_found_by_its_thread_root() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "100.1", NOW);
    assert!(conversations.contains("100.1", NOW));
    assert!(!conversations.contains("100.2", NOW));
    assert_eq!(
        conversations.agent("100.1", NOW),
        Some((
            ConversationAgent::SlackSuperintendent,
            "the Slack Superintendent".to_owned()
        ))
    );
    assert_eq!(
        conversations.begin("100.2", "q", NOW),
        Err(NotBegun::Unknown)
    );
}

#[test]
fn one_turn_runs_at_a_time_and_history_is_replayed_in_order() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    let first = conversations.begin("root", "Q1", NOW).unwrap();
    assert!(first.history.is_empty());
    assert_eq!(
        conversations.begin("root", "Q2", NOW),
        Err(NotBegun::Busy),
        "the first turn is still in flight"
    );
    conversations.sent("root", first.token, delivered("command-1"));
    conversations.answered("command-1", "A1");
    // The answer ends the turn even before its watch does.
    let second = conversations.begin("root", "Q2", NOW).unwrap();
    conversations.finish("root", first.token);
    assert_eq!(
        conversations.begin("root", "Q2", NOW),
        Err(NotBegun::Busy),
        "the old watch ending never ends the new turn"
    );
    assert_eq!(second.history.len(), 1);
    assert_eq!(second.history[0].question, "Q1");
    assert_eq!(second.history[0].answer.as_deref(), Some("A1"));
    conversations.sent("root", second.token, delivered("command-2"));
    // The window ended without an answer: the turn stays, unanswered.
    conversations.finish("root", second.token);
    let third = conversations.begin("root", "Q3", NOW).unwrap();
    let questions = third
        .history
        .iter()
        .map(|turn| turn.question.as_str())
        .collect::<Vec<_>>();
    assert_eq!(questions, ["Q1", "Q2"]);
    assert_eq!(third.history[1].answer, None);
    // A late answer still lands on its own turn.
    conversations.answered("command-2", "A2 late");
    conversations.finish("root", third.token);
    let fourth = conversations.begin("root", "Q4", NOW).unwrap();
    assert_eq!(fourth.history[1].answer.as_deref(), Some("A2 late"));
    assert_eq!(fourth.history.len(), 2, "the unsent third turn was dropped");
}

#[test]
fn turns_and_text_are_capped() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    for index in 0..10 {
        let begun = conversations
            .begin("root", &format!("Q{index}"), NOW)
            .unwrap();
        conversations.sent("root", begun.token, delivered(&format!("command-{index}")));
        conversations.finish("root", begun.token);
    }
    let begun = conversations
        .begin("root", &"x".repeat(5_000), NOW)
        .unwrap();
    assert_eq!(begun.history.len(), KEEP_TURNS);
    assert_eq!(begun.history[0].question, "Q4", "oldest dropped first");
    conversations.sent("root", begun.token, delivered("command-long"));
    conversations.answered("command-long", &"y".repeat(5_000));
    conversations.finish("root", begun.token);
    let next = conversations.begin("root", "next", NOW).unwrap();
    let last = next.history.last().unwrap();
    assert_eq!(last.question.chars().count(), TURN_CHARS);
    assert_eq!(last.answer.as_ref().unwrap().chars().count(), TURN_CHARS);
}

#[test]
fn the_least_recently_used_conversation_is_evicted() {
    let mut conversations = Conversations::default();
    for index in 0..MAX_CONVERSATIONS {
        let at = NOW + u64::try_from(index).unwrap();
        superintendent(&mut conversations, &format!("root-{index}"), at);
    }
    // Using root-0 makes root-1 the least recently used.
    conversations.begin("root-0", "q", NOW + 1_000).unwrap();
    superintendent(&mut conversations, "root-new", NOW + 2_000);
    assert!(conversations.contains("root-0", NOW + 2_000));
    assert!(!conversations.contains("root-1", NOW + 2_000));
    assert!(conversations.contains("root-new", NOW + 2_000));
}

#[test]
fn an_idle_conversation_expires() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    let ttl = u64::try_from(IDLE_TTL.as_millis()).unwrap();
    assert!(conversations.contains("root", NOW + ttl - 1));
    assert!(!conversations.contains("root", NOW + ttl));
}

#[test]
fn retargeting_resets_history() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    let begun = conversations.begin("root", "Q1", NOW).unwrap();
    conversations.sent("root", begun.token, delivered("command-1"));
    conversations.finish("root", begun.token);
    let orchestrator = ConversationAgent::Fixed(AgentTarget::ProjectOrchestrator {
        project_id: "project-1".to_owned(),
    });
    conversations.retarget("root", orchestrator.clone(), "the X orchestrator", NOW);
    let next = conversations.begin("root", "Q2", NOW).unwrap();
    assert!(next.history.is_empty());
    assert_eq!(next.agent, orchestrator);
    assert_eq!(next.to, "the X orchestrator");
}

#[test]
fn an_unanswered_turn_holds_its_thread_only_for_the_busy_bound() {
    let busy_for = std::time::Duration::from_secs(600);
    let bound = u64::try_from(busy_for.as_millis()).unwrap();
    let mut conversations = Conversations::new(busy_for);
    superintendent(&mut conversations, "root", NOW);
    let first = conversations.begin("root", "Q1", NOW).unwrap();
    conversations.sent("root", first.token, delivered("command-1"));
    assert!(conversations.busy("root", NOW + bound - 1));
    assert_eq!(
        conversations.begin("root", "Q2", NOW + bound - 1),
        Err(NotBegun::Busy)
    );
    // No answer within the bound: the next reply supersedes the turn.
    assert!(!conversations.busy("root", NOW + bound));
    let second = conversations.begin("root", "Q2", NOW + bound).unwrap();
    assert_eq!(second.history.len(), 1);
    assert_eq!(second.history[0].answer, None, "(no answer relayed)");
    // The old watch ending, or its late answer, never frees the new turn.
    conversations.finish("root", first.token);
    conversations.answered("command-1", "A1 late");
    assert_eq!(
        conversations.begin("root", "Q3", NOW + bound + 1),
        Err(NotBegun::Busy)
    );
    conversations.sent("root", second.token, delivered("command-2"));
    conversations.answered("command-2", "A2");
    let third = conversations.begin("root", "Q3", NOW + bound + 2).unwrap();
    let answers = third
        .history
        .iter()
        .map(|turn| turn.answer.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(answers, [Some("A1 late"), Some("A2")]);
}

fn queued(question: &str, at: u64) -> super::Queued {
    super::Queued {
        question: question.to_owned(),
        agent: None,
        user: "U01OWNER".to_owned(),
        kind: super::TurnKind::FollowUp,
        queued_at_unix_ms: at,
        note: None,
        redacted: false,
    }
}

fn delivered(command_id: &str) -> super::Delivered {
    super::Delivered {
        command_id: command_id.to_owned(),
        target: AgentTarget::YardOrchestrator,
        to: "the Superintendent".to_owned(),
        answer_key: None,
        progress_ts: Some("200.1".to_owned()),
        user: "U01OWNER".to_owned(),
        note: None,
    }
}

fn owner(user: &str) -> super::Owner {
    super::Owner {
        team_id: "T01TEAM".to_owned(),
        channel_id: "D01DM".to_owned(),
        user_id: user.to_owned(),
    }
}

const BUSY: std::time::Duration = std::time::Duration::from_secs(600);

#[test]
fn conversations_survive_a_reload_redacted_with_their_queue_and_context() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join(super::CONVERSATIONS_FILE);
    let mut conversations = Conversations::load(Some(file.clone()), BUSY, NOW);
    conversations.adopt(
        "root",
        ConversationAgent::SlackSuperintendent,
        "the Slack Superintendent",
        Some("A status card.".to_owned()),
        NOW,
    );
    conversations
        .enqueue("root", queued("Q1 curl -H token=abc123", NOW), NOW)
        .unwrap();
    let first = conversations.next("root", NOW).unwrap();
    conversations.sent("root", first.begun.token, delivered("command-1"));
    conversations.answered("command-1", "A1 done");
    let second = conversations
        .enqueue("root", queued("Q2", NOW + 1), NOW + 1)
        .unwrap();
    assert_eq!(second, super::Enqueued::Ready);
    let begun = conversations.next("root", NOW + 1).unwrap();
    conversations.sent("root", begun.begun.token, delivered("command-2"));
    assert_eq!(
        conversations.enqueue("root", queued("Q3", NOW + 2), NOW + 2),
        Ok(super::Enqueued::Waiting { notify: true })
    );
    // Begun but never sent: not kept.
    conversations.adopt(
        "other",
        ConversationAgent::Fixed(AgentTarget::YardOrchestrator),
        "the Superintendent",
        None,
        NOW,
    );
    conversations
        .enqueue("other", queued("never sent", NOW), NOW)
        .unwrap();
    conversations.next("other", NOW).unwrap();
    drop(conversations);

    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(!saved.contains("abc123"), "{saved}");
    let names = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, [super::CONVERSATIONS_FILE], "no temporary left");

    let mut reloaded = Conversations::load(Some(file.clone()), BUSY, NOW + 3);
    assert!(reloaded.contains("root", NOW + 3));
    // The turn in flight at the restart still holds its thread and is
    // watched again; once answered, the queued message is the next turn
    // with both earlier turns.
    let resumed = reloaded.resume(NOW + 3, BUSY);
    assert_eq!(resumed.waiting, ["root"]);
    assert!(resumed.dropped.is_empty());
    assert_eq!(resumed.rewatch.len(), 1);
    let rewatch = &resumed.rewatch[0];
    assert_eq!(
        (rewatch.root.as_str(), rewatch.question.as_str()),
        ("root", "Q2")
    );
    assert_eq!(rewatch.delivered, delivered("command-2"));
    assert_eq!(rewatch.since_unix_ms, NOW + 1);
    assert!(reloaded.next("root", NOW + 3).is_none(), "still held");
    reloaded.answered("command-2", "A2 after the restart");
    let next = reloaded.next("root", NOW + 3).unwrap();
    assert_eq!(next.queued.question, "Q3");
    assert_eq!(next.begun.context.as_deref(), Some("A status card."));
    let history = next.begun.history;
    assert_eq!(history.len(), 2);
    assert!(history[0].question.starts_with("Q1 curl -H token="));
    assert!(!history[0].question.contains("abc123"));
    assert_eq!(history[0].answer.as_deref(), Some("A1 done"));
    assert_eq!(
        (history[1].question.as_str(), history[1].answer.as_deref()),
        ("Q2", Some("A2 after the restart"))
    );
    assert!(!reloaded.has_turns("other"), "the unsent turn was dropped");
}

#[test]
fn old_conversations_and_old_queued_messages_are_not_resumed() {
    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join(super::CONVERSATIONS_FILE);
    let mut conversations = Conversations::load(Some(file.clone()), BUSY, NOW);
    superintendent(&mut conversations, "stale", NOW);
    superintendent(&mut conversations, "fresh", NOW + 1_000);
    let ttl = u64::try_from(IDLE_TTL.as_millis()).unwrap();
    conversations
        .enqueue("fresh", queued("old message", NOW + 1_000), NOW + 1_000)
        .unwrap();
    drop(conversations);

    // Reloaded just after the stale one's 7 days: it is gone.
    assert_eq!(IDLE_TTL, std::time::Duration::from_secs(7 * 24 * 60 * 60));
    let later = NOW + ttl + 500;
    let mut reloaded = Conversations::load(Some(file), BUSY, later);
    assert!(!reloaded.contains("stale", later));
    assert!(reloaded.contains("fresh", later));
    // Queued days ago: dropped (the owner is told), not sent now.
    let resumed = reloaded.resume(later, std::time::Duration::from_secs(1_800));
    assert!(resumed.waiting.is_empty());
    assert_eq!(resumed.dropped, [("fresh".to_owned(), 1)]);
    assert!(reloaded.next("fresh", later).is_none());
}

#[test]
fn a_corrupt_or_symlinked_conversations_file_starts_empty() {
    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join(super::CONVERSATIONS_FILE);
    std::fs::write(&file, "not json").unwrap();
    let mut conversations = Conversations::load(Some(file.clone()), BUSY, NOW);
    assert!(!conversations.contains("root", NOW));
    superintendent(&mut conversations, "root", NOW);
    assert!(Conversations::load(Some(file.clone()), BUSY, NOW).contains("root", NOW));

    let target = temp.path().join("elsewhere.json");
    std::fs::rename(&file, &target).unwrap();
    std::os::unix::fs::symlink(&target, &file).unwrap();
    let mut followed = Conversations::load(Some(file.clone()), BUSY, NOW);
    assert!(!followed.contains("root", NOW), "a symlink is not read");
    superintendent(&mut followed, "new", NOW);
    assert!(
        std::fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink(),
        "nor written through"
    );
}

#[test]
fn at_most_five_messages_wait_per_thread_and_the_owner_is_told_once() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    conversations
        .enqueue("root", queued("Q0", NOW), NOW)
        .unwrap();
    let first = conversations.next("root", NOW).unwrap();
    conversations.sent("root", first.begun.token, delivered("command-0"));
    let mut notices = 0;
    for index in 0..super::MAX_QUEUED {
        match conversations.enqueue("root", queued(&format!("Q{index}"), NOW), NOW) {
            Ok(super::Enqueued::Waiting { notify }) => notices += usize::from(notify),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(notices, 1);
    assert_eq!(
        conversations.enqueue("root", queued("one too many", NOW), NOW),
        Err(super::NotQueued::Full)
    );
}

#[test]
fn a_requeued_turn_never_drops_a_message_already_accepted() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    conversations
        .enqueue("root", queued("Q0", NOW), NOW)
        .unwrap();
    // Q0 is in flight but not sent yet (its agent is still being resolved),
    // so it keeps its slot: only MAX_QUEUED - 1 more are accepted and the
    // newest one is refused, visibly, instead of being dropped later.
    let first = conversations.next("root", NOW).unwrap();
    for index in 1..super::MAX_QUEUED {
        conversations
            .enqueue("root", queued(&format!("Q{index}"), NOW), NOW)
            .unwrap();
    }
    assert_eq!(
        conversations.enqueue("root", queued("Q5", NOW), NOW),
        Err(super::NotQueued::Full)
    );
    // The agent can't take Q0 yet: it goes back to the front, nothing lost.
    assert!(conversations.requeue(
        "root",
        first.begun.token,
        first.queued.clone(),
        None,
        NOW,
        NOW,
    ));
    let mut sent = Vec::new();
    for index in 0..super::MAX_QUEUED {
        let next = conversations.next("root", NOW + 1).unwrap();
        sent.push(next.queued.question.clone());
        conversations.sent(
            "root",
            next.begun.token,
            delivered(&format!("command-{index}")),
        );
        conversations.answered(&format!("command-{index}"), "A");
    }
    assert_eq!(sent, ["Q0", "Q1", "Q2", "Q3", "Q4"]);
    assert!(conversations.next("root", NOW + 2).is_none());
}

#[test]
fn a_queued_message_changed_by_redaction_is_marked_after_a_reload() {
    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join(super::CONVERSATIONS_FILE);
    let mut conversations = Conversations::load(Some(file.clone()), BUSY, NOW);
    superintendent(&mut conversations, "root", NOW);
    conversations
        .enqueue("root", queued("Q0", NOW), NOW)
        .unwrap();
    let first = conversations.next("root", NOW).unwrap();
    conversations
        .enqueue("root", queued("deploy with token=abc123", NOW), NOW)
        .unwrap();
    conversations
        .enqueue("root", queued("then tag it", NOW), NOW)
        .unwrap();
    // In memory the text is as typed.
    conversations.finish("root", first.begun.token);
    let next = conversations.next("root", NOW).unwrap();
    assert_eq!(next.queued.question, "deploy with token=abc123");
    assert!(!next.queued.redacted);
    conversations.finish("root", next.begun.token);
    conversations
        .enqueue("root", queued("rotate token=abc123", NOW), NOW)
        .unwrap();
    drop(conversations);

    let mut reloaded = Conversations::load(Some(file), BUSY, NOW + 1);
    let next = reloaded.next("root", NOW + 1).unwrap();
    assert_eq!(next.queued.question, "then tag it");
    assert!(!next.queued.redacted);
    reloaded.finish("root", next.begun.token);
    let next = reloaded.next("root", NOW + 1).unwrap();
    assert!(!next.queued.question.contains("abc123"));
    assert!(next.queued.redacted, "the owner is told");
}

#[test]
fn conversations_saved_for_another_owner_or_workspace_are_dropped() {
    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join(super::CONVERSATIONS_FILE);
    let mut conversations = Conversations::load(Some(file.clone()), BUSY, NOW);
    assert!(!conversations.bind(owner("U01OWNER")));
    superintendent(&mut conversations, "root", NOW);
    conversations
        .enqueue("root", queued("delete the old branches", NOW), NOW)
        .unwrap();
    drop(conversations);

    // The same owner: kept.
    let mut same = Conversations::load(Some(file.clone()), BUSY, NOW + 1);
    assert!(!same.is_bound());
    assert!(!same.bind(owner("U01OWNER")));
    assert!(same.is_bound());
    assert_eq!(same.resume(NOW + 1, BUSY).waiting, ["root"]);
    drop(same);

    // Another owner (or team, or DM channel): nothing is resumed, and the
    // file no longer holds the old owner's messages.
    let mut other = Conversations::load(Some(file.clone()), BUSY, NOW + 2);
    assert!(other.bind(owner("U02SOMEONE")));
    assert!(!other.contains("root", NOW + 2));
    assert_eq!(other.resume(NOW + 2, BUSY), super::Resumed::default());
    drop(other);
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(!saved.contains("delete the old branches"), "{saved}");
    let mut team = owner("U02SOMEONE");
    team.team_id = "T02OTHER".to_owned();
    let mut moved = Conversations::load(Some(file.clone()), BUSY, NOW + 3);
    superintendent(&mut moved, "again", NOW + 3);
    assert!(moved.bind(team));
    assert!(!moved.contains("again", NOW + 3));

    // A file that names no owner (written before it was recorded): dropped.
    std::fs::write(
        &file,
        r#"{"version":1,"conversations":[{"root":"legacy","agent":"slack_superintendent","to":"x","last_used_unix_ms":1800000000000}]}"#,
    )
    .unwrap();
    let mut legacy = Conversations::load(Some(file), BUSY, NOW + 4);
    assert!(legacy.contains("legacy", NOW + 4));
    assert!(legacy.bind(owner("U01OWNER")));
    assert!(!legacy.contains("legacy", NOW + 4));
}

#[test]
fn a_message_its_agent_cannot_take_yet_waits_first_in_line() {
    let mut conversations = Conversations::default();
    superintendent(&mut conversations, "root", NOW);
    conversations
        .enqueue("root", queued("Q1", NOW), NOW)
        .unwrap();
    let first = conversations.next("root", NOW).unwrap();
    assert!(first.retried.is_none());
    conversations
        .enqueue("root", queued("Q2", NOW), NOW)
        .unwrap();
    // On a prompt: Q1 goes back to the front, tried again later.
    assert!(conversations.requeue(
        "root",
        first.begun.token,
        first.queued.clone(),
        Some("300.1".to_owned()),
        NOW + 20_000,
        NOW + 1,
    ));
    assert!(
        !conversations.has_turns("root"),
        "the unsent turn is not history"
    );
    assert!(
        conversations.next("root", NOW + 2).is_none(),
        "not before its retry"
    );
    // A prompt answered from Slack wakes it at once.
    assert_eq!(conversations.wake(), ["root"]);
    let again = conversations.next("root", NOW + 3).unwrap();
    assert_eq!(again.queued.question, "Q1");
    let retried = again.retried.clone().unwrap();
    assert_eq!(retried.since_unix_ms, NOW + 1);
    assert_eq!(retried.progress_ts.as_deref(), Some("300.1"));
    // Still waiting: since and progress message kept.
    assert!(conversations.requeue(
        "root",
        again.begun.token,
        again.queued.clone(),
        None,
        NOW + 4,
        NOW + 4,
    ));
    let third = conversations.next("root", NOW + 5).unwrap();
    let retried = third.retried.clone().unwrap();
    assert_eq!(
        (retried.since_unix_ms, retried.progress_ts.as_deref()),
        (NOW + 1, Some("300.1"))
    );
    // Sent: the wait is over and Q2 follows in order.
    conversations.sent("root", third.begun.token, delivered("command-1"));
    conversations.answered("command-1", "A1");
    let next = conversations.next("root", NOW + 6).unwrap();
    assert_eq!(next.queued.question, "Q2");
    assert!(next.retried.is_none());
    // A stale token can't requeue.
    assert!(!conversations.requeue("root", 999, queued("x", NOW), None, NOW, NOW));
}

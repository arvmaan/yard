//! Board v3: the relevance window, Keep / Skip decisions, human titles,
//! the three card lines and helpers (neutral fixtures only).

use yard_domain::{ObservedStatus, ProviderSessionRef};

use super::super::board::{
    self, ChildAgent, DAY_MS, Decision, Item, LooseAgent, PaneInfo, Relevance,
};
use super::super::conversation::Conversation;
use super::super::rules::Column;
use super::super::titles;
use super::board_tests::fixture;

const NOW: u64 = 1_800_000_000_000;
const HOUR: u64 = 60 * 60 * 1000;

fn find<'a>(built: &'a board::Built, id: &str) -> &'a Item {
    built
        .items
        .iter()
        .find(|item| item.id == id)
        .unwrap_or_else(|| panic!("{id} missing"))
}

fn pane(name: Option<&str>, tab: Option<&str>, session: Option<&str>) -> PaneInfo {
    PaneInfo {
        pane_label: None,
        tab_label: tab.map(str::to_owned),
        workspace_label: Some("web-app".to_owned()),
        agent_name: name.map(str::to_owned),
        provider: Some("claude".to_owned()),
        provider_session: session.map(str::to_owned),
        cwd: Some("/work/web-app".to_owned()),
    }
}

fn loose(id: &str, name: &str, status: ObservedStatus) -> LooseAgent {
    LooseAgent {
        session: "main".to_owned(),
        pane_id: format!("p-{id}"),
        terminal_id: format!("t-{id}"),
        name: name.to_owned(),
        status,
        state_change_sequence: 1,
    }
}

fn with_loose(inputs: &mut board::Inputs, agent: LooseAgent, info: PaneInfo) {
    inputs
        .pane_info
        .insert((agent.session.clone(), agent.terminal_id.clone()), info);
    inputs.loose_agents.push(agent);
}

fn talk(title: Option<&str>, at: u64, human: bool) -> Conversation {
    Conversation {
        title: title.map(str::to_owned),
        last_activity_ms: Some(at),
        last_user_ms: human.then_some(at),
        human,
        complete: true,
        yard_summary: false,
        digest: "You: fix the flaky upload test".to_owned(),
        first_user: human.then(|| "Fix the flaky upload test".to_owned()),
        last_agent: None,
        last_user: None,
    }
}

#[test]
fn the_main_board_keeps_recent_waiting_and_pinned_work_only() {
    let mut inputs = fixture();
    let built = board::build(&inputs, None);
    // Working now, and a turn finished 2 h ago: recent.
    assert_eq!(find(&built, "a:a-ui").relevance, Relevance::Recent);
    assert_eq!(find(&built, "a:a-review").relevance, Relevance::Recent);
    // Quiet for 9 days with nothing for the user: Older work, flagged to
    // archive only past 14 days.
    let old = find(&built, "a:a-old");
    assert_eq!(old.relevance, Relevance::Older);
    assert!(!old.suggest_archive && !board::on_main(old));
    assert!(built.older >= 1 && built.to_review == built.older);
    // Older cards never count in the tiles and never make to-dos.
    let main_cards = built
        .items
        .iter()
        .filter(|item| board::on_main(item) && !item.rollup)
        .count();
    let counts = &built.counts;
    assert_eq!(
        counts.doing
            + counts.needs_you
            + counts.stale_needs_work
            + counts.stale_likely_done
            + counts.close
            + counts.closed,
        main_cards
    );
    assert!(
        built
            .todos
            .iter()
            .all(|todo| todo.sources.iter().all(|source| source.id != "a:a-old"))
    );

    // Keep pins it to the board; Skip leaves it in Older work, reviewed.
    inputs.triage.insert(
        "a:a-old".to_owned(),
        Decision {
            keep: true,
            at_ms: NOW,
        },
    );
    inputs.triage.insert(
        "w:w-docs".to_owned(),
        Decision {
            keep: false,
            at_ms: NOW,
        },
    );
    let built = board::build(&inputs, None);
    let old = find(&built, "a:a-old");
    assert!(old.pinned && old.relevance == Relevance::Pinned && board::on_main(old));
    let docs = built
        .items
        .iter()
        .find(|item| item.worker_id.as_deref() == Some("w-docs"));
    if let Some(docs) = docs.filter(|docs| docs.id == "w:w-docs") {
        assert!(docs.reviewed);
    }
    assert!(built.to_review < built.older + 1);
}

#[test]
fn skipped_cards_come_back_for_review_after_new_activity() {
    let mut inputs = fixture();
    inputs.triage.insert(
        "a:a-old".to_owned(),
        Decision {
            keep: false,
            at_ms: NOW - 10 * DAY_MS,
        },
    );
    // Skipped ten days ago, active nine days ago: offered again.
    let built = board::build(&inputs, None);
    assert!(!find(&built, "a:a-old").reviewed);
    inputs.triage.get_mut("a:a-old").unwrap().at_ms = NOW - DAY_MS;
    assert!(find(&board::build(&inputs, None), "a:a-old").reviewed);
}

#[test]
fn waiting_on_you_stays_for_the_waiting_window_only() {
    let mut inputs = fixture();
    inputs.now_ms = NOW + 4 * DAY_MS;
    let built = board::build(&inputs, None);
    let api = find(&built, "a:a-api");
    assert_eq!(api.column, Column::NeedsYou);
    assert_eq!(api.relevance, Relevance::Waiting);
    inputs.now_ms = NOW + 8 * DAY_MS;
    assert_eq!(
        find(&board::build(&inputs, None), "a:a-api").relevance,
        Relevance::Older
    );
    // The window is configurable.
    inputs.window.waiting_ms = 30 * DAY_MS;
    assert_eq!(
        find(&board::build(&inputs, None), "a:a-api").relevance,
        Relevance::Waiting
    );
}

#[test]
fn conversation_activity_dates_a_card_and_beats_record_times() {
    let mut inputs = fixture();
    // a-old's record is 9 days old, but the user typed to it an hour ago.
    let old = inputs
        .assignments
        .iter_mut()
        .find(|assignment| assignment.id == "a-old")
        .unwrap();
    let runtime = old.worker.runtime.as_mut().unwrap();
    runtime.provider_session = Some(ProviderSessionRef {
        source: "herdr:claude".to_owned(),
        provider: "claude".to_owned(),
        kind: "id".to_owned(),
        value: "conv-old".to_owned(),
    });
    for candidate in &mut inputs.workers {
        if candidate.worker.id == "w-old" {
            candidate.worker.runtime.clone_from(&old.worker.runtime);
        }
    }
    inputs.conversations.insert(
        "conv-old".to_owned(),
        talk(Some("Docs cleanup"), NOW - HOUR, true),
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-old");
    assert_eq!(item.age_source, "conversation");
    assert_eq!(item.age_ms, HOUR);
    assert_eq!(item.relevance, Relevance::Recent);
    // The user's display name stays the title; the topic becomes "About".
    assert_eq!(item.title, "docs-old");
    assert!(item.about == "Docs cleanup", "{}", item.about);
}

#[test]
fn titles_never_show_ids_profiles_or_provider_names() {
    let mut inputs = fixture();
    with_loose(
        &mut inputs,
        loose("chat", "", ObservedStatus::Idle),
        pane(None, Some("Generalist-2"), Some("conv-chat")),
    );
    inputs.conversations.insert(
        "conv-chat".to_owned(),
        talk(Some("Upload retry fix"), NOW - HOUR, true),
    );
    with_loose(
        &mut inputs,
        loose("plain", "", ObservedStatus::Idle),
        pane(None, Some("1"), Some("conv-plain")),
    );
    inputs
        .conversations
        .insert("conv-plain".to_owned(), talk(None, NOW - 2 * HOUR, true));
    let built = board::build(&inputs, None);
    let chat = find(&built, "l:main:p-chat");
    assert_eq!(
        (chat.title.as_str(), chat.title_source),
        ("Upload retry fix", titles::FROM_CONVERSATION)
    );
    assert_eq!(chat.relevance, Relevance::Recent);
    let plain = find(&built, "l:main:p-plain");
    assert_eq!(plain.title, "Untitled Claude chat in web-app");
    assert_eq!(plain.title_source, titles::UNTITLED);
    for item in &built.items {
        let lower = item.title.to_lowercase();
        assert!(
            !["generalist", "worker", "claude", "codex"].contains(&lower.as_str()),
            "{}",
            item.title
        );
        assert!(!item.title.starts_with("yard-"), "{}", item.title);
    }
}

#[test]
fn helpers_are_hidden_and_counted() {
    let mut inputs = fixture();
    with_loose(
        &mut inputs,
        loose(
            "tmp",
            "yard-a1b2c3d-0123456789abcdef",
            ObservedStatus::Blocked,
        ),
        pane(
            Some("yard-a1b2c3d-0123456789abcdef"),
            Some("Generalist-2"),
            Some("conv-tmp"),
        ),
    );
    with_loose(
        &mut inputs,
        loose("tool", "", ObservedStatus::Idle),
        pane(None, None, Some("conv-tool")),
    );
    inputs
        .conversations
        .insert("conv-tool".to_owned(), talk(None, NOW - HOUR, false));
    inputs.children.push(ChildAgent {
        id: "provider-child:claude:conv-tmp:a1".to_owned(),
        parent_session: "conv-tmp".to_owned(),
        title: "Draft the design doc".to_owned(),
        status: ObservedStatus::Working,
        updated_at_ms: NOW - 60_000,
    });
    let built = board::build(&inputs, None);
    let tmp = find(&built, "l:main:p-tmp");
    assert_eq!(tmp.helper, Some(super::super::plain::HELPER_TEMPORARY));
    let tool = find(&built, "l:main:p-tool");
    assert_eq!(
        tool.helper,
        Some(super::super::plain::HELPER_NO_CONVERSATION)
    );
    // A temporary session that "has a question" never reaches Needs you or
    // the to-do list.
    assert!(!board::on_main(tmp));
    assert!(
        built
            .todos
            .iter()
            .all(|todo| todo.sources.iter().all(|source| source.id != tmp.id))
    );
    assert_eq!(built.helper_rows.len(), 1);
    assert_eq!(
        built.helper_rows[0].parent_id.as_deref(),
        Some("l:main:p-tmp")
    );
    assert_eq!(built.helpers, 3);
}

#[test]
fn every_card_has_three_plain_lines_without_placeholders() {
    let mut inputs = fixture();
    with_loose(
        &mut inputs,
        loose("quiet", "notes-review", ObservedStatus::Idle),
        pane(Some("notes-review"), None, None),
    );
    let built = board::build(&inputs, None);
    for item in built.items.iter().filter(|item| !item.rollup) {
        // Your move is always there; About and Status are left out (empty)
        // rather than filled with a placeholder, and an empty Status still
        // shows the age.
        assert!(
            !item.your_move.trim().is_empty(),
            "{}: no Your move",
            item.id
        );
        assert!(
            !item.status.is_empty() || item.activity_known,
            "{}: neither a status nor an age",
            item.id
        );
        for line in [&item.about, &item.status, &item.your_move] {
            assert_eq!(line.trim(), line.as_str(), "{}: padded line", item.id);
            assert!(!line.contains("N/A"), "{}: {line}", item.id);
            for banned in [
                "not tracked",
                "No conversation topic",
                "Nothing more found",
                "the AI thinks",
                "no task assigned",
            ] {
                assert!(!line.contains(banned), "{}: {line}", item.id);
            }
        }
        if !item.activity_known {
            for line in [&item.about, &item.status, &item.your_move, &item.summary] {
                assert!(!line.contains("under a minute"), "{}: {line}", item.id);
                assert!(!line.contains(" ago"), "{}: {line}", item.id);
            }
        }
        if item.column == Column::Doing {
            assert_eq!(item.your_move, "Nothing \u{2014} it's working");
        }
    }
    let quiet = find(&built, "l:main:p-quiet");
    assert!(!quiet.activity_known);
    // A lowercase agent name reads as a title ("notes-review").
    assert_eq!(quiet.title, "Notes review");
}

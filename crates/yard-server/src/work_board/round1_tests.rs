//! Board v3 review round 1 (neutral fixtures only): idle coordinators are
//! helpers, chats join their folder's project, About / Status / Your move
//! come from the conversation and never contradict the title, role names
//! are qualifiers, and display names read as words.

use yard_domain::{ObservedStatus, ProviderSessionRef, WorkerAvailability};

use super::super::board::{self, Item, LooseAgent, PaneInfo, Relevance};
use super::super::conversation::{self, Conversation};
use super::super::plain;
use super::super::rules::{Column, ItemKind};
use super::super::titles::{self, TitleSources};
use super::board_tests::{candidate, fixture, worker};

const NOW: u64 = 1_800_000_000_000;
const HOUR: u64 = 60 * 60 * 1000;
const DAY: u64 = 24 * HOUR;

fn find<'a>(built: &'a board::Built, id: &str) -> &'a Item {
    built
        .items
        .iter()
        .find(|item| item.id == id)
        .unwrap_or_else(|| panic!("{id} missing"))
}

fn talk(first_user: Option<&str>, last_agent: Option<&str>, at: u64) -> Conversation {
    Conversation {
        title: None,
        last_activity_ms: Some(at),
        last_user_ms: Some(at),
        human: true,
        complete: true,
        yard_summary: false,
        digest: String::new(),
        first_user: first_user.map(str::to_owned),
        last_agent: last_agent.map(str::to_owned),
        last_user: None,
    }
}

fn bind(inputs: &mut board::Inputs, worker_id: &str, session: &str) {
    let reference = Some(ProviderSessionRef {
        source: "herdr:claude".to_owned(),
        provider: "claude".to_owned(),
        kind: "id".to_owned(),
        value: session.to_owned(),
    });
    for candidate in &mut inputs.workers {
        if candidate.worker.id == worker_id {
            candidate.worker.runtime.as_mut().unwrap().provider_session = reference.clone();
        }
    }
    for assignment in &mut inputs.assignments {
        if assignment.worker.id == worker_id {
            assignment.worker.runtime.as_mut().unwrap().provider_session = reference.clone();
        }
    }
}

#[test]
fn an_idle_yard_coordinator_is_a_helper_not_a_card() {
    let mut inputs = fixture();
    for (id, availability) in [
        ("w-yard", WorkerAvailability::YardOrchestrator),
        ("w-stream", WorkerAvailability::CoordinationNode),
    ] {
        inputs.workers.push(candidate(
            worker(id, None, Some(ObservedStatus::Idle), NOW - HOUR),
            availability,
            None,
        ));
    }
    let built = board::build(&inputs, None);
    for id in ["w:w-yard", "w:w-stream"] {
        let item = find(&built, id);
        assert_eq!(item.helper, Some(plain::HELPER_IDLE), "{id}");
        assert!(!board::on_main(item), "{id}");
    }
    // A bare role is never the whole title.
    assert_eq!(
        find(&built, "w:w-stream").title,
        "Untitled workstream in Yard"
    );
    // Working, it is a card again.
    inputs
        .workers
        .last_mut()
        .unwrap()
        .worker
        .runtime
        .as_mut()
        .unwrap()
        .status = ObservedStatus::Working;
    let built = board::build(&inputs, None);
    assert_eq!(find(&built, "w:w-stream").helper, None);
}

#[test]
fn a_chat_in_a_projects_folder_joins_that_project() {
    let mut inputs = fixture();
    let agent = LooseAgent {
        session: "main".to_owned(),
        pane_id: "p-chat".to_owned(),
        terminal_id: "t-chat".to_owned(),
        name: String::new(),
        status: ObservedStatus::Idle,
        state_change_sequence: 1,
    };
    inputs.pane_info.insert(
        ("main".to_owned(), "t-chat".to_owned()),
        PaneInfo {
            workspace_label: Some("beta".to_owned()),
            provider: Some("claude".to_owned()),
            provider_session: Some("conv-chat".to_owned()),
            cwd: Some("/work/beta".to_owned()),
            ..PaneInfo::default()
        },
    );
    inputs.loose_agents.push(agent);
    inputs.conversations.insert(
        "conv-chat".to_owned(),
        talk(
            Some("Task: Investigate the flaky upload tests seen here, and plan a fix"),
            Some("Done.\n\nI've traced the flake to a shared temp folder. Want me to fix it?"),
            NOW - HOUR,
        ),
    );
    let built = board::build(&inputs, None);
    let chat = find(&built, "l:main:p-chat");
    assert_eq!(chat.project_id.as_deref(), Some("beta"));
    assert_eq!(chat.project_name.as_deref(), Some("beta"));
    // About is the request, without "Task:" or "seen here"; nothing about
    // tracking (the chip says that once).
    assert_eq!(chat.about, "Investigate the flaky upload tests");
    // A closing question waits on the user: Needs you, its question quoted.
    assert_eq!(chat.column, Column::NeedsYou);
    assert_eq!(chat.status, "Asks: want me to fix it?");
    assert_eq!(chat.your_move, "Open it and answer the question.");
    assert!(
        !chat
            .labels
            .iter()
            .any(|label| label == "outside Yard" || label.contains("tracked"))
    );
    // The project now has a header.
    assert!(built.projects.iter().any(|project| project.id == "beta"));
}

#[test]
fn a_finished_turn_says_what_it_produced_and_what_to_review() {
    let mut inputs = fixture();
    bind(&mut inputs, "w-review", "conv-review");
    inputs.conversations.insert(
        "conv-review".to_owned(),
        talk(
            Some("Plan the retry rollout"),
            Some(
                "## Summary\nI've drafted the plan with 4 fixes for the retry rollout.\n- step one",
            ),
            NOW - 2 * HOUR,
        ),
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-review");
    assert_eq!(item.column, Column::NeedsYou);
    assert_eq!(
        item.status,
        "Finished: drafted the plan with 4 fixes for the retry rollout"
    );
    assert_eq!(item.your_move, "Review the plan, then mark it done.");
}

#[test]
fn about_never_contradicts_the_title_or_repeats_metadata() {
    let mut inputs = fixture();
    // A worker with no display name: its title comes from the task.
    for assignment in &mut inputs.assignments {
        assignment.worker.display_name = None;
    }
    for candidate in &mut inputs.workers {
        candidate.worker.display_name = None;
    }
    let built = board::build(&inputs, None);
    let mut from_task = 0;
    for item in built.items.iter().filter(|item| !item.rollup) {
        if item.title_source == titles::FROM_TASK {
            from_task += 1;
        }
        for banned in ["no task", "not tracked", "Task:", "Yard has"] {
            assert!(!item.about.contains(banned), "{}: {}", item.id, item.about);
        }
        assert!(
            !(item.about.contains("no task") && item.status.contains("no task")),
            "{}",
            item.id
        );
    }
    assert!(from_task > 0);
}

#[test]
fn needs_you_is_one_queue_and_headers_need_a_card() {
    let built = board::build(&fixture(), None);
    for project in &built.projects {
        assert!(
            built.items.iter().any(|item| !item.rollup
                && board::on_main(item)
                && item.project_id.as_deref() == Some(project.id.as_str())),
            "{}",
            project.id
        );
    }
    let ages = built
        .items
        .iter()
        .filter(|item| item.column == Column::NeedsYou && item.kind != ItemKind::Orchestrator)
        .map(|item| item.age_ms)
        .collect::<Vec<_>>();
    assert!(ages.windows(2).all(|pair| pair[0] >= pair[1]), "{ages:?}");
}

#[test]
fn old_untouched_work_suggests_archiving_in_its_your_move() {
    let mut inputs = fixture();
    inputs.now_ms = NOW + 30 * DAY;
    let built = board::build(&inputs, None);
    let flagged = built
        .items
        .iter()
        .filter(|item| {
            !item.rollup
                && item.helper.is_none()
                && item.suggest_archive
                && item.relevance == Relevance::Older
        })
        .collect::<Vec<_>>();
    assert!(!flagged.is_empty());
    for item in flagged {
        // The age is on the card and in the review banner, not repeated here.
        assert_eq!(item.your_move, "Archive it if you're done with it.");
    }
}

#[test]
fn clauses_are_short_and_plain() {
    assert_eq!(
        plain::clause(
            "Task: Investigate the flaky upload tests seen here, and create a plan of how we can fix them",
            plain::ABOUT_WORDS
        )
        .as_deref(),
        Some("Investigate the flaky upload tests")
    );
    assert_eq!(
        plain::clause(
            "Review the work the orchestrator is doing on design and help round out the design document",
            plain::ABOUT_WORDS
        )
        .as_deref(),
        Some("Review the work the orchestrator is doing on design")
    );
    assert_eq!(
        plain::about_request(
            Some("Read-only research task, do not edit files. FINAL LOCAL-ONLY TASK. Map the cache keys to owners."),
            "Cache"
        )
        .as_deref(),
        Some("Map the cache keys to owners")
    );
    assert_eq!(
        plain::outcome(Some(
            "You're right that the two revisions make the same change."
        )),
        Some(("The two revisions make the same change".to_owned(), false))
    );
    assert_eq!(plain::outcome(Some("Done.")), None);
}

#[test]
fn display_names_read_as_words_and_roles_qualify() {
    assert_eq!(
        titles::split_camel("SearchServiceChanges"),
        "Search Service Changes"
    );
    assert_eq!(titles::split_camel("APIKeyRotation"), "API Key Rotation");
    assert_eq!(titles::split_camel("Docs(Draft)"), "Docs (Draft)");
    assert_eq!(titles::split_camel("CLISDK"), "CLISDK");
    assert_eq!(titles::split_camel("Web App Beta"), "Web App Beta");
    assert_eq!(
        titles::tidy_ids("CR-123456789 review comments"),
        "Review comments on CR 123456789"
    );
    let profiles = std::collections::HashSet::new();
    let mut sources = TitleSources {
        role: Some("Workstream"),
        place: Some("Yard"),
        ..TitleSources::default()
    };
    assert_eq!(
        titles::choose(&sources, &profiles),
        ("Untitled workstream in Yard".to_owned(), titles::UNTITLED)
    );
    sources.conversation_title = Some("Release notes");
    assert_eq!(
        titles::choose(&sources, &profiles).0,
        "Workstream: Release notes"
    );
    sources.role = Some("Yard orchestrator");
    assert_eq!(titles::choose(&sources, &profiles).0, "Yard orchestrator");
    assert!(!titles::meaningful("Workstream", &profiles));
}

#[test]
fn replies_to_yard_messages_are_not_the_outcome_and_the_opening_is_read_from_the_head() {
    let line = |kind: &str, text: &str| {
        format!(
            "{{\"type\":\"{kind}\",\"timestamp\":\"2027-01-15T08:00:00Z\",\"message\":{{\"content\":{}}}}}\n",
            serde_json::to_string(text).unwrap()
        )
    };
    let mut body = String::new();
    body.push_str(&line("assistant", "Earlier reply in the tail."));
    body.push_str(&line("user", "Fix the upload retry bug"));
    body.push_str(&line("assistant", "Fixed the retry loop and added a test."));
    body.push_str(&line(
        "user",
        "AUTOMATIC WORKER SUMMARY REQUEST Your project orchestrator wants a summary.",
    ));
    body.push_str(&line(
        "assistant",
        "Nothing has changed since the last summary.",
    ));
    let parsed = conversation::parse_claude(body.as_bytes());
    assert_eq!(
        parsed.last_agent.as_deref(),
        Some("Fixed the retry loop and added a test.")
    );
    assert_eq!(
        parsed.first_user.as_deref(),
        Some("Fix the upload retry bug")
    );
    // A long file: the opening request is read from its head.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    let mut long = line("user", "Plan the cache migration for the docs site");
    let filler = line("assistant", &"x".repeat(1000));
    while long.len() < usize::try_from(conversation::MAX_TAIL_BYTES).unwrap() + 4096 {
        long.push_str(&filler);
    }
    long.push_str(&line("user", "AUTOMATIC WORKER SUMMARY REQUEST again"));
    std::fs::write(project.join("conv-long.jsonl"), long).unwrap();
    let roots = conversation::ConversationRoots {
        claude_projects: Some(dir.path().to_path_buf()),
        codex_home: None,
    };
    let read = roots.read(&[("claude".to_owned(), "conv-long".to_owned())]);
    let long = &read["conv-long"];
    assert!(!long.complete);
    assert_eq!(
        long.first_user.as_deref(),
        Some("Plan the cache migration for the docs site")
    );
}

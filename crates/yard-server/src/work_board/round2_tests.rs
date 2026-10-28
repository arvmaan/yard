//! Board v3 review round 2 (neutral fixtures only): a quiet card is placed
//! by what the agent last said (a blocker or a question waits on the user,
//! a completion is probably done, "three CRs left" is not), Needs you is
//! split into answer-first and review, "Finished:" only for completions,
//! no placeholder About lines, and titles keep the user's casing.

use yard_domain::{ObservedStatus, ProviderSessionRef};

use super::super::board::{self, ChildAgent, Item, Relevance};
use super::super::conversation::Conversation;
use super::super::plain;
use super::super::rules::{Column, Section};
use super::super::titles;
use super::super::todos::{TodoKind, Urgency};
use super::board_tests::fixture;

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

fn said(inputs: &mut board::Inputs, worker_id: &str, last_agent: &str, at: u64) {
    let session = format!("conv-{worker_id}");
    let reference = Some(ProviderSessionRef {
        source: "herdr:claude".to_owned(),
        provider: "claude".to_owned(),
        kind: "id".to_owned(),
        value: session.clone(),
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
    inputs.conversations.insert(
        session,
        Conversation {
            last_activity_ms: Some(at),
            last_user_ms: Some(at),
            human: true,
            complete: true,
            first_user: Some("Move the uploads to the new storage client".to_owned()),
            last_agent: Some(last_agent.to_owned()),
            ..Conversation::default()
        },
    );
}

#[test]
fn a_blocker_only_the_user_can_clear_waits_on_the_user() {
    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-io",
        "AWS authentication failed \u{b7} refresh your AWS credentials and retry",
        NOW - 4 * DAY,
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-io");
    // Four days quiet, but blocked on the user: the 7-day window keeps it.
    assert_eq!(item.column, Column::NeedsYou);
    assert_eq!(item.relevance, Relevance::Waiting);
    assert_eq!(item.need, Some(plain::NEED_ANSWER));
    assert_eq!(item.status, "Blocked: AWS authentication failed");
    assert_eq!(
        item.your_move,
        "Refresh your AWS credentials, then tell it to continue."
    );
    let todo = built
        .todos
        .iter()
        .find(|todo| todo.sources.iter().any(|source| source.id == item.id))
        .unwrap();
    assert_eq!((todo.kind, todo.urgency), (TodoKind::Unblock, Urgency::Now));
    // Answer comes before review in Needs you.
    let needs = built
        .items
        .iter()
        .filter(|item| item.column == Column::NeedsYou && board::on_main(item))
        .map(|item| item.need)
        .collect::<Vec<_>>();
    let first_review = needs
        .iter()
        .position(|need| *need == Some(plain::NEED_REVIEW));
    let last_answer = needs
        .iter()
        .rposition(|need| *need == Some(plain::NEED_ANSWER));
    if let (Some(review), Some(answer)) = (first_review, last_answer) {
        assert!(answer < review, "{needs:?}");
    }
}

#[test]
fn a_decision_someone_else_must_act_on_is_named() {
    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-io",
        "The publishing root is a signing key that only Ops can use. Its signature is checked.",
        NOW - 2 * DAY,
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-io");
    assert_eq!(item.column, Column::NeedsYou);
    assert!(
        item.status.starts_with("Needs a decision: "),
        "{}",
        item.status
    );
    assert_eq!(
        item.your_move,
        "Decide who contacts Ops, then tell it how to go on."
    );
}

#[test]
fn a_completion_is_probably_done_and_work_left_is_not() {
    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-io",
        "No required SDK changes are left. The model and client are merged.",
        NOW - DAY,
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-io");
    assert_eq!(
        (item.column, item.section),
        (Column::Stale, Some(Section::LikelyDone))
    );
    assert_eq!(item.status, "Finished: no required SDK changes are left");
    assert_eq!(item.your_move, "Mark it done or close it.");

    let mut inputs = fixture();
    // Old and quiet with nothing open: probably done, unless it says otherwise.
    inputs.now_ms = NOW + 10 * DAY;
    said(
        &mut inputs,
        "w-io",
        "You have three CRs left, and all three are unpublished drafts.",
        NOW - 9 * DAY,
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-io");
    assert_eq!(item.column, Column::Stale);
    assert_ne!(item.section, Some(Section::LikelyDone));
    assert_eq!(item.status, "Stopped with three CRs left");
    assert!(
        item.your_move.contains("three CRs left"),
        "{}",
        item.your_move
    );
}

#[test]
fn finished_only_for_completions_found_for_findings() {
    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-review",
        "Gamma is still running two tasks on a September image. The fix is to redeploy.",
        NOW - 2 * HOUR,
    );
    let built = board::build(&inputs, None);
    let item = find(&built, "a:a-review");
    assert_eq!(item.column, Column::NeedsYou);
    assert_eq!(item.need, Some(plain::NEED_REVIEW));
    assert_eq!(
        item.status,
        "Found: gamma is still running two tasks on a September image"
    );
    assert_eq!(
        item.your_move,
        "Read what it found and decide the next step."
    );
    // Finished work to review is "Soon", folded onto its card by the web.
    let todo = built
        .todos
        .iter()
        .find(|todo| todo.kind == TodoKind::Review)
        .unwrap();
    assert_eq!(todo.urgency, Urgency::Soon);

    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-review",
        "I posted four draft replies on CR-123456789, to the revision 1 threads.",
        NOW - 2 * HOUR,
    );
    let item = find(&board::build(&inputs, None), "a:a-review").clone();
    assert!(
        item.status
            .starts_with("Finished: posted four draft replies"),
        "{}",
        item.status
    );
    assert_eq!(item.your_move, "Review CR-123456789, then mark it done.");
}

#[test]
fn acknowledgements_are_never_the_status_snippet() {
    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-io",
        "Your message came through as just \"|\". Which did you mean: 1 or 2?",
        NOW - HOUR,
    );
    let item = find(&board::build(&inputs, None), "a:a-io").clone();
    assert_eq!(item.column, Column::NeedsYou);
    assert_eq!(item.status, "Asks: which did you mean: 1 or 2?");
    assert!(!item.status.contains("came through"));
}

#[test]
fn about_is_a_request_or_nothing() {
    for (request, expected) in [
        ("is it discoverResponse or discoveryResponse?", None),
        ("add the setter", None),
        (
            "When several services list one host, the newest one wins.",
            None,
        ),
        (
            "Read-only research task. SyncJob writes these docs with keys like appName. \
             Find where the appName copies come from.",
            Some("Find where the appName copies come from"),
        ),
        (
            "Verify whether head a1b2c3d fast-forwards through 0f9e8d7 on the branch.",
            None,
        ),
        (
            "Investigate the shepherd risks seen here, and plan a fix",
            Some("Investigate the shepherd risks"),
        ),
    ] {
        assert_eq!(
            plain::about_request(Some(request), "Some title").as_deref(),
            expected,
            "{request}"
        );
    }
    // Nothing known beyond the title: no About at all, never a placeholder.
    let built = board::build(&fixture(), None);
    for item in &built.items {
        assert!(
            !item.about.contains("Nothing more found") && !item.about.contains("No conversation"),
            "{}: {}",
            item.id,
            item.about
        );
    }
}

#[test]
fn titles_keep_the_users_casing_and_agent_names_read_as_titles() {
    assert_eq!(titles::split_camel("PlanXCA(R&D)"), "PlanXCA (R&D)");
    assert_eq!(titles::split_camel("FollowUpTasks"), "Follow Up Tasks");
    assert_eq!(titles::split_camel("APIGateXSetup"), "API GateX Setup");
    assert_eq!(titles::split_camel("WEBSDK"), "WEBSDK");
    assert_eq!(titles::case_words("api cdk review"), "API CDK review");
    assert_eq!(titles::case_words("web phase2"), "Web phase 2");
    assert_eq!(titles::case_words("Release notes"), "Release notes");
}

#[test]
fn a_working_card_says_what_it_is_doing_and_its_subagents() {
    let mut inputs = fixture();
    said(
        &mut inputs,
        "w-ui",
        "I'm applying the review fixes now.",
        NOW - 60_000,
    );
    for (index, title) in ["Review design vs code", "Review design vs requirements"]
        .into_iter()
        .enumerate()
    {
        inputs.children.push(ChildAgent {
            id: format!("child-{index}"),
            parent_session: "conv-w-ui".to_owned(),
            title: title.to_owned(),
            status: ObservedStatus::Working,
            updated_at_ms: NOW,
        });
    }
    let item = find(&board::build(&inputs, None), "a:a-ui").clone();
    assert_eq!(item.column, Column::Doing);
    assert_eq!(
        item.status,
        "Working: applying the review fixes now, with 2 subagents"
    );
}

#[test]
fn idle_coordinators_say_idle_and_never_stalled() {
    let built = board::build(&fixture(), None);
    for item in built
        .items
        .iter()
        .filter(|item| item.helper == Some(plain::HELPER_IDLE))
    {
        assert!(
            item.status.starts_with("Idle"),
            "{}: {}",
            item.id,
            item.status
        );
        assert_eq!(item.your_move, "Nothing \u{2014} it's idle");
    }
}

#[test]
fn lowercase_agent_names_take_the_conversations_capitals() {
    assert_eq!(
        titles::capitals_like("Orbit cdk review", "You: review the ORBIT stack"),
        "ORBIT cdk review"
    );
    assert_eq!(
        titles::capitals_like("Notes review", "You: hi"),
        "Notes review"
    );
}

#[test]
fn capitals_inside_camel_case_names_count() {
    assert_eq!(
        titles::capitals_like("Orbit sync", "ORBITSyncService"),
        "ORBIT sync"
    );
}

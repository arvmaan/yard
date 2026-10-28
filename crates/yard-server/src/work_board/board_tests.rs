//! Deterministic board assembly over neutral fixtures (alpha/beta/gamma).

use std::collections::HashMap;

use yard_domain::{
    Assignment, AssignmentAttempt, AssignmentLifecycle, AttemptLifecycle, CanvasPlacement,
    CompletionOutcome, CompletionReceipt, IsolationPolicy, ObservedStatus,
    OrchestratorStatusReport, OrchestratorStatusState, Project, ProjectPlacement,
    ProjectRuntimeBinding, ProjectWorkflowProfilePin, ReceiptDetailLevel, RuntimeObservationState,
    RuntimeProcessState, Worker, WorkerAvailability, WorkerCandidate, WorkerDesiredState,
    WorkerOwnershipKind, WorkerRuntimeBinding,
};

use super::super::board::{self, Action, Inputs, LooseAgent, Mark, Previous, SnapshotNext};
use super::super::rules::{COLD_MS, Column, ItemKind, Section};
use super::super::todos::{SnapshotLine, TodoKind, Urgency};

const NOW: u64 = 1_800_000_000_000;
const HOUR: u64 = 60 * 60 * 1000;

pub(crate) fn worker(
    id: &str,
    name: Option<&str>,
    status: Option<ObservedStatus>,
    updated: u64,
) -> Worker {
    Worker {
        id: id.to_owned(),
        profile_id: Some("profile".to_owned()),
        profile_version: Some(1),
        ownership_kind: WorkerOwnershipKind::YardOwned,
        display_name: name.map(str::to_owned),
        desired_state: WorkerDesiredState::Running,
        runtime: status.map(|status| WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "main".to_owned(),
            workspace_id: "ws".to_owned(),
            terminal_id: format!("t-{id}"),
            tab_id: Some("tab".to_owned()),
            pane_id: format!("p-{id}"),
            provider_session: None,
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status,
            state_change_sequence: 1,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: updated,
        }),
        version: 1,
        created_at_unix_ms: updated,
        updated_at_unix_ms: updated,
    }
}

pub(crate) fn project(id: &str, name: &str, orchestrator: Worker) -> Project {
    Project {
        id: id.to_owned(),
        name: name.to_owned(),
        runtime: ProjectRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "main".to_owned(),
            workspace_id: "ws".to_owned(),
        },
        orchestrator,
        placement: ProjectPlacement {
            geometry: CanvasPlacement {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 300.0,
            },
            version: 1,
            updated_at_unix_ms: NOW,
        },
        workflow_profile: ProjectWorkflowProfilePin {
            profile_id: "wf".to_owned(),
            profile_version: 1,
            pinned_by: "test".to_owned(),
            pinned_at_unix_ms: NOW,
        },
        version: 1,
        created_at_unix_ms: NOW - 30 * 24 * HOUR,
        updated_at_unix_ms: NOW - HOUR,
    }
}

pub(crate) fn assignment(
    id: &str,
    project_id: &str,
    worker: Worker,
    lifecycle: AssignmentLifecycle,
    updated: u64,
) -> Assignment {
    let receipt = (lifecycle == AssignmentLifecycle::Completed).then(|| CompletionReceipt {
        id: format!("r-{id}"),
        assignment_id: id.to_owned(),
        attempt_id: format!("at-{id}"),
        outcome: CompletionOutcome::Completed,
        detail_level: ReceiptDetailLevel::Detailed,
        objective_snapshot: None,
        summary: "Docs pass done".to_owned(),
        artifact_refs: Vec::new(),
        artifacts: Vec::new(),
        evidence_refs: Vec::new(),
        unresolved_blockers: Vec::new(),
        actor: "worker".to_owned(),
        created_at_unix_ms: updated,
    });
    Assignment {
        id: id.to_owned(),
        project_id: project_id.to_owned(),
        allocation_id: format!("al-{id}"),
        worker,
        profile_id: "profile".to_owned(),
        profile_version: 1,
        profile_name: "Coder".to_owned(),
        objective: format!("Objective of {id}\nmore detail"),
        role: "worker".to_owned(),
        isolation_policy: IsolationPolicy::ProjectWorkspace,
        lifecycle,
        attempt: AssignmentAttempt {
            id: format!("at-{id}"),
            assignment_id: id.to_owned(),
            ordinal: 1,
            lifecycle: AttemptLifecycle::Active,
            error: None,
            version: 1,
            created_at_unix_ms: updated,
            updated_at_unix_ms: updated,
        },
        completion_receipt: receipt,
        cancellation: None,
        version: 1,
        created_at_unix_ms: updated,
        updated_at_unix_ms: updated,
    }
}

pub(crate) fn candidate(
    worker: Worker,
    availability: WorkerAvailability,
    project: Option<&str>,
) -> WorkerCandidate {
    WorkerCandidate {
        worker,
        profile_name: Some("Coder".to_owned()),
        default_role: None,
        availability,
        project_id: project.map(str::to_owned),
        assignment_id: None,
        reason: None,
    }
}

/// alpha: one working, one finished turn, one stale; beta: a failed one and
/// an orchestrator asking for attention; gamma: completed with receipt.
#[allow(clippy::too_many_lines)]
pub(crate) fn fixture() -> Inputs {
    let w_ui = worker(
        "w-ui",
        Some("worker-ui"),
        Some(ObservedStatus::Working),
        NOW - 2 * 60 * 1000,
    );
    let w_review = worker(
        "w-review",
        Some("worker-review"),
        Some(ObservedStatus::Done),
        NOW - 2 * HOUR,
    );
    let w_io = worker(
        "w-io",
        Some("worker-io"),
        Some(ObservedStatus::Idle),
        NOW - 3 * 24 * HOUR,
    );
    let w_old = worker(
        "w-old",
        Some("docs-old"),
        Some(ObservedStatus::Idle),
        NOW - 9 * 24 * HOUR,
    );
    let w_api = worker(
        "w-api",
        Some("api-fix"),
        Some(ObservedStatus::Idle),
        NOW - 5 * HOUR,
    );
    let w_docs = worker(
        "w-docs",
        Some("docs-pass"),
        Some(ObservedStatus::Idle),
        NOW - 9 * 24 * HOUR,
    );
    let w_spare = worker("w-spare", None, Some(ObservedStatus::Idle), NOW - 4 * HOUR);
    let w_dead1 = worker("w-dead1", None, None, NOW - 20 * 24 * HOUR);
    let w_dead2 = worker("w-dead2", None, None, NOW - 21 * 24 * HOUR);
    let w_ended = worker("w-ended", None, None, NOW - 40 * 24 * HOUR);
    let o_alpha = worker("o-alpha", None, Some(ObservedStatus::Idle), NOW - HOUR);
    let o_beta = worker("o-beta", None, Some(ObservedStatus::Idle), NOW - HOUR);
    let o_gamma = worker("o-gamma", None, Some(ObservedStatus::Idle), NOW - HOUR);
    let projects = vec![
        project("alpha", "alpha", o_alpha.clone()),
        project("beta", "beta", o_beta.clone()),
        project("gamma", "gamma", o_gamma.clone()),
    ];
    let assignments = vec![
        assignment(
            "a-ui",
            "alpha",
            w_ui.clone(),
            AssignmentLifecycle::Active,
            NOW - 30 * 60 * 1000,
        ),
        assignment(
            "a-review",
            "alpha",
            w_review.clone(),
            AssignmentLifecycle::Active,
            NOW - 2 * HOUR,
        ),
        assignment(
            "a-io",
            "alpha",
            w_io.clone(),
            AssignmentLifecycle::Active,
            NOW - 3 * 24 * HOUR,
        ),
        assignment(
            "a-old",
            "alpha",
            w_old.clone(),
            AssignmentLifecycle::Active,
            NOW - 9 * 24 * HOUR,
        ),
        assignment(
            "a-api",
            "beta",
            w_api.clone(),
            AssignmentLifecycle::Failed,
            NOW - 5 * HOUR,
        ),
        assignment(
            "a-docs",
            "gamma",
            w_docs.clone(),
            AssignmentLifecycle::Completed,
            NOW - 9 * 24 * HOUR,
        ),
        // An older completed assignment of the working worker never wins.
        assignment(
            "a-ui-old",
            "alpha",
            w_ui.clone(),
            AssignmentLifecycle::Completed,
            NOW - 20 * 24 * HOUR,
        ),
    ];
    let workers = vec![
        candidate(w_ui, WorkerAvailability::Assigned, Some("alpha")),
        candidate(w_review, WorkerAvailability::Assigned, Some("alpha")),
        candidate(w_io, WorkerAvailability::Assigned, Some("alpha")),
        candidate(w_old, WorkerAvailability::Assigned, Some("alpha")),
        candidate(w_api, WorkerAvailability::Assigned, Some("beta")),
        candidate(w_docs, WorkerAvailability::UnassignedLive, Some("gamma")),
        candidate(w_spare, WorkerAvailability::UnassignedLive, None),
        candidate(w_dead1, WorkerAvailability::Unavailable, None),
        candidate(w_dead2, WorkerAvailability::Unavailable, None),
        candidate(w_ended, WorkerAvailability::Ended, None),
        candidate(o_alpha, WorkerAvailability::Orchestrator, Some("alpha")),
        candidate(o_beta, WorkerAvailability::Orchestrator, Some("beta")),
        candidate(o_gamma, WorkerAvailability::Orchestrator, Some("gamma")),
    ];
    let mut reports = HashMap::new();
    reports.insert(
        "beta".to_owned(),
        OrchestratorStatusReport {
            version: 1,
            command_id: "cmd".to_owned(),
            state: OrchestratorStatusState::NeedsAttention,
            last: "Waiting on schema decision".to_owned(),
            next: "Pick v2 schema; rerun migration".to_owned(),
            blockers: vec!["Use v2 schema?".to_owned()],
        },
    );
    let mut snapshots = HashMap::new();
    snapshots.insert(
        "alpha".to_owned(),
        SnapshotNext {
            node_id: "node-1".to_owned(),
            snapshot_id: "snap-1".to_owned(),
            lines: vec![SnapshotLine {
                text: "Add retry test for upload path".to_owned(),
                checkbox: false,
            }],
        },
    );
    Inputs {
        now_ms: NOW,
        projects,
        assignments,
        workers,
        reports,
        failed_commands: std::collections::HashSet::new(),
        snapshots,
        loose_agents: vec![LooseAgent {
            session: "main".to_owned(),
            pane_id: "p-loose".to_owned(),
            terminal_id: "t-loose".to_owned(),
            name: "scratch agent".to_owned(),
            status: ObservedStatus::Idle,
            state_change_sequence: 4,
        }],
        tail_revisions: HashMap::new(),
        live_panes: None,
        ..Inputs::default()
    }
}

fn find<'a>(built: &'a board::Built, id: &str) -> &'a board::Item {
    built
        .items
        .iter()
        .find(|item| item.id == id)
        .unwrap_or_else(|| panic!("{id} missing"))
}

#[test]
fn fixture_lands_in_the_expected_columns() {
    let built = board::build(&fixture(), None);
    assert_eq!(find(&built, "a:a-ui").column, Column::Doing);
    assert_eq!(find(&built, "a:a-review").column, Column::NeedsYou);
    let api = find(&built, "a:a-api");
    assert_eq!(api.column, Column::NeedsYou);
    // Completion receipts refuse a failed assignment: never offer one.
    assert!(
        !api.actions.contains(&Action::MarkDone),
        "{:?}",
        api.actions
    );
    let io = find(&built, "a:a-io");
    assert_eq!(
        (io.column, io.section),
        (Column::Stale, Some(Section::NeedsWork))
    );
    // Cold, nothing next, no open checkbox in the snapshot: likely done.
    let old = find(&built, "a:a-old");
    assert_eq!(
        (old.column, old.section),
        (Column::Stale, Some(Section::LikelyDone))
    );
    assert!(old.actions.contains(&Action::MarkDone));
    let docs = find(&built, "a:a-docs");
    assert_eq!(docs.column, Column::Close);
    assert!(docs.actions.contains(&Action::ArchiveProject));
    // Records without a live agent are not cards, only counted.
    assert!(
        built
            .items
            .iter()
            .all(|item| item.id != "dead" && item.id != "w:w-ended")
    );
    assert_eq!(
        built.hidden,
        board::Hidden {
            not_live: 3,
            dead_runtimes: 2
        }
    );
    assert_eq!(find(&built, "w:w-spare").column, Column::Stale);
    assert_eq!(find(&built, "l:main:p-loose").kind, ItemKind::LooseAgent);
    let beta = find(&built, "o:beta");
    assert_eq!(beta.column, Column::NeedsYou);
    assert!(!beta.rollup);
    assert!(find(&built, "o:alpha").rollup);
    // Exactly one card per worker.
    assert!(built.items.iter().all(|item| item.id != "a:a-ui-old"));
}

#[test]
fn every_live_agent_has_one_card_and_the_rest_are_counted() {
    let inputs = fixture();
    let built = board::build(&inputs, None);
    // 6 workers with assignments + 3 orchestrators + spare + loose; the
    // ended and the two dead records are counted in `hidden`.
    assert_eq!(built.items.len(), 6 + 3 + 1 + 1);
    let records = inputs.workers.len();
    let cards_with_worker = built
        .items
        .iter()
        .filter(|item| item.worker_id.is_some())
        .count();
    assert_eq!(cards_with_worker + built.hidden.not_live, records);
    let counts = &built.counts;
    let rolled = built.items.iter().filter(|item| item.rollup).count();
    assert_eq!(
        counts.doing
            + counts.needs_you
            + counts.stale_needs_work
            + counts.stale_likely_done
            + counts.close
            + counts.closed
            + rolled
            + built.older
            + built
                .items
                .iter()
                .filter(|item| item.helper.is_some() && !item.rollup)
                .count(),
        built.items.len()
    );
}

#[test]
fn todos_are_ordered_by_urgency_and_cite_sources() {
    let built = board::build(&fixture(), None);
    let texts = built
        .todos
        .iter()
        .map(|todo| todo.text.as_str())
        .collect::<Vec<_>>();
    let has_prefix = |prefix: &str| texts.iter().any(|text| text.starts_with(prefix));
    assert!(
        has_prefix("Find out why \u{201c}api-fix\u{201d}"),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"Answer \u{201c}beta orchestrator\u{201d}: use v2 schema?"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("Review ")
                && text.contains("\u{201c}worker-review\u{201d}")),
        "{texts:?}"
    );
    // An orchestrator's own next steps are not rows of their own (it needs
    // an answer first; its steps are on its card).
    assert!(!texts.contains(&"Pick v2 schema"), "{texts:?}");
    assert!(!texts.contains(&"rerun migration"), "{texts:?}");
    assert!(texts.contains(&"Add retry test for upload path"));
    assert!(!texts.iter().any(|text| text.starts_with("Clean up")));
    // The loose agent's age is unknown (no conversation file), so it waits
    // in Older work and makes no to-do.
    assert!(
        !texts
            .iter()
            .any(|text| text.starts_with("Decide whether to keep "))
    );
    assert_eq!(
        find(&built, "l:main:p-loose").relevance,
        board::Relevance::Older
    );
    let urgencies = built
        .todos
        .iter()
        .map(|todo| todo.urgency)
        .collect::<Vec<_>>();
    let mut sorted = urgencies.clone();
    sorted.sort();
    assert_eq!(urgencies, sorted);
    assert!(built.todos.iter().all(|todo| !todo.sources.is_empty()));
    let snapshot = built
        .todos
        .iter()
        .find(|todo| todo.kind == TodoKind::Snapshot)
        .unwrap();
    assert_eq!(snapshot.urgency, Urgency::Later);
}

#[test]
fn open_snapshot_checkbox_keeps_cold_items_in_needs_work() {
    let mut inputs = fixture();
    inputs.snapshots.get_mut("alpha").unwrap().lines[0].checkbox = true;
    let built = board::build(&inputs, None);
    let old = find(&built, "a:a-old");
    assert_eq!(old.section, Some(Section::NeedsWork));
}

#[test]
fn a_change_since_the_last_board_corrects_the_age() {
    let inputs = fixture();
    let first = board::build(&inputs, None);
    let old = find(&first, "a:a-old");
    assert!(old.age_ms >= COLD_MS);
    let mut marks = board::marks(&first.items);
    marks.insert(
        "a:a-old".to_owned(),
        Mark {
            state_change_sequence: 0,
            tail_revision: None,
            last_changed_ms: None,
        },
    );
    let previous = Previous {
        generated_at_ms: NOW - HOUR,
        marks,
    };
    let second = board::build(&inputs, Some(&previous));
    let corrected = find(&second, "a:a-old");
    assert_eq!(corrected.age_ms, HOUR);
    assert_eq!(corrected.age_source, "runtime status changed");
    assert_eq!(corrected.section, Some(Section::NeedsWork));
}

#[test]
fn a_loose_agent_ages_while_its_state_is_unchanged() {
    let mut inputs = fixture();
    let first = board::build(&inputs, None);
    let id = "l:main:p-loose";
    assert_eq!(find(&first, id).age_ms, 0);
    let previous = Previous {
        generated_at_ms: NOW,
        marks: board::marks(&first.items),
    };
    // Five days later, same state_change_sequence: the age keeps growing.
    inputs.now_ms = NOW + 5 * 24 * HOUR;
    let second = board::build(&inputs, Some(&previous));
    let loose = find(&second, id);
    assert_eq!(loose.age_ms, 5 * 24 * HOUR);
    assert_eq!(loose.age_source, "unchanged since first seen");
    // And it carries across a third generation.
    let previous = Previous {
        generated_at_ms: inputs.now_ms,
        marks: board::marks(&second.items),
    };
    inputs.now_ms += HOUR;
    let third = board::build(&inputs, Some(&previous));
    assert_eq!(find(&third, id).age_ms, 5 * 24 * HOUR + HOUR);
    // A state change dates activity to the previous generation.
    let previous = Previous {
        generated_at_ms: inputs.now_ms,
        marks: board::marks(&third.items),
    };
    inputs.now_ms += HOUR;
    inputs.loose_agents[0].state_change_sequence += 1;
    let fourth = board::build(&inputs, Some(&previous));
    assert_eq!(find(&fourth, id).age_ms, HOUR);
}

#[test]
fn an_idle_report_is_closable_only_without_later_activity() {
    let mut inputs = fixture();
    inputs.reports.insert(
        "gamma".to_owned(),
        OrchestratorStatusReport {
            version: 1,
            command_id: "cmd-g".to_owned(),
            state: OrchestratorStatusState::Idle,
            last: "All done".to_owned(),
            next: String::new(),
            blockers: Vec::new(),
        },
    );
    let built = board::build(&inputs, None);
    assert_eq!(find(&built, "o:gamma").column, Column::Close);
    // An assignment in the project changed after the orchestrator did.
    inputs
        .assignments
        .iter_mut()
        .find(|assignment| assignment.id == "a-docs")
        .unwrap()
        .updated_at_unix_ms = NOW - 60 * 1000;
    let built = board::build(&inputs, None);
    assert_ne!(find(&built, "o:gamma").column, Column::Close);
}

#[test]
fn every_needs_you_card_yields_a_todo_now_or_soon_for_a_review() {
    let mut inputs = fixture();
    // needs_attention with no blockers: no chip maps to a to-do on its own.
    inputs.reports.get_mut("beta").unwrap().blockers.clear();
    let built = board::build(&inputs, None);
    for item in built
        .items
        .iter()
        .filter(|item| item.column == Column::NeedsYou)
    {
        // Finished work to review is "Soon"; anything to answer is "Now".
        let urgency = if item.need == Some(super::super::plain::NEED_REVIEW) {
            Urgency::Soon
        } else {
            Urgency::Now
        };
        assert!(
            built.todos.iter().any(|todo| todo.urgency == urgency
                && todo.sources.iter().any(|source| source.id == item.id)),
            "{} has no {urgency:?} to-do: {:?}",
            item.id,
            built.todos
        );
    }
    assert!(
        built
            .todos
            .iter()
            .any(|todo| todo.text.starts_with("Check on ")),
        "{:?}",
        built.todos
    );
}

#[test]
fn fingerprint_tracks_workers() {
    let inputs = fixture();
    let before = board::fingerprint(&inputs.workers);
    let mut workers = inputs.workers;
    workers[0].worker.updated_at_unix_ms = NOW + 1;
    assert_ne!(before, board::fingerprint(&workers));
}

#[test]
fn card_text_is_redacted() {
    let mut inputs = fixture();
    // Built at runtime so secret scanners do not flag a fake key.
    let fake_key = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    inputs.assignments[0].objective =
        format!("Deploy with {fake_key} from /Users/someone/repo/x.rs");
    let built = board::build(&inputs, None);
    let json = serde_json::to_string(&built).unwrap();
    assert!(!json.contains(&fake_key));
    assert!(!json.contains("/Users/someone"));
}

/// The deterministic board is the critical path: 10 projects, 50 workers
/// with assignments, 20 spare workers and 30 dead ones must build far under
/// the 1 s budget (measured, p95 over 50 runs).
#[test]
fn deterministic_build_is_fast_at_scale() {
    let mut inputs = fixture();
    inputs.projects.clear();
    inputs.assignments.clear();
    inputs.workers.clear();
    for project_index in 0..10 {
        let orchestrator = worker(
            &format!("o-{project_index}"),
            None,
            Some(ObservedStatus::Idle),
            NOW - HOUR,
        );
        let project_id = format!("p-{project_index}");
        inputs.projects.push(project(
            &project_id,
            &format!("project {project_index}"),
            orchestrator.clone(),
        ));
        inputs.workers.push(candidate(
            orchestrator,
            WorkerAvailability::Orchestrator,
            Some(&project_id),
        ));
        for worker_index in 0..5 {
            let id = format!("w-{project_index}-{worker_index}");
            let status = [
                ObservedStatus::Working,
                ObservedStatus::Done,
                ObservedStatus::Idle,
            ][worker_index % 3];
            let age = u64::try_from(worker_index).unwrap() * 2 * 24 * HOUR;
            let w = worker(&id, Some(&id), Some(status), NOW - age);
            inputs.assignments.push(assignment(
                &format!("a-{id}"),
                &project_id,
                w.clone(),
                AssignmentLifecycle::Active,
                NOW - age,
            ));
            inputs.workers.push(candidate(
                w,
                WorkerAvailability::Assigned,
                Some(&project_id),
            ));
        }
    }
    for index in 0..20 {
        let w = worker(
            &format!("spare-{index}"),
            None,
            Some(ObservedStatus::Idle),
            NOW - HOUR,
        );
        inputs
            .workers
            .push(candidate(w, WorkerAvailability::UnassignedLive, None));
    }
    for index in 0..30 {
        let w = worker(&format!("dead-{index}"), None, None, NOW - 20 * 24 * HOUR);
        inputs
            .workers
            .push(candidate(w, WorkerAvailability::Unavailable, None));
    }
    let mut samples = Vec::new();
    let mut built = None;
    for _ in 0..50 {
        let start = std::time::Instant::now();
        built = Some(board::build(&inputs, None));
        samples.push(start.elapsed());
    }
    samples.sort();
    let p95 = samples[47];
    let built = built.unwrap();
    // 50 assigned + 10 orchestrators + 20 spare + the loose agent; the 30
    // dead records are counted, not shown.
    assert_eq!(built.items.len(), 50 + 10 + 20 + 1);
    assert_eq!(
        built.hidden,
        board::Hidden {
            not_live: 30,
            dead_runtimes: 30
        }
    );
    eprintln!("deterministic build p50={:?} p95={:?}", samples[25], p95);
    assert!(p95 < std::time::Duration::from_millis(200), "p95 {p95:?}");
}

fn panes(running: &[&str], read: &[&str], ids: &[&str]) -> board::LivePanes {
    board::LivePanes {
        running_sessions: running.iter().map(|name| (*name).to_owned()).collect(),
        read_sessions: read.iter().map(|name| (*name).to_owned()).collect(),
        panes: ids
            .iter()
            .map(|id| (("main".to_owned(), (*id).to_owned()), ObservedStatus::Idle))
            .collect(),
    }
}

#[test]
fn only_agents_open_in_herdr_now_are_cards() {
    // Herdr lists the session and its agent panes: only those workers stay.
    let mut inputs = fixture();
    inputs.live_panes = Some(panes(
        &["main"],
        &["main"],
        &["p-w-ui", "t-o-alpha", "p-w-spare"],
    ));
    let built = board::build(&inputs, None);
    let ids = built
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 4, "{ids:?}");
    for id in ["a:a-ui", "o:alpha", "w:w-spare", "l:main:p-loose"] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    // Only projects with a live agent keep a header; snapshot to-dos only
    // come from them.
    assert_eq!(
        built
            .projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"]
    );
    assert!(
        built.todos.iter().all(|todo| {
            todo.project_id.is_none() || todo.project_id.as_deref() == Some("alpha")
        })
    );
    assert_eq!(built.hidden.not_live, 13 - 3);
    assert_eq!(built.hidden.dead_runtimes, 2);

    // A session that is not running hides its workers; one whose inventory
    // could not be read trusts Yard's own observation.
    inputs.live_panes = Some(panes(&[], &[], &[]));
    let built = board::build(&inputs, None);
    assert!(
        built
            .items
            .iter()
            .all(|item| item.kind == ItemKind::LooseAgent)
    );
    assert!(built.projects.is_empty());
    inputs.live_panes = Some(panes(&["main"], &[], &[]));
    assert_eq!(board::build(&inputs, None).items.len(), 11);
}

#[test]
fn cards_are_grouped_by_project_within_a_column() {
    let built = board::build(&fixture(), None);
    let order = ["alpha", "beta", "gamma"];
    // Needs you is one queue, oldest first (the to-dos use the same order).
    let waiting = built
        .items
        .iter()
        .filter(|item| item.column == Column::NeedsYou && !item.rollup)
        .filter(|item| item.base.rule != super::super::rules::Rule::R3)
        .map(|item| item.age_ms)
        .collect::<Vec<_>>();
    assert!(
        waiting.windows(2).all(|pair| pair[0] >= pair[1]),
        "{waiting:?}"
    );
    for column in [Column::Stale, Column::Close] {
        let ranks = built
            .items
            .iter()
            .filter(|item| item.column == column && !item.rollup)
            .map(|item| {
                item.project_id
                    .as_deref()
                    .and_then(|id| order.iter().position(|name| *name == id))
                    .unwrap_or(usize::MAX)
            })
            .collect::<Vec<_>>();
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        assert_eq!(ranks, sorted, "{column:?}");
    }
}

#[test]
fn an_assignment_run_by_the_orchestrator_worker_is_one_card() {
    let mut inputs = fixture();
    let orchestrator = inputs.projects[1].orchestrator.clone();
    inputs.assignments.push(assignment(
        "a-by-orchestrator",
        "beta",
        orchestrator,
        AssignmentLifecycle::Active,
        NOW - HOUR,
    ));
    // A second record bound to an already shown pane is not a card either.
    let mut twin = worker("w-twin", None, Some(ObservedStatus::Idle), NOW - HOUR);
    twin.runtime.as_mut().unwrap().pane_id = "p-w-spare".to_owned();
    inputs
        .workers
        .push(candidate(twin, WorkerAvailability::UnassignedLive, None));
    let built = board::build(&inputs, None);
    assert!(
        built
            .items
            .iter()
            .all(|item| item.id != "a:a-by-orchestrator")
    );
    assert_eq!(
        built
            .items
            .iter()
            .filter(|item| item.worker_id.as_deref() == Some("o-beta"))
            .count(),
        1
    );
    assert_eq!(
        built
            .items
            .iter()
            .filter(|item| item.id == "w:w-spare" || item.id == "w:w-twin")
            .count(),
        1
    );
}

#[test]
fn an_ambiguous_worker_listed_by_herdr_is_one_needs_you_card() {
    // Yard lost track of the runtime (ambiguous observation), but Herdr
    // lists its terminal: the agent is open, so it is a card that needs the
    // user, and its pane is not also a loose agent.
    let mut inputs = fixture();
    let mut lost = worker(
        "w-lost",
        Some("lost"),
        Some(ObservedStatus::Idle),
        NOW - HOUR,
    );
    lost.runtime.as_mut().unwrap().observation_state = RuntimeObservationState::Ambiguous;
    inputs.workers.push(candidate(
        lost,
        WorkerAvailability::Ambiguous,
        Some("alpha"),
    ));
    inputs.loose_agents.push(LooseAgent {
        session: "main".to_owned(),
        pane_id: "p-w-lost".to_owned(),
        terminal_id: "t-w-lost".to_owned(),
        name: "lost".to_owned(),
        status: ObservedStatus::Idle,
        state_change_sequence: 2,
    });
    inputs.live_panes = Some(panes(&["main"], &["main"], &["p-w-lost", "t-w-lost"]));
    let built = board::build(&inputs, None);
    let card = find(&built, "w:w-lost");
    assert_eq!(card.column, Column::NeedsYou);
    assert!(card.chips.iter().any(|chip| chip == "ambiguous runtime"));
    assert!(built.items.iter().all(|item| item.id != "l:main:p-w-lost"));
    assert!(built.todos.iter().any(
        |todo| todo.urgency == Urgency::Now && todo.text.starts_with("Sort out which terminal")
    ));
    // Only its terminal counts when Yard does not observe it: a reused
    // pane id alone does not revive it.
    inputs.live_panes = Some(panes(&["main"], &["main"], &["p-w-lost"]));
    let built = board::build(&inputs, None);
    assert!(built.items.iter().all(|item| item.id != "w:w-lost"));
}

#[test]
fn a_pane_bound_to_a_record_off_the_board_is_a_loose_agent() {
    // The ended record's terminal is still open in Herdr: shown once, as a
    // loose agent (not hidden because some record names its pane).
    let mut inputs = fixture();
    inputs.loose_agents.push(LooseAgent {
        session: "main".to_owned(),
        pane_id: "p-w-ended".to_owned(),
        terminal_id: "t-w-ended".to_owned(),
        name: "old agent".to_owned(),
        status: ObservedStatus::Idle,
        state_change_sequence: 3,
    });
    let built = board::build(&inputs, None);
    assert!(built.items.iter().any(|item| item.id == "l:main:p-w-ended"));
    // A pane a card claims is never also a loose agent.
    inputs.loose_agents.push(LooseAgent {
        session: "main".to_owned(),
        pane_id: "p-w-ui".to_owned(),
        terminal_id: "t-w-ui".to_owned(),
        name: "worker-ui".to_owned(),
        status: ObservedStatus::Working,
        state_change_sequence: 3,
    });
    let built = board::build(&inputs, None);
    assert!(built.items.iter().all(|item| item.id != "l:main:p-w-ui"));
}

#[test]
fn live_yard_orchestrator_and_workstreams_are_cards() {
    let mut inputs = fixture();
    let yard = worker("w-yard", None, Some(ObservedStatus::Idle), NOW - HOUR);
    let node = worker(
        "w-node",
        Some("research"),
        Some(ObservedStatus::Working),
        NOW - HOUR,
    );
    let gone = worker("w-node-gone", None, None, NOW - 9 * 24 * HOUR);
    // The orchestrator of a project that is not on the board (archived).
    let other = worker("o-other", None, Some(ObservedStatus::Idle), NOW - HOUR);
    inputs
        .workers
        .push(candidate(yard, WorkerAvailability::YardOrchestrator, None));
    inputs
        .workers
        .push(candidate(node, WorkerAvailability::CoordinationNode, None));
    inputs
        .workers
        .push(candidate(gone, WorkerAvailability::CoordinationNode, None));
    inputs.workers.push(candidate(
        other,
        WorkerAvailability::Orchestrator,
        Some("archived"),
    ));
    let before = board::build(&fixture(), None).hidden;
    let built = board::build(&inputs, None);
    let yard = find(&built, "w:w-yard");
    assert_eq!(yard.title, "Yard orchestrator");
    assert_eq!(yard.project_name.as_deref(), Some("Yard"));
    assert_eq!(yard.column, Column::Stale);
    assert_eq!(yard.section, Some(Section::NeedsWork));
    assert!(yard.chips.iter().any(|chip| chip == "yard orchestrator"));
    assert!(!yard.actions.contains(&Action::EndSession));
    let node = find(&built, "w:w-node");
    assert_eq!(node.column, Column::Doing);
    assert_eq!(node.project_name.as_deref(), Some("Yard"));
    let other = find(&built, "w:o-other");
    assert!(other.chips.iter().any(|chip| chip == "orchestrator"));
    // A Yard-level agent that is not live is neither a card nor counted
    // as a record to clean up.
    assert!(built.items.iter().all(|item| item.id != "w:w-node-gone"));
    assert_eq!(built.hidden, before);
    // Herdr decides once read.
    inputs.live_panes = Some(panes(&["main"], &["main"], &["t-w-yard"]));
    let built = board::build(&inputs, None);
    assert!(built.items.iter().any(|item| item.id == "w:w-yard"));
    assert!(built.items.iter().all(|item| item.id != "w:w-node"));
}

#[test]
fn a_failed_assignment_folded_into_the_orchestrator_card_still_needs_you() {
    let mut inputs = fixture();
    let orchestrator = inputs.projects[2].orchestrator.clone();
    let mut failed = assignment(
        "a-by-orchestrator",
        "gamma",
        orchestrator,
        AssignmentLifecycle::Failed,
        NOW - HOUR,
    );
    failed.attempt.error = Some("cargo test failed".to_owned());
    inputs.assignments.push(failed);
    let built = board::build(&inputs, None);
    let card = find(&built, "o:gamma");
    assert_eq!(card.column, Column::NeedsYou);
    assert!(!card.rollup);
    assert!(card.chips.iter().any(|chip| chip == "failed"));
    assert_eq!(card.assignment_id.as_deref(), Some("a-by-orchestrator"));
    assert_eq!(card.status_line, "cargo test failed");
    assert!(
        built
            .items
            .iter()
            .all(|item| item.id != "a:a-by-orchestrator")
    );
    assert!(built.todos.iter().any(|todo| {
        todo.urgency == Urgency::Now && todo.text.starts_with("Find out why \u{201c}gamma")
    }));
}

#[test]
fn a_second_live_record_on_a_shown_pane_is_not_counted_as_not_live() {
    let mut inputs = fixture();
    let before = board::build(&inputs, None).hidden;
    let mut twin = worker("w-twin", None, Some(ObservedStatus::Idle), NOW - HOUR);
    twin.runtime.as_mut().unwrap().pane_id = "p-w-spare".to_owned();
    twin.runtime.as_mut().unwrap().terminal_id = "t-w-spare".to_owned();
    inputs
        .workers
        .push(candidate(twin, WorkerAvailability::UnassignedLive, None));
    let built = board::build(&inputs, None);
    assert!(built.items.iter().all(|item| item.id != "w:w-twin"));
    assert_eq!(built.hidden, before);
}

/// Words a person glancing at the board should never have to decode.
const JARGON: &[&str] = &[
    "receipt",
    "runtime",
    "unassigned_live",
    "resumable",
    "ambiguous",
    "rollup",
    "term_",
    "pane",
    "Turn ended",
    "R0",
    "R1",
    "R3",
    "R4",
    "R5",
    "P1",
    "Triage",
    "loose agent",
    "Herdr",
    "recorded",
    " . ",
    " - ",
    "\u{2014}",
];

fn assert_plain(what: &str, text: &str) {
    for word in JARGON {
        assert!(
            !text.to_lowercase().contains(&word.to_lowercase()),
            "{what} {text:?} contains {word:?}"
        );
    }
}

#[test]
fn every_card_and_todo_reads_in_plain_words() {
    let built = board::build(&fixture(), None);
    for item in &built.items {
        // Only a working agent with a task may have no summary: the card
        // shows its task instead of repeating the column heading.
        assert!(
            !item.summary.is_empty()
                || (item.column == Column::Doing && !item.objective.trim().is_empty()),
            "{} has no summary",
            item.id
        );
        assert!(
            item.summary != "Working right now",
            "{} repeats the column",
            item.id
        );
        assert_plain("summary", &item.summary);
        assert_plain("next step", &item.next_step);
        // Summaries are fragments: no full stop, no semicolon.
        assert!(
            !item.summary.ends_with('.') && !item.summary.contains(';'),
            "{} summary {:?}",
            item.id,
            item.summary
        );
        for label in &item.labels {
            assert_plain("label", label);
            // A chip adds a fact: never what the summary already says.
            assert!(
                !item.summary.to_lowercase().contains(&label.to_lowercase()),
                "{} label {label:?} repeats {:?}",
                item.id,
                item.summary
            );
        }
        assert!(
            !item
                .labels
                .iter()
                .any(|label| label == "working" || label == "quiet"),
            "{} labels {:?} repeat the column",
            item.id,
            item.labels
        );
        if matches!(item.column, Column::Closed | Column::Doing) {
            // "Working now" says "nothing for you to do" in its heading.
            assert!(
                item.next_step.is_empty(),
                "{} next step {:?}",
                item.id,
                item.next_step
            );
        } else {
            assert!(
                (item.next_step.ends_with('.') || item.next_step.ends_with('?'))
                    && !item.next_step.contains(';'),
                "{} next step {:?}",
                item.id,
                item.next_step
            );
        }
    }
    for todo in &built.todos {
        assert_plain("to-do", &todo.text);
    }
    for project in &built.projects {
        assert_plain("project summary", &project.summary);
    }
}

#[test]
fn card_faces_follow_the_templates() {
    let built = board::build(&fixture(), None);
    let review = built
        .items
        .iter()
        .find(|item| item.chips.iter().any(|chip| chip == "ready for review"))
        .expect("a finished turn");
    assert_eq!(review.summary, "Finished, waiting for your review");
    // The summary says it, so the status chip is not repeated.
    assert!(!review.labels.contains(&"finished a turn".to_owned()));
    // An orchestrator whose report asks a question: the card names the
    // question and Open (its terminal) is the first button.
    let asking = built
        .items
        .iter()
        .find(|item| item.id == "o:beta")
        .expect("beta orchestrator");
    assert_eq!(asking.summary, "Has a question for you");
    if asking.actions.first() == Some(&Action::OpenTerminal) {
        assert_eq!(asking.next_step, "Answer its question: use v2 schema?");
    } else {
        assert!(!asking.actions.contains(&Action::OpenTerminal));
        assert_eq!(
            asking.next_step,
            "Open its project page to answer: use v2 schema?"
        );
    }
    assert!(!asking.labels.contains(&"asks for help".to_owned()));
    let failed = built
        .items
        .iter()
        .find(|item| item.chips.iter().any(|chip| chip == "failed"))
        .expect("a failed task");
    assert_eq!(failed.summary, "Its task failed");
    assert_eq!(
        failed.next_step,
        "Open it to see what went wrong, then retry or close it."
    );
    assert_eq!(super::super::plain::duration(3 * 24 * 3_600_000), "3 days");
    assert_eq!(super::super::plain::duration(60_000), "1 min");
    assert_eq!(super::super::plain::duration(2 * 3_600_000), "2 hr");
    assert_eq!(super::super::plain::duration(0), "under a minute");
}

#[test]
fn to_dos_are_orders_naming_the_agent_and_steps_are_one_row() {
    use super::super::plain::{lower_first, steps_todo};
    let who = "\u{201c}Upload refactor\u{201d}";
    let steps = vec![
        "Add a retry test for large uploads.".to_owned(),
        "Split the upload retry into its own module".to_owned(),
    ];
    assert_eq!(
        steps_todo(who, &steps, true).as_deref(),
        Some("Ask \u{201c}Upload refactor\u{201d} to continue (2 steps left) or close it")
    );
    assert_eq!(
        steps_todo(who, &steps[..1], false).as_deref(),
        Some("Ask \u{201c}Upload refactor\u{201d} to continue: add a retry test for large uploads")
    );
    assert_eq!(steps_todo(who, &[], true), None);
    assert_eq!(
        lower_first("Use the v2 invoice schema?"),
        "use the v2 invoice schema?"
    );
    assert_eq!(lower_first("PR merged"), "PR merged");
    assert_eq!(lower_first("Stripe's API"), "Stripe's API");
    let built = board::build(&fixture(), None);
    for todo in &built.todos {
        let named = built.items.iter().any(|item| {
            todo.text
                .contains(&format!("\u{201c}{}\u{201d}", item.title))
        });
        let project_file = todo.sources.iter().all(|source| source.kind != "item");
        assert!(
            named || project_file,
            "{:?} does not name its agent",
            todo.text
        );
        assert!(!todo.text.contains(" asks:"), "{:?}", todo.text);
    }
}

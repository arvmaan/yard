use std::fs;

use yard_domain::{
    AssignmentLifecycle as L, ObservedStatus as S, OrchestratorStatusState, WorkerAvailability as A,
};

use super::read_next_actions;
use super::rules::{
    self, COLD_MS, Column, FRESH_MS, Facts, ItemKind, REVIEW_WINDOW_MS, ReportFacts, Rule, Section,
    Verdict,
};
use super::todos::{self, Todo, TodoKind, TodoSource, Urgency};

const MIN: u64 = 60 * 1000;
const HOUR: u64 = 60 * MIN;

fn active(herdr: S, age_ms: u64) -> Facts {
    Facts {
        availability: Some(A::Assigned),
        lifecycle: Some(L::Active),
        worker_live: true,
        herdr,
        bound_visible: true,
        age_ms,
        ..Facts::neutral(ItemKind::Assignment)
    }
}

fn report(state: OrchestratorStatusState, age_ms: u64) -> ReportFacts {
    ReportFacts {
        state,
        next_empty: true,
        blockers_empty: true,
        age_ms,
        activity_after: false,
    }
}

fn place(facts: &Facts) -> (Column, Option<Section>, Rule) {
    let placement = rules::classify(facts);
    (placement.column, placement.section, placement.rule)
}

#[test]
fn r0_closed_rules() {
    let ended = Facts {
        availability: Some(A::Ended),
        ..active(S::Working, 0)
    };
    assert_eq!(place(&ended).2, Rule::R0);
    for lifecycle in [L::Cancelled, L::HandedOff, L::Completed] {
        let gone = Facts {
            lifecycle: Some(lifecycle),
            worker_live: false,
            herdr: S::Unknown,
            ..active(S::Unknown, 0)
        };
        assert_eq!(place(&gone).0, Column::Closed, "{lifecycle:?}");
    }
    let disposed = Facts {
        lifecycle: Some(L::Failed),
        has_disposition: true,
        ..active(S::Idle, 0)
    };
    assert_eq!(place(&disposed).2, Rule::R0);
}

#[test]
fn failed_without_disposition_needs_you_and_beats_working() {
    let failed = Facts {
        lifecycle: Some(L::Failed),
        ..active(S::Working, 0)
    };
    let placement = rules::classify(&failed);
    assert_eq!(
        (placement.column, placement.rule),
        (Column::NeedsYou, Rule::R1)
    );
    assert!(placement.chips.contains(&"failed".to_owned()));
}

#[test]
fn blocked_beats_working_and_needs_a_visible_binding() {
    let blocked = active(S::Blocked, 0);
    assert_eq!(place(&blocked).2, Rule::R1);
    let hidden = Facts {
        bound_visible: false,
        ..blocked
    };
    assert_ne!(place(&hidden).2, Rule::R1);
    let attention = Facts {
        report: Some(report(OrchestratorStatusState::NeedsAttention, 0)),
        ..active(S::Working, 0)
    };
    assert_eq!(place(&attention).2, Rule::R1);
    let blockers = Facts {
        report: Some(ReportFacts {
            blockers_empty: false,
            ..report(OrchestratorStatusState::Working, 0)
        }),
        ..active(S::Idle, 0)
    };
    assert_eq!(place(&blockers).2, Rule::R1);
    let command = Facts {
        last_command_failed: true,
        ..active(S::Working, 0)
    };
    assert_eq!(place(&command).2, Rule::R1);
    let ambiguous = Facts {
        availability: Some(A::Ambiguous),
        ..active(S::Idle, 0)
    };
    assert_eq!(place(&ambiguous).2, Rule::R1);
}

#[test]
fn r2_working_and_fresh_report_boundary() {
    assert_eq!(place(&active(S::Working, 3 * COLD_MS)).0, Column::Doing);
    let fresh = Facts {
        report: Some(report(OrchestratorStatusState::Working, FRESH_MS - MIN)),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    assert_eq!(place(&fresh).2, Rule::R2);
    let stale = Facts {
        report: Some(report(OrchestratorStatusState::Working, FRESH_MS)),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    assert_ne!(place(&stale).2, Rule::R2);
    // Idle but recent is not "doing now".
    assert_ne!(place(&active(S::Idle, MIN)).0, Column::Doing);
}

#[test]
fn r3_review_window_boundary() {
    let inside = active(S::Done, REVIEW_WINDOW_MS - MIN);
    assert_eq!(place(&inside), (Column::NeedsYou, None, Rule::R3));
    let outside = active(S::Done, REVIEW_WINDOW_MS);
    assert_eq!(place(&outside).0, Column::Stale);
}

#[test]
fn r4_done_needs_hard_evidence() {
    let receipt = Facts {
        lifecycle: Some(L::Completed),
        has_receipt: true,
        has_disposition: true,
        availability: Some(A::UnassignedLive),
        ..active(S::Idle, 2 * HOUR)
    };
    assert_eq!(place(&receipt), (Column::Close, None, Rule::R4));
    let dead = Facts::neutral(ItemKind::DeadAggregate);
    assert_eq!(place(&dead).2, Rule::R4);
    let idle_report = Facts {
        report: Some(report(OrchestratorStatusState::Idle, COLD_MS - MIN)),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    assert_eq!(place(&idle_report).2, Rule::R4);
    // A cold report, or activity after it, is not evidence of done.
    let cold_report = Facts {
        report: Some(report(OrchestratorStatusState::Idle, COLD_MS)),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    assert_ne!(place(&cold_report).0, Column::Close);
    let moved = Facts {
        report: Some(ReportFacts {
            activity_after: true,
            ..report(OrchestratorStatusState::Idle, HOUR)
        }),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    assert_ne!(place(&moved).0, Column::Close);
    let with_next = Facts {
        report: Some(ReportFacts {
            next_empty: false,
            ..report(OrchestratorStatusState::Idle, HOUR)
        }),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    assert_ne!(place(&with_next).0, Column::Close);
}

#[test]
fn r5_stale_sections_and_r7_leftovers() {
    let idle = active(S::Idle, REVIEW_WINDOW_MS);
    assert_eq!(
        place(&idle),
        (Column::Stale, Some(Section::Ambiguous), Rule::R5)
    );
    let explicit = Facts {
        explicit_next: true,
        ..idle
    };
    assert_eq!(place(&explicit).1, Some(Section::NeedsWork));
    let recent = rules::classify(&active(S::Idle, HOUR));
    assert_eq!(recent.rule, Rule::R5);
    assert_eq!(recent.chips, vec!["idle".to_owned()]);
    let resumable = Facts {
        availability: Some(A::Resumable),
        ..Facts::neutral(ItemKind::Worker)
    };
    assert_eq!(place(&resumable).2, Rule::R5);
    let loose = Facts::neutral(ItemKind::LooseAgent);
    assert_eq!(place(&loose).2, Rule::R5);
    let orphan = Facts {
        availability: Some(A::Assigned),
        ..Facts::neutral(ItemKind::Worker)
    };
    let placement = rules::classify(&orphan);
    assert_eq!(
        (placement.column, placement.rule),
        (Column::Stale, Rule::R7)
    );
    assert_eq!(placement.chips, vec!["unknown".to_owned()]);
}

#[test]
fn tiebreak_needs_cold_and_no_open_snapshot_items() {
    let facts = active(S::Idle, COLD_MS - MIN);
    let base = rules::classify(&facts);
    assert_eq!(
        rules::tiebreak(&base, &facts, false).section,
        Some(Section::NeedsWork)
    );
    let cold = active(S::Idle, COLD_MS);
    let base = rules::classify(&cold);
    let settled = rules::tiebreak(&base, &cold, false);
    assert_eq!(
        (settled.column, settled.section),
        (Column::Stale, Some(Section::LikelyDone))
    );
    assert_eq!(
        rules::tiebreak(&base, &cold, true).section,
        Some(Section::NeedsWork)
    );
}

#[test]
fn model_verdicts_refine_only_ambiguous_and_never_yield_done() {
    let facts = active(S::Idle, COLD_MS);
    let base = rules::classify(&facts);
    let finished = rules::apply_verdict(&base, Verdict::Finished, 0.9);
    assert_eq!(
        (finished.column, finished.section),
        (Column::Stale, Some(Section::LikelyDone))
    );
    let waiting = rules::apply_verdict(&base, Verdict::WaitingOnUser, 0.7);
    assert_eq!(waiting.column, Column::NeedsYou);
    let unsure = rules::apply_verdict(&base, Verdict::Finished, 0.59);
    assert_eq!(unsure.section, Some(Section::NeedsWork));
    assert!(unsure.chips.contains(&"?".to_owned()));
    let unclear = rules::apply_verdict(&base, Verdict::Unclear, 0.99);
    assert!(unclear.chips.contains(&"?".to_owned()));
    // R0-R4 and explicit "needs work" are untouchable.
    for fixed in [
        active(S::Working, 0),
        active(S::Blocked, 0),
        active(S::Done, HOUR),
        Facts {
            explicit_next: true,
            ..facts
        },
        Facts {
            availability: Some(A::Ended),
            ..facts
        },
    ] {
        let base = rules::classify(&fixed);
        for verdict in [
            Verdict::Finished,
            Verdict::WaitingOnUser,
            Verdict::NeedsWork,
            Verdict::Unclear,
        ] {
            assert_eq!(rules::apply_verdict(&base, verdict, 1.0), base);
        }
    }
}

/// Exhaustive check over the discrete state space (in place of a random
/// property test): every input yields exactly one placement, the model never
/// produces Done, and a working or blocked item is never Closed by P1.
#[test]
#[allow(clippy::too_many_lines)]
fn every_state_yields_exactly_one_placement() {
    let kinds = [
        ItemKind::Assignment,
        ItemKind::Worker,
        ItemKind::LooseAgent,
        ItemKind::DeadAggregate,
        ItemKind::Orchestrator,
    ];
    let availabilities = [
        None,
        Some(A::Assigned),
        Some(A::UnassignedLive),
        Some(A::Resumable),
        Some(A::Unavailable),
        Some(A::Ambiguous),
        Some(A::Ended),
        Some(A::Orchestrator),
    ];
    let lifecycles = [
        None,
        Some(L::Allocating),
        Some(L::Active),
        Some(L::HandingOff),
        Some(L::HandedOff),
        Some(L::Completed),
        Some(L::Failed),
        Some(L::Cancelled),
    ];
    let statuses = [S::Idle, S::Working, S::Blocked, S::Done, S::Unknown];
    let reports = [
        None,
        Some(report(OrchestratorStatusState::Working, MIN)),
        Some(report(OrchestratorStatusState::Idle, HOUR)),
        Some(report(OrchestratorStatusState::NeedsAttention, HOUR)),
    ];
    let ages = [0, FRESH_MS, REVIEW_WINDOW_MS, COLD_MS];
    let mut count = 0_usize;
    for kind in kinds {
        for availability in availabilities {
            for lifecycle in lifecycles {
                for herdr in statuses {
                    for report in reports {
                        for age_ms in ages {
                            for bits in 0_u8..32 {
                                let facts = Facts {
                                    kind,
                                    availability,
                                    lifecycle,
                                    has_disposition: bits & 1 != 0,
                                    has_receipt: bits & 2 != 0,
                                    worker_live: bits & 4 != 0,
                                    herdr,
                                    bound_visible: bits & 8 != 0,
                                    question_pending: false,
                                    report,
                                    last_command_failed: false,
                                    age_ms,
                                    explicit_next: bits & 16 != 0,
                                };
                                let base = rules::classify(&facts);
                                count += 1;
                                assert_eq!(
                                    base.section.is_some(),
                                    base.column == Column::Stale,
                                    "{facts:?}"
                                );
                                for verdict in [
                                    Verdict::Finished,
                                    Verdict::WaitingOnUser,
                                    Verdict::NeedsWork,
                                    Verdict::Unclear,
                                ] {
                                    let refined = rules::apply_verdict(&base, verdict, 1.0);
                                    if !matches!(base.column, Column::Close | Column::Closed) {
                                        assert_ne!(refined.column, Column::Close);
                                        assert_ne!(refined.column, Column::Closed);
                                    }
                                    if base.rule != Rule::R5 {
                                        assert_eq!(refined, base);
                                    }
                                }
                                let settled = rules::tiebreak(&base, &facts, false);
                                if base.column != Column::Stale {
                                    assert_eq!(settled, base);
                                }
                                if herdr == S::Working && base.column == Column::Closed {
                                    assert!(
                                        availability == Some(A::Ended)
                                            || matches!(
                                                lifecycle,
                                                Some(
                                                    L::Failed
                                                        | L::Cancelled
                                                        | L::HandedOff
                                                        | L::Completed
                                                )
                                            ),
                                        "{facts:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, 5 * 8 * 8 * 5 * 4 * 4 * 32);
}

#[test]
fn heat_buckets() {
    assert_eq!(rules::heat(HOUR - 1), rules::Heat::Hot);
    assert_eq!(rules::heat(HOUR), rules::Heat::Warm);
    assert_eq!(rules::heat(REVIEW_WINDOW_MS), rules::Heat::Cool);
    assert_eq!(rules::heat(COLD_MS), rules::Heat::Cold);
}

#[test]
fn next_actions_parse_skips_fences_and_checked_items() {
    let text = "# Next\n- [ ] Add retry test\n- [x] Done thing\n* Write docs\n  - nested bullet\n1. Ship it\n```\n- not this\n```\nplain prose\n";
    let lines = todos::parse_next_actions(text);
    let texts = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        ["Add retry test", "Write docs", "nested bullet", "Ship it"]
    );
    assert!(lines[0].checkbox);
    assert!(!lines[1].checkbox);
    let many = (0..30)
        .map(|index| format!("- item {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        todos::parse_next_actions(&many).len(),
        todos::MAX_SNAPSHOT_ITEMS
    );
    let long = format!("- {}", "x".repeat(500));
    assert!(todos::parse_next_actions(&long)[0].text.chars().count() <= todos::MAX_TODO_CHARS);
}

#[test]
fn next_actions_reader_rejects_symlinks_outside_root_and_oversize() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("knowledge");
    let node = root.join("nodes").join("node-1");
    let folder = node
        .join("snapshots")
        .join("s1")
        .join("projects")
        .join("p1");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("next-actions.md"), "- [ ] one\n").unwrap();
    assert_eq!(
        read_next_actions(&root, "node-1", &folder)
            .unwrap()
            .as_deref(),
        Some("- [ ] one\n")
    );
    // Missing file: nothing, not an error.
    let empty = node.join("snapshots").join("s2");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(read_next_actions(&root, "node-1", &empty).unwrap(), None);
    // Outside the node's folder.
    let outside = temp.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("next-actions.md"), "- [ ] leak\n").unwrap();
    assert!(read_next_actions(&root, "node-1", &outside).is_err());
    // A symlinked folder and a symlinked file.
    let linked = node.join("snapshots").join("linked");
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    assert!(read_next_actions(&root, "node-1", &linked).is_err());
    let file_link = node.join("snapshots").join("s3");
    fs::create_dir_all(&file_link).unwrap();
    std::os::unix::fs::symlink(
        outside.join("next-actions.md"),
        file_link.join("next-actions.md"),
    )
    .unwrap();
    assert!(read_next_actions(&root, "node-1", &file_link).is_err());
    // Oversize.
    fs::write(folder.join("next-actions.md"), "x".repeat(64 * 1024 + 1)).unwrap();
    assert!(read_next_actions(&root, "node-1", &folder).is_err());
    assert!(read_next_actions(&root, "../nodes", &folder).is_err());
}

fn todo(text: &str, urgency: Urgency, project: &str, source: &str) -> Todo {
    Todo {
        id: todos::todo_id(text),
        text: text.to_owned(),
        kind: TodoKind::Next,
        urgency,
        project_id: Some(project.to_owned()),
        sources: vec![TodoSource {
            kind: "item".to_owned(),
            id: source.to_owned(),
        }],
        age_ms: 0,
    }
}

#[test]
fn dedupe_merges_exact_and_near_duplicates_within_a_project() {
    let merged = todos::dedupe(vec![
        todo(
            "Add retry test for upload path",
            Urgency::Later,
            "alpha",
            "a",
        ),
        todo(
            "add **retry** test for upload path!",
            Urgency::Now,
            "alpha",
            "b",
        ),
        todo(
            "Add the retry test for upload path",
            Urgency::Soon,
            "alpha",
            "c",
        ),
        todo("Add retry test for upload path", Urgency::Soon, "beta", "d"),
        todo("Write release notes", Urgency::Soon, "alpha", "e"),
    ]);
    assert_eq!(merged.len(), 3, "{merged:?}");
    assert_eq!(merged[0].urgency, Urgency::Now);
    // Exact (b) and near (c, Jaccard 6/7) duplicates within alpha merge.
    assert_eq!(merged[0].sources.len(), 3);
    // The same bullet in another project stays its own to-do, with its own
    // project and a distinct id.
    assert_eq!(merged[1].project_id.as_deref(), Some("beta"));
    assert_ne!(merged[1].id, merged[0].id);
    assert_eq!(todos::todo_id("Fix it."), todos::todo_id("fix   IT"));
}

#[test]
fn generated_todos_of_different_items_never_fuzzy_merge() {
    let mut one = todo(
        "Review finished turn: shared long worker name alpha one",
        Urgency::Now,
        "alpha",
        "w1",
    );
    let mut two = todo(
        "Review finished turn: shared long worker name alpha two",
        Urgency::Now,
        "alpha",
        "w2",
    );
    one.kind = TodoKind::Review;
    two.kind = TodoKind::Review;
    let merged = todos::dedupe(vec![one, two]);
    assert_eq!(merged.len(), 2, "{merged:?}");
}

#[test]
fn split_bullets_and_redaction_of_todo_text() {
    let bullets = todos::split_bullets("- first; second\n2. third\n\n");
    assert_eq!(bullets, ["first", "second", "third"]);
    let secret = todos::split_bullets("rotate token=abcd1234 in /Users/someone/project/file.rs");
    assert!(!secret[0].contains("abcd1234"));
    assert!(!secret[0].contains("/Users/"));
}

#[path = "board_tests.rs"]
pub(crate) mod board_tests;

#[cfg(test)]
#[path = "relevance_tests.rs"]
mod relevance_tests;

#[cfg(test)]
#[path = "round1_tests.rs"]
mod round1_tests;

#[cfg(test)]
#[path = "round2_tests.rs"]
mod round2_tests;

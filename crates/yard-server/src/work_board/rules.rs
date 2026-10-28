//! Placement rules of the work board: one pure, total function from the
//! deterministic facts of an item to its column, section and rule.
//!
//! First match wins (R0, R1, R2, R3, R4, R5, R7). The model post-pass (P1)
//! runs afterwards and only refines R5 "ambiguous" items; see
//! [`apply_verdict`] and [`tiebreak`].

use serde::{Deserialize, Serialize};
use yard_domain::{
    AssignmentLifecycle, ObservedStatus, OrchestratorStatusState, WorkerAvailability,
};

/// Younger than this, a working status report still counts as "doing now".
pub(crate) const FRESH_MS: u64 = 15 * 60 * 1000;
/// A finished turn waits this long in "needs you (review)" before it is stale.
pub(crate) const REVIEW_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
/// Idle this long with nothing open, an item is probably done.
pub(crate) const COLD_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// The model must be at least this sure before its verdict moves a card.
pub(crate) const MODEL_CONFIDENCE: f64 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Column {
    Doing,
    NeedsYou,
    Stale,
    Close,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Section {
    /// Stale with an explicit open signal (or a model `needs_work`).
    NeedsWork,
    /// Stale and undecided: the only section the model post-pass refines.
    Ambiguous,
    /// Stale and probably finished; the user decides with one click.
    LikelyDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Rule {
    R0,
    R1,
    R2,
    R3,
    R4,
    R5,
    R7,
    P1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ItemKind {
    Assignment,
    Worker,
    LooseAgent,
    DeadAggregate,
    /// A project orchestrator: rolled up into its project header unless it
    /// needs the user.
    Orchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Heat {
    Hot,
    Warm,
    Cool,
    Cold,
}

/// The orchestrator status report as the rules see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportFacts {
    pub state: OrchestratorStatusState,
    pub next_empty: bool,
    pub blockers_empty: bool,
    pub age_ms: u64,
    /// Something happened after the report was written.
    pub activity_after: bool,
}

/// Every deterministic signal an item's placement depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Facts {
    pub kind: ItemKind,
    pub availability: Option<WorkerAvailability>,
    pub lifecycle: Option<AssignmentLifecycle>,
    /// A failed assignment has a recorded disposition (receipt or cancellation).
    pub has_disposition: bool,
    pub has_receipt: bool,
    /// The bound runtime is observed and its process has not exited.
    pub worker_live: bool,
    pub herdr: ObservedStatus,
    /// The Herdr status belongs to a bound, observed runtime of visible work.
    pub bound_visible: bool,
    pub question_pending: bool,
    pub report: Option<ReportFacts>,
    pub last_command_failed: bool,
    pub age_ms: u64,
    /// An explicit open signal: next/blockers, receipt blockers or an
    /// unchecked snapshot item for this item's project.
    pub explicit_next: bool,
}

impl Facts {
    /// Facts with every signal neutral, for tests and simple kinds.
    pub(crate) const fn neutral(kind: ItemKind) -> Self {
        Self {
            kind,
            availability: None,
            lifecycle: None,
            has_disposition: false,
            has_receipt: false,
            worker_live: false,
            herdr: ObservedStatus::Unknown,
            bound_visible: false,
            question_pending: false,
            report: None,
            last_command_failed: false,
            age_ms: 0,
            explicit_next: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Placement {
    pub column: Column,
    pub section: Option<Section>,
    pub rule: Rule,
    pub chips: Vec<String>,
}

impl Placement {
    fn new(column: Column, section: Option<Section>, rule: Rule, chips: &[&str]) -> Self {
        Self {
            column,
            section,
            rule,
            chips: chips.iter().map(|chip| (*chip).to_owned()).collect(),
        }
    }
}

/// The heat bucket of an age: hot < 1 h, warm < 24 h, cool < 7 d, else cold.
pub(crate) const fn heat(age_ms: u64) -> Heat {
    if age_ms < 60 * 60 * 1000 {
        Heat::Hot
    } else if age_ms < REVIEW_WINDOW_MS {
        Heat::Warm
    } else if age_ms < COLD_MS {
        Heat::Cool
    } else {
        Heat::Cold
    }
}

/// Place one item. Total: every input yields exactly one placement.
pub(crate) fn classify(facts: &Facts) -> Placement {
    use AssignmentLifecycle as L;
    if let Some(placement) = closed(facts) {
        return placement;
    }
    if let Some(placement) = needs_you(facts) {
        return placement;
    }
    // R2: doing now.
    if facts.herdr == ObservedStatus::Working {
        return Placement::new(Column::Doing, None, Rule::R2, &["working"]);
    }
    if let Some(report) = facts.report
        && report.state == OrchestratorStatusState::Working
        && report.age_ms < FRESH_MS
    {
        return Placement::new(Column::Doing, None, Rule::R2, &["reported working"]);
    }
    if matches!(facts.lifecycle, Some(L::Allocating | L::HandingOff)) {
        return Placement::new(Column::Doing, None, Rule::R2, &["starting"]);
    }
    // R3: a finished, unseen turn inside the review window.
    if facts.lifecycle == Some(L::Active)
        && facts.herdr == ObservedStatus::Done
        && facts.age_ms < REVIEW_WINDOW_MS
    {
        return Placement::new(Column::NeedsYou, None, Rule::R3, &["ready for review"]);
    }
    if let Some(placement) = done(facts) {
        return placement;
    }
    if let Some(placement) = stale(facts) {
        return placement;
    }
    // R7: anything left over stays visible.
    Placement::new(
        Column::Stale,
        Some(Section::NeedsWork),
        Rule::R7,
        &["unknown"],
    )
}

/// R0: closed (hidden unless "Show closed").
fn closed(facts: &Facts) -> Option<Placement> {
    use AssignmentLifecycle as L;
    let ended = facts.availability == Some(WorkerAvailability::Ended);
    let gone = matches!(
        facts.lifecycle,
        Some(L::Cancelled | L::HandedOff | L::Completed)
    ) && !facts.worker_live
        && facts.kind != ItemKind::DeadAggregate;
    let disposed = facts.lifecycle == Some(L::Failed) && facts.has_disposition;
    (ended || gone || disposed).then(|| Placement::new(Column::Closed, None, Rule::R0, &[]))
}

/// R1: needs you (blocked, failed, ambiguous, a question, a failed command).
fn needs_you(facts: &Facts) -> Option<Placement> {
    let mut chips = Vec::new();
    if facts.lifecycle == Some(AssignmentLifecycle::Failed) {
        chips.push("failed");
    }
    if facts.availability == Some(WorkerAvailability::Ambiguous) {
        chips.push("ambiguous runtime");
    }
    if facts.herdr == ObservedStatus::Blocked && facts.bound_visible {
        chips.push("blocked");
    }
    if facts.question_pending {
        chips.push("question");
    }
    if let Some(report) = facts.report
        && (report.state == OrchestratorStatusState::NeedsAttention || !report.blockers_empty)
    {
        chips.push("needs attention");
    }
    if facts.last_command_failed {
        chips.push("command failed");
    }
    (!chips.is_empty()).then(|| Placement::new(Column::NeedsYou, None, Rule::R1, &chips))
}

/// R4: done, can close (hard evidence only).
fn done(facts: &Facts) -> Option<Placement> {
    if facts.kind == ItemKind::DeadAggregate {
        return Some(Placement::new(
            Column::Close,
            None,
            Rule::R4,
            &["dead runtimes"],
        ));
    }
    if facts.lifecycle == Some(AssignmentLifecycle::Completed)
        && facts.has_receipt
        && facts.worker_live
    {
        return Some(Placement::new(Column::Close, None, Rule::R4, &["receipt"]));
    }
    if let Some(report) = facts.report
        && report.state == OrchestratorStatusState::Idle
        && report.next_empty
        && report.blockers_empty
        && report.age_ms < COLD_MS
        && !report.activity_after
    {
        return Some(Placement::new(
            Column::Close,
            None,
            Rule::R4,
            &["reported nothing next"],
        ));
    }
    None
}

/// R5: open but stale.
fn stale(facts: &Facts) -> Option<Placement> {
    // A live Yard-level agent (Yard orchestrator, workstream, orchestrator
    // of a project not on the board) that is not working or blocked is
    // simply open: never "ambiguous", so neither the model nor the
    // tiebreak can call it likely done.
    if facts.kind == ItemKind::Worker {
        let role = match facts.availability {
            Some(WorkerAvailability::YardOrchestrator) => Some("yard orchestrator"),
            Some(WorkerAvailability::CoordinationNode) => Some("workstream"),
            Some(WorkerAvailability::Orchestrator) => Some("orchestrator"),
            _ => None,
        };
        if let Some(role) = role {
            return Some(Placement::new(
                Column::Stale,
                Some(Section::NeedsWork),
                Rule::R5,
                &[role],
            ));
        }
    }
    let section = if facts.explicit_next {
        Section::NeedsWork
    } else {
        Section::Ambiguous
    };
    let chip = if facts.lifecycle == Some(AssignmentLifecycle::Active) {
        if facts.age_ms >= REVIEW_WINDOW_MS {
            Some("stale")
        } else {
            Some("idle")
        }
    } else if facts.availability == Some(WorkerAvailability::Resumable) {
        Some("resumable")
    } else if facts.availability == Some(WorkerAvailability::UnassignedLive) && !facts.has_receipt {
        Some("unassigned")
    } else if facts.kind == ItemKind::LooseAgent {
        Some("loose agent")
    } else {
        None
    }?;
    Some(Placement::new(
        Column::Stale,
        Some(section),
        Rule::R5,
        &[chip],
    ))
}

/// The model's verdict on an ambiguous item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    NeedsWork,
    Finished,
    WaitingOnUser,
    Unclear,
}

/// P1 with the model: refine an R5 "ambiguous" placement. Any other
/// placement is returned unchanged, so the model can never move a card into
/// Done, out of R0-R4, or out of an explicit "needs work".
pub(crate) fn apply_verdict(placement: &Placement, verdict: Verdict, confidence: f64) -> Placement {
    if placement.rule != Rule::R5 || placement.section != Some(Section::Ambiguous) {
        return placement.clone();
    }
    let mut chips = placement.chips.clone();
    let sure = confidence >= MODEL_CONFIDENCE;
    let (column, section) = match verdict {
        Verdict::Finished if sure => (Column::Stale, Section::LikelyDone),
        Verdict::WaitingOnUser if sure => {
            chips.push("waiting on you".to_owned());
            (Column::NeedsYou, Section::NeedsWork)
        }
        Verdict::NeedsWork => (Column::Stale, Section::NeedsWork),
        _ => {
            chips.push("?".to_owned());
            (Column::Stale, Section::NeedsWork)
        }
    };
    Placement {
        column,
        section: (column == Column::Stale).then_some(section),
        rule: Rule::P1,
        chips,
    }
}

/// P1 without the model (off or failed): "likely done?" only when nothing is
/// open in the project snapshot and the item is cold; otherwise "needs work".
/// Never Done.
pub(crate) fn tiebreak(placement: &Placement, facts: &Facts, snapshot_open: bool) -> Placement {
    if placement.rule != Rule::R5 || placement.section != Some(Section::Ambiguous) {
        return placement.clone();
    }
    let likely_done = !facts.explicit_next && !snapshot_open && facts.age_ms >= COLD_MS;
    Placement {
        column: Column::Stale,
        section: Some(if likely_done {
            Section::LikelyDone
        } else {
            Section::NeedsWork
        }),
        rule: Rule::P1,
        chips: placement.chips.clone(),
    }
}

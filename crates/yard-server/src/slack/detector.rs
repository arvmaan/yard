//! Server-side attention detection with Herdr's anti-spam rules.
//!
//! Signals (all from durable state written by reconciliation or command
//! services; the browser is not involved):
//! - **Blocked**: a bound, observed, not-exited runtime of visible work
//!   (assignment worker, project orchestrator, superintendent, workstream)
//!   has Herdr status `blocked`. This is the web's "Blocked: needs input".
//! - **Ready for review**: an assignment worker moves from `working` or
//!   `blocked` to `done` (Herdr: idle and not yet seen). This is the web's
//!   "Ready for review"; orchestrators finishing a turn are not notified,
//!   matching `projectUpdates`. A turn that Yard's automatic summary request
//!   (`yard:auto:*`) started is left out: the latest assignment prompt is
//!   automatic and was created during the quiet stretch before the turn
//!   (with [`AUTOMATIC_PROMPT_TOLERANCE_MS`] for the store-read lag).
//! - **Command failed / ambiguous**: a prompt, route, allocation, handoff or
//!   disposition acknowledgement ends `failed` or `ambiguous`.
//!
//! Rules: a silent baseline on start (nothing already true is announced);
//! one pending slot per worker that a newer episode replaces; a settle delay
//! followed by a re-check that the state, the Herdr change sequence and the
//! provider session are unchanged; claim once per (worker, state) until the
//! state changes; suppression while the owner views that agent; nothing for
//! ended, archived or deleted work (the store never returns it).

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use yard_domain::{ObservedStatus, RuntimeObservationState, RuntimeProcessState};
use yard_store::{
    ATTENTION_COMMAND_BATCH_LIMIT, AttentionCommand, AttentionCommandStatus, AttentionRecords,
    AttentionRuntime, AttentionRuntimeRole,
};

use super::presence::ViewTarget;

/// Re-read commands this far behind the watermark, because timestamps are
/// taken before commit and a slower transaction can land "in the past".
pub const COMMAND_LOOKBACK_MS: u64 = 60_000;

/// An automatic prompt created this long before the detector first saw the
/// worker quiet still counts as starting the next turn: the automation reads
/// the store's status, which the detector sees up to one tick later.
pub const AUTOMATIC_PROMPT_TOLERANCE_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorTiming {
    pub blocked_settle: Duration,
    pub ready_settle: Duration,
    pub command_settle: Duration,
}

impl Default for DetectorTiming {
    fn default() -> Self {
        Self {
            blocked_settle: Duration::from_secs(15),
            ready_settle: Duration::from_secs(20),
            command_settle: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttentionKind {
    Blocked,
    ReadyForReview,
    CommandFailed,
    CommandAmbiguous,
}

impl AttentionKind {
    /// Act-now kinds need the owner; read-later kinds can wait.
    #[must_use]
    pub const fn act_now(self) -> bool {
        !matches!(self, Self::ReadyForReview)
    }
}

/// Who or what the notification is about (titles only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Worker {
        profile_name: Option<String>,
        objective: Option<String>,
    },
    ProjectOrchestrator,
    YardOrchestrator,
    Workstream {
        name: Option<String>,
    },
    Command {
        command_type: String,
        objective: Option<String>,
        node_name: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventSource {
    Runtime {
        worker_id: String,
        status: ObservedStatus,
    },
    Command {
        command_id: String,
        node_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionEvent {
    pub kind: AttentionKind,
    pub source: EventSource,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub subject: Subject,
    pub view_target: Option<ViewTarget>,
    /// When the state was first observed (for the message time).
    pub observed_at_unix_ms: u64,
    /// When the event passed its settle re-check.
    pub ready_at: Instant,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub events: Vec<AttentionEvent>,
    /// Events that settled while the owner was viewing the agent.
    pub suppressed: usize,
}

#[derive(Debug)]
struct WorkerState {
    status: ObservedStatus,
    /// The status already announced (or suppressed); cleared when the
    /// status moves away from it.
    claimed: Option<ObservedStatus>,
    pending: Option<PendingRuntime>,
    /// When the detector first saw the current (or latest) quiet stretch
    /// (`idle`/`done`).
    quiet_since_unix_ms: Option<u64>,
    /// The quiet stretch that preceded the current turn.
    turn_after_quiet_since_unix_ms: Option<u64>,
}

#[derive(Debug)]
struct PendingRuntime {
    kind: AttentionKind,
    since: Instant,
    since_unix_ms: u64,
    sequence: u64,
    provider_session: Option<String>,
}

#[derive(Debug)]
struct PendingCommand {
    since: Instant,
    since_unix_ms: u64,
}

#[derive(Debug)]
pub struct Detector {
    timing: DetectorTiming,
    primed: bool,
    workers: HashMap<String, WorkerState>,
    command_watermark_unix_ms: u64,
    command_batch_full: bool,
    commands_seen: HashMap<String, u64>,
    commands_pending: HashMap<String, PendingCommand>,
    visible_project_ids: HashSet<String>,
    visible_node_ids: HashSet<String>,
}

impl Detector {
    /// `started_at_unix_ms` is the command baseline: acknowledgements that
    /// ended before it are never announced.
    #[must_use]
    pub fn new(timing: DetectorTiming, started_at_unix_ms: u64) -> Self {
        Self {
            timing,
            primed: false,
            workers: HashMap::new(),
            command_watermark_unix_ms: started_at_unix_ms,
            command_batch_full: false,
            commands_seen: HashMap::new(),
            commands_pending: HashMap::new(),
            visible_project_ids: HashSet::new(),
            visible_node_ids: HashSet::new(),
        }
    }

    /// The `updated_after` bound for the next command read.
    #[must_use]
    pub const fn commands_query_after(&self) -> u64 {
        if self.command_batch_full {
            self.command_watermark_unix_ms
        } else {
            self.command_watermark_unix_ms
                .saturating_sub(COMMAND_LOOKBACK_MS)
        }
    }

    /// Diff one durable snapshot against the previous ones.
    pub fn observe(
        &mut self,
        records: &AttentionRecords,
        now: Instant,
        now_unix_ms: u64,
        viewing: impl Fn(&ViewTarget) -> bool,
    ) -> Observation {
        let mut observation = Observation::default();
        let primed = self.primed;
        self.primed = true;
        let mut present = HashSet::with_capacity(records.runtimes.len());
        for runtime in &records.runtimes {
            if !present.insert(runtime.worker_id.clone()) {
                continue;
            }
            self.observe_runtime(
                runtime,
                primed,
                now,
                now_unix_ms,
                &viewing,
                &mut observation,
            );
        }
        // Ended, archived, deleted or unbound workers leave the snapshot;
        // their pending slots go with them.
        self.workers
            .retain(|worker_id, _| present.contains(worker_id));
        self.visible_project_ids
            .clone_from(&records.visible_project_ids);
        self.visible_node_ids.clone_from(&records.visible_node_ids);
        self.observe_commands(
            &records.commands,
            primed,
            now,
            now_unix_ms,
            &viewing,
            &mut observation,
        );
        observation
    }

    /// Re-check a queued event just before it is sent.
    #[must_use]
    pub fn still_true(&self, event: &AttentionEvent) -> bool {
        match &event.source {
            EventSource::Runtime { worker_id, status } => self
                .workers
                .get(worker_id)
                .is_some_and(|worker| worker.status == *status),
            // Commands are re-listed only near the watermark, so check that
            // their project and workstream are still neither archived nor
            // deleted.
            EventSource::Command { node_id, .. } => {
                event
                    .project_id
                    .as_ref()
                    .is_none_or(|project_id| self.visible_project_ids.contains(project_id))
                    && node_id
                        .as_ref()
                        .is_none_or(|node_id| self.visible_node_ids.contains(node_id))
            }
        }
    }

    fn observe_runtime(
        &mut self,
        runtime: &AttentionRuntime,
        primed: bool,
        now: Instant,
        now_unix_ms: u64,
        viewing: &impl Fn(&ViewTarget) -> bool,
        observation: &mut Observation,
    ) {
        let live = runtime.observation_state == RuntimeObservationState::Observed
            && runtime.process_state != RuntimeProcessState::Exited;
        let Some(worker) = self.workers.get_mut(&runtime.worker_id) else {
            self.workers.insert(
                runtime.worker_id.clone(),
                first_sight(runtime, primed, live, now, now_unix_ms),
            );
            return;
        };
        let previous = worker.status;
        worker.status = runtime.status;
        if is_quiet(runtime.status) && !is_quiet(previous) {
            worker.quiet_since_unix_ms = Some(now_unix_ms);
        } else if is_active(runtime.status) && !is_active(previous) {
            worker.turn_after_quiet_since_unix_ms = worker.quiet_since_unix_ms;
        }
        if worker
            .claimed
            .is_some_and(|claimed| claimed != runtime.status)
        {
            worker.claimed = None;
        }
        let signal = if !live || worker.claimed == Some(runtime.status) {
            None
        } else {
            match runtime.status {
                ObservedStatus::Blocked => Some(AttentionKind::Blocked),
                ObservedStatus::Done
                    if runtime.role == AttentionRuntimeRole::Assignment
                        && (matches!(
                            previous,
                            ObservedStatus::Working | ObservedStatus::Blocked
                        ) || worker.pending.as_ref().is_some_and(|pending| {
                            pending.kind == AttentionKind::ReadyForReview
                        })) =>
                {
                    Some(AttentionKind::ReadyForReview)
                }
                _ => None,
            }
        };
        let Some(kind) = signal else {
            worker.pending = None;
            return;
        };
        let same_episode = worker.pending.as_ref().is_some_and(|pending| {
            pending.kind == kind
                && pending.sequence == runtime.state_change_sequence
                && pending.provider_session == runtime.provider_session
        });
        if !same_episode {
            // A newer episode (or a different agent) replaces the slot and
            // restarts the settle window.
            worker.pending = Some(PendingRuntime {
                kind,
                since: now,
                since_unix_ms: now_unix_ms,
                sequence: runtime.state_change_sequence,
                provider_session: runtime.provider_session.clone(),
            });
            return;
        }
        let settle = match kind {
            AttentionKind::Blocked => self.timing.blocked_settle,
            _ => self.timing.ready_settle,
        };
        let pending = worker.pending.as_ref().expect("same episode has a slot");
        if now.saturating_duration_since(pending.since) < settle {
            return;
        }
        let since_unix_ms = pending.since_unix_ms;
        worker.pending = None;
        worker.claimed = Some(runtime.status);
        if kind == AttentionKind::ReadyForReview
            && automatic_turn(runtime, worker.turn_after_quiet_since_unix_ms)
        {
            // Yard asked for a summary; the owner did not start this turn.
            return;
        }
        let event = runtime_event(runtime, kind, since_unix_ms, now);
        if event.view_target.as_ref().is_some_and(viewing) {
            observation.suppressed += 1;
        } else {
            observation.events.push(event);
        }
    }

    fn observe_commands(
        &mut self,
        commands: &[AttentionCommand],
        primed: bool,
        now: Instant,
        now_unix_ms: u64,
        viewing: &impl Fn(&ViewTarget) -> bool,
        observation: &mut Observation,
    ) {
        let mut listed = HashSet::with_capacity(commands.len());
        for command in commands {
            listed.insert(command.command_id.as_str());
            self.command_watermark_unix_ms = self
                .command_watermark_unix_ms
                .max(command.updated_at_unix_ms);
            if self
                .commands_seen
                .insert(command.command_id.clone(), command.updated_at_unix_ms)
                .is_some()
            {
                continue;
            }
            if primed {
                self.commands_pending.insert(
                    command.command_id.clone(),
                    PendingCommand {
                        since: now,
                        since_unix_ms: now_unix_ms,
                    },
                );
            }
        }
        self.command_batch_full = commands.len() >= ATTENTION_COMMAND_BATCH_LIMIT;
        let settle = self.timing.command_settle;
        let mut settled = Vec::new();
        self.commands_pending.retain(|command_id, pending| {
            // Re-check: the command must still be listed, which also drops
            // it once its project is archived or deleted.
            if !listed.contains(command_id.as_str()) {
                return false;
            }
            if now.saturating_duration_since(pending.since) < settle {
                return true;
            }
            settled.push((command_id.clone(), pending.since_unix_ms));
            false
        });
        for (command_id, since_unix_ms) in settled {
            let Some(command) = commands
                .iter()
                .find(|command| command.command_id == command_id)
            else {
                continue;
            };
            let event = command_event(command, since_unix_ms, now);
            if event.view_target.as_ref().is_some_and(viewing) {
                observation.suppressed += 1;
            } else {
                observation.events.push(event);
            }
        }
        let floor = self
            .command_watermark_unix_ms
            .saturating_sub(2 * COMMAND_LOOKBACK_MS);
        self.commands_seen
            .retain(|_, updated_at| *updated_at >= floor);
    }
}

/// A worker's first snapshot. Before priming this is the silent baseline: a
/// state that is already true counts as announced.
fn first_sight(
    runtime: &AttentionRuntime,
    primed: bool,
    live: bool,
    now: Instant,
    now_unix_ms: u64,
) -> WorkerState {
    let claimed = (!primed
        && matches!(
            runtime.status,
            ObservedStatus::Blocked | ObservedStatus::Done
        ))
    .then_some(runtime.status);
    let pending =
        (primed && live && runtime.status == ObservedStatus::Blocked).then(|| PendingRuntime {
            kind: AttentionKind::Blocked,
            since: now,
            since_unix_ms: now_unix_ms,
            sequence: runtime.state_change_sequence,
            provider_session: runtime.provider_session.clone(),
        });
    WorkerState {
        status: runtime.status,
        claimed,
        pending,
        quiet_since_unix_ms: is_quiet(runtime.status).then_some(now_unix_ms),
        turn_after_quiet_since_unix_ms: None,
    }
}

const fn is_quiet(status: ObservedStatus) -> bool {
    matches!(status, ObservedStatus::Idle | ObservedStatus::Done)
}

const fn is_active(status: ObservedStatus) -> bool {
    matches!(status, ObservedStatus::Working | ObservedStatus::Blocked)
}

/// Whether the turn that just finished was started by an automatic summary
/// request: the latest prompt is automatic and was created during the quiet
/// stretch before the turn. Without a known quiet stretch (the worker was
/// already working when Yard started) the turn is not treated as automatic.
fn automatic_turn(runtime: &AttentionRuntime, turn_after_quiet_since: Option<u64>) -> bool {
    runtime.last_prompt_automatic
        && runtime
            .last_prompt_at_unix_ms
            .zip(turn_after_quiet_since)
            .is_some_and(|(prompt_at, quiet_since)| {
                prompt_at.saturating_add(AUTOMATIC_PROMPT_TOLERANCE_MS) >= quiet_since
            })
}

fn runtime_event(
    runtime: &AttentionRuntime,
    kind: AttentionKind,
    since_unix_ms: u64,
    now: Instant,
) -> AttentionEvent {
    let (subject, view_target) = match runtime.role {
        AttentionRuntimeRole::Assignment => (
            Subject::Worker {
                profile_name: runtime.profile_name.clone(),
                objective: runtime.objective.clone(),
            },
            runtime.assignment_id.clone().map(ViewTarget::Assignment),
        ),
        AttentionRuntimeRole::ProjectOrchestrator => (
            Subject::ProjectOrchestrator,
            runtime
                .project_id
                .clone()
                .map(ViewTarget::ProjectOrchestrator),
        ),
        AttentionRuntimeRole::YardOrchestrator => (
            Subject::YardOrchestrator,
            Some(ViewTarget::YardOrchestrator),
        ),
        AttentionRuntimeRole::CoordinationNode => (
            Subject::Workstream {
                name: runtime.node_name.clone(),
            },
            runtime.node_id.clone().map(ViewTarget::CoordinationNode),
        ),
    };
    AttentionEvent {
        kind,
        source: EventSource::Runtime {
            worker_id: runtime.worker_id.clone(),
            status: runtime.status,
        },
        project_id: runtime.project_id.clone(),
        project_name: runtime.project_name.clone(),
        subject,
        view_target,
        observed_at_unix_ms: since_unix_ms,
        ready_at: now,
    }
}

fn command_event(command: &AttentionCommand, since_unix_ms: u64, now: Instant) -> AttentionEvent {
    let view_target = match command.command_type.as_str() {
        "assignment_prompt" | "assignment_disposition" | "worker_handoff" => {
            command.assignment_id.clone().map(ViewTarget::Assignment)
        }
        "orchestrator_prompt" => command
            .project_id
            .clone()
            .map(ViewTarget::ProjectOrchestrator),
        "yard_orchestrator_prompt" | "yard_orchestrator_route" => {
            Some(ViewTarget::YardOrchestrator)
        }
        "coordination_node_prompt" | "coordination_node_route" => {
            command.node_id.clone().map(ViewTarget::CoordinationNode)
        }
        _ => None,
    };
    AttentionEvent {
        kind: match command.status {
            AttentionCommandStatus::Failed => AttentionKind::CommandFailed,
            AttentionCommandStatus::Ambiguous => AttentionKind::CommandAmbiguous,
        },
        source: EventSource::Command {
            command_id: command.command_id.clone(),
            node_id: command.node_id.clone(),
        },
        project_id: command.project_id.clone(),
        project_name: command.project_name.clone(),
        subject: Subject::Command {
            command_type: command.command_type.clone(),
            objective: command.objective.clone(),
            node_name: command.node_name.clone(),
        },
        view_target,
        observed_at_unix_ms: command.updated_at_unix_ms.min(since_unix_ms),
        ready_at: now,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use yard_domain::{ObservedStatus, RuntimeObservationState, RuntimeProcessState};
    use yard_store::{
        AttentionCommand, AttentionCommandStatus, AttentionRecords, AttentionRuntime,
        AttentionRuntimeRole,
    };

    use super::{AttentionKind, COMMAND_LOOKBACK_MS, Detector, DetectorTiming, Subject};
    use crate::slack::presence::ViewTarget;

    const SETTLE: Duration = Duration::from_secs(15);

    fn timing() -> DetectorTiming {
        DetectorTiming {
            blocked_settle: SETTLE,
            ready_settle: SETTLE,
            command_settle: SETTLE,
        }
    }

    fn worker(status: ObservedStatus, sequence: u64) -> AttentionRuntime {
        AttentionRuntime {
            role: AttentionRuntimeRole::Assignment,
            worker_id: "w-1".to_owned(),
            project_id: Some("p-1".to_owned()),
            project_name: Some("Checkout".to_owned()),
            assignment_id: Some("a-1".to_owned()),
            objective: Some("Ship it".to_owned()),
            node_id: None,
            node_name: None,
            profile_name: Some("Implementer".to_owned()),
            status,
            process_state: RuntimeProcessState::Running,
            observation_state: RuntimeObservationState::Observed,
            state_change_sequence: sequence,
            provider_session: Some("session-1".to_owned()),
            last_prompt_automatic: false,
            last_prompt_at_unix_ms: None,
        }
    }

    fn records(runtimes: Vec<AttentionRuntime>) -> AttentionRecords {
        AttentionRecords {
            runtimes,
            commands: Vec::new(),
            visible_project_ids: std::iter::once("p-1".to_owned()).collect(),
            visible_node_ids: std::collections::HashSet::new(),
        }
    }

    struct Clock {
        start: Instant,
    }

    impl Clock {
        fn at(&self, seconds: u64) -> Instant {
            self.start + Duration::from_secs(seconds)
        }
    }

    fn never(_: &ViewTarget) -> bool {
        false
    }

    /// Prime with `baseline`, then feed `steps` at the given seconds and
    /// collect every emitted kind.
    fn run(
        detector: &mut Detector,
        clock: &Clock,
        steps: &[(u64, AttentionRecords)],
    ) -> Vec<(u64, AttentionKind)> {
        let mut emitted = Vec::new();
        for (second, snapshot) in steps {
            let observation = detector.observe(snapshot, clock.at(*second), *second * 1000, never);
            emitted.extend(
                observation
                    .events
                    .into_iter()
                    .map(|event| (*second, event.kind)),
            );
        }
        emitted
    }

    #[test]
    fn blocked_is_announced_once_after_the_settle_window() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Working, 1)])),
                (2, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (10, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (17, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (40, records(vec![worker(ObservedStatus::Blocked, 2)])),
            ],
        );
        assert_eq!(emitted, vec![(17, AttentionKind::Blocked)]);
    }

    #[test]
    fn a_flap_inside_the_window_is_not_announced_and_restarts_the_window() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Working, 1)])),
                (2, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (6, records(vec![worker(ObservedStatus::Working, 3)])),
                (8, records(vec![worker(ObservedStatus::Blocked, 4)])),
                // A newer blocked episode (sequence 5) restarts the window.
                (12, records(vec![worker(ObservedStatus::Blocked, 5)])),
                (20, records(vec![worker(ObservedStatus::Blocked, 5)])),
                (27, records(vec![worker(ObservedStatus::Blocked, 5)])),
            ],
        );
        assert_eq!(emitted, vec![(27, AttentionKind::Blocked)]);
    }

    #[test]
    fn dedupe_holds_until_the_state_changes() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Working, 1)])),
                (1, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (16, records(vec![worker(ObservedStatus::Blocked, 2)])),
                // A presentation-only sequence bump while still blocked.
                (18, records(vec![worker(ObservedStatus::Blocked, 3)])),
                (40, records(vec![worker(ObservedStatus::Blocked, 3)])),
                (41, records(vec![worker(ObservedStatus::Working, 4)])),
                (42, records(vec![worker(ObservedStatus::Blocked, 5)])),
                (57, records(vec![worker(ObservedStatus::Blocked, 5)])),
            ],
        );
        assert_eq!(
            emitted,
            vec![(16, AttentionKind::Blocked), (57, AttentionKind::Blocked)]
        );
    }

    #[test]
    fn ready_for_review_needs_working_or_blocked_first_and_only_for_assignments() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let mut orchestrator = worker(ObservedStatus::Working, 1);
        orchestrator.worker_id = "w-orchestrator".to_owned();
        orchestrator.role = AttentionRuntimeRole::ProjectOrchestrator;
        let mut orchestrator_done = orchestrator.clone();
        orchestrator_done.status = ObservedStatus::Done;
        orchestrator_done.state_change_sequence = 2;
        let mut idle = worker(ObservedStatus::Idle, 1);
        idle.worker_id = "w-idle".to_owned();
        let mut idle_done = idle.clone();
        idle_done.status = ObservedStatus::Done;
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (
                    0,
                    records(vec![worker(ObservedStatus::Working, 1), orchestrator, idle]),
                ),
                (
                    2,
                    records(vec![
                        worker(ObservedStatus::Done, 2),
                        orchestrator_done.clone(),
                        idle_done.clone(),
                    ]),
                ),
                (
                    30,
                    records(vec![
                        worker(ObservedStatus::Done, 2),
                        orchestrator_done,
                        idle_done,
                    ]),
                ),
            ],
        );
        assert_eq!(emitted, vec![(30, AttentionKind::ReadyForReview)]);
    }

    #[test]
    fn silent_baseline_does_not_replay_existing_states() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (30, records(vec![worker(ObservedStatus::Blocked, 2)])),
                // Past a whole settle window after the first primed read.
                (60, records(vec![worker(ObservedStatus::Blocked, 2)])),
            ],
        );
        assert!(emitted.is_empty(), "{emitted:?}");
    }

    #[test]
    fn automatic_summary_turns_are_not_ready_for_review() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let prompted = |status: ObservedStatus, sequence: u64, automatic: bool, at: u64| {
            let mut runtime = worker(status, sequence);
            runtime.last_prompt_automatic = automatic;
            runtime.last_prompt_at_unix_ms = Some(at);
            records(vec![runtime])
        };
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Done, 1)])),
                // Yard's 15-minute summary request (created while quiet; the
                // store is read a moment after the automation saw `done`).
                (900, prompted(ObservedStatus::Working, 2, true, 899_000)),
                (960, prompted(ObservedStatus::Done, 3, true, 899_000)),
                (990, prompted(ObservedStatus::Done, 3, true, 899_000)),
                // The next summary round.
                (1_800, prompted(ObservedStatus::Working, 4, true, 1_799_000)),
                (1_860, prompted(ObservedStatus::Done, 5, true, 1_799_000)),
                (1_890, prompted(ObservedStatus::Done, 5, true, 1_799_000)),
                // A turn nobody prompted through Yard after that (the latest
                // prompt is still the old automatic one): announced.
                (2_000, prompted(ObservedStatus::Working, 6, true, 1_799_000)),
                (2_060, prompted(ObservedStatus::Done, 7, true, 1_799_000)),
                (2_090, prompted(ObservedStatus::Done, 7, true, 1_799_000)),
                // The owner prompts: announced.
                (
                    3_000,
                    prompted(ObservedStatus::Working, 8, false, 2_999_000),
                ),
                (3_060, prompted(ObservedStatus::Done, 9, false, 2_999_000)),
                (3_090, prompted(ObservedStatus::Done, 9, false, 2_999_000)),
                // An automatic turn that blocks still needs the owner.
                (
                    4_000,
                    prompted(ObservedStatus::Working, 10, true, 3_999_000),
                ),
                (
                    4_010,
                    prompted(ObservedStatus::Blocked, 11, true, 3_999_000),
                ),
                (
                    4_030,
                    prompted(ObservedStatus::Blocked, 11, true, 3_999_000),
                ),
            ],
        );
        assert_eq!(
            emitted,
            vec![
                (2_090, AttentionKind::ReadyForReview),
                (3_090, AttentionKind::ReadyForReview),
                (4_030, AttentionKind::Blocked),
            ]
        );
    }

    #[test]
    fn viewing_the_agent_suppresses_and_consumes_the_episode() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        detector.observe(
            &records(vec![worker(ObservedStatus::Working, 1)]),
            clock.at(0),
            0,
            never,
        );
        detector.observe(
            &records(vec![worker(ObservedStatus::Blocked, 2)]),
            clock.at(1),
            1000,
            never,
        );
        let viewing = |target: &ViewTarget| *target == ViewTarget::Assignment("a-1".to_owned());
        let observation = detector.observe(
            &records(vec![worker(ObservedStatus::Blocked, 2)]),
            clock.at(20),
            20_000,
            viewing,
        );
        assert!(observation.events.is_empty());
        assert_eq!(observation.suppressed, 1);
        let later = detector.observe(
            &records(vec![worker(ObservedStatus::Blocked, 2)]),
            clock.at(40),
            40_000,
            never,
        );
        assert!(later.events.is_empty());
    }

    #[test]
    fn exited_missing_and_vanished_workers_are_ignored() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let mut exited = worker(ObservedStatus::Blocked, 2);
        exited.process_state = RuntimeProcessState::Exited;
        let mut missing = worker(ObservedStatus::Blocked, 2);
        missing.observation_state = RuntimeObservationState::Missing;
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Working, 1)])),
                (1, records(vec![exited.clone()])),
                (20, records(vec![exited])),
                (21, records(vec![missing.clone()])),
                (40, records(vec![missing])),
                (41, records(vec![worker(ObservedStatus::Blocked, 3)])),
                // Archived, deleted or ended: the store stops returning it.
                (50, records(Vec::new())),
                (70, records(Vec::new())),
            ],
        );
        assert!(emitted.is_empty(), "{emitted:?}");
    }

    #[test]
    fn a_different_provider_session_restarts_the_window() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let mut replaced = worker(ObservedStatus::Blocked, 2);
        replaced.provider_session = Some("session-2".to_owned());
        let emitted = run(
            &mut detector,
            &clock,
            &[
                (0, records(vec![worker(ObservedStatus::Working, 1)])),
                (1, records(vec![worker(ObservedStatus::Blocked, 2)])),
                (10, records(vec![replaced.clone()])),
                (16, records(vec![replaced.clone()])),
                (25, records(vec![replaced])),
            ],
        );
        assert_eq!(emitted, vec![(25, AttentionKind::Blocked)]);
    }

    fn command(id: &str, status: AttentionCommandStatus, updated_at: u64) -> AttentionCommand {
        AttentionCommand {
            command_id: id.to_owned(),
            command_type: "assignment_prompt".to_owned(),
            status,
            updated_at_unix_ms: updated_at,
            project_id: Some("p-1".to_owned()),
            project_name: Some("Checkout".to_owned()),
            assignment_id: Some("a-1".to_owned()),
            objective: Some("Ship it".to_owned()),
            node_id: None,
            node_name: None,
        }
    }

    #[test]
    fn commands_settle_once_and_the_baseline_is_silent() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 100_000);
        assert_eq!(
            detector.commands_query_after(),
            100_000 - COMMAND_LOOKBACK_MS
        );
        let old = command("old", AttentionCommandStatus::Failed, 90_000);
        let new = command("new", AttentionCommandStatus::Ambiguous, 101_000);
        let with = |commands: Vec<AttentionCommand>| AttentionRecords {
            commands,
            ..records(Vec::new())
        };
        let first = detector.observe(&with(vec![old.clone()]), clock.at(0), 100_000, never);
        assert!(first.events.is_empty());
        detector.observe(
            &with(vec![old.clone(), new.clone()]),
            clock.at(1),
            101_000,
            never,
        );
        let settled = detector.observe(
            &with(vec![old.clone(), new.clone()]),
            clock.at(16),
            116_000,
            never,
        );
        assert_eq!(settled.events.len(), 1);
        assert_eq!(settled.events[0].kind, AttentionKind::CommandAmbiguous);
        assert!(matches!(settled.events[0].subject, Subject::Command { .. }));
        assert_eq!(
            detector.commands_query_after(),
            101_000 - COMMAND_LOOKBACK_MS
        );
        let again = detector.observe(&with(vec![old, new]), clock.at(40), 140_000, never);
        assert!(again.events.is_empty());
    }

    #[test]
    fn a_command_whose_project_disappears_before_settling_is_dropped() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let with = |commands: Vec<AttentionCommand>| AttentionRecords {
            commands,
            ..records(Vec::new())
        };
        detector.observe(&with(Vec::new()), clock.at(0), 0, never);
        detector.observe(
            &with(vec![command("c", AttentionCommandStatus::Failed, 1_000)]),
            clock.at(1),
            1_000,
            never,
        );
        let settled = detector.observe(&with(Vec::new()), clock.at(20), 20_000, never);
        assert!(settled.events.is_empty());
    }

    #[test]
    fn queued_commands_are_dropped_once_their_project_or_workstream_is_archived() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let mut node_command = command("n", AttentionCommandStatus::Failed, 1_000);
        node_command.command_type = "coordination_node_prompt".to_owned();
        node_command.project_id = None;
        node_command.project_name = None;
        node_command.node_id = Some("node-1".to_owned());
        let visible = |commands: Vec<AttentionCommand>| {
            let mut snapshot = AttentionRecords {
                commands,
                ..records(Vec::new())
            };
            snapshot.visible_node_ids.insert("node-1".to_owned());
            snapshot
        };
        detector.observe(&visible(Vec::new()), clock.at(0), 0, never);
        let both = vec![
            command("c", AttentionCommandStatus::Failed, 1_000),
            node_command,
        ];
        detector.observe(&visible(both.clone()), clock.at(1), 1_000, never);
        let events = detector
            .observe(&visible(both), clock.at(20), 20_000, never)
            .events;
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| detector.still_true(event)));
        // Later reads no longer list the commands (watermark moved on); the
        // project and the workstream are archived while Slack is down.
        let mut archived = records(Vec::new());
        archived.visible_project_ids.clear();
        detector.observe(&archived, clock.at(3_600), 3_600_000, never);
        assert!(events.iter().all(|event| !detector.still_true(event)));
    }

    #[test]
    fn queued_events_are_rechecked_against_the_latest_status() {
        let clock = Clock {
            start: Instant::now(),
        };
        let mut detector = Detector::new(timing(), 0);
        let emitted = detector
            .observe(
                &records(vec![worker(ObservedStatus::Working, 1)]),
                clock.at(0),
                0,
                never,
            )
            .events;
        assert!(emitted.is_empty());
        detector.observe(
            &records(vec![worker(ObservedStatus::Blocked, 2)]),
            clock.at(1),
            1,
            never,
        );
        let event = detector
            .observe(
                &records(vec![worker(ObservedStatus::Blocked, 2)]),
                clock.at(20),
                2,
                never,
            )
            .events
            .pop()
            .unwrap();
        assert!(detector.still_true(&event));
        detector.observe(
            &records(vec![worker(ObservedStatus::Working, 3)]),
            clock.at(21),
            3,
            never,
        );
        assert!(!detector.still_true(&event));
    }
}

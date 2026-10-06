//! Rate-limited provider session ids from pane foreground command lines.
//!
//! Herdr reports `agent_session: null` for an agent relaunched by hand, and
//! its Codex hook can attribute a session to the wrong pane (Codex runs hooks
//! in one shared `codex app-server` daemon). The pane's foreground
//! `codex resume <id>` / `claude --resume <id>` is ground truth. This tracker
//! decides which agent panes to inspect with Herdr `pane.process_info`,
//! caches what each inspection found, and applies the cached ids with the
//! precedence in [`yard_domain::apply_command_line_provider_sessions`].
//!
//! Inspection is not free (one socket connection per pane, and Herdr 0.9.1
//! answers a request after up to one 100 ms poll), so it is rate limited:
//!
//! * a Codex/Claude agent pane seen for the first time (or whose pane or
//!   provider changed) is inspected at once, uncapped, so the first snapshot
//!   Yard reconciles already carries the command-line id. The inspection is
//!   reserved at plan time: a concurrent snapshot does not repeat it, and
//!   waits (bounded) for its result instead;
//! * otherwise a pane is re-inspected when Herdr's report changed since the
//!   last inspection ([`REPORT_CHANGE_MIN_INTERVAL`]); after a failed
//!   inspection or while the command-line id disagrees with Herdr
//!   ([`DISAGREEMENT_RECHECK_INTERVAL`]); and otherwise (agreement, or a line
//!   without an id such as a fresh `codex --yolo`) only as a slow
//!   re-verification ([`AGREEMENT_REVERIFY_INTERVAL`]);
//! * at most [`MAX_RECHECKS_PER_INVENTORY`] re-inspections run per snapshot,
//!   oldest first; the rest keep their cached result until a later snapshot.
//!
//! Entries are dropped as soon as the agent leaves the snapshot, so a new
//! process in that pane is inspected afresh. An entry also belongs to one
//! Herdr pane instance (`pane_instance_id`, when Herdr reports it): a pane id
//! reused by a new pane instance restarts the cadence with a first-sight
//! inspection.
//!
//! The re-inspection cap can be shared by several snapshots through one
//! [`CommandLineRecheckBudget`] (the fleet view snapshots every session at
//! once and spends one budget across all of them).

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use yard_domain::{
    COMMAND_LINE_SESSION_PROVIDERS, CommandLineProviderSession, CommandLineSessionOverride,
    ObservedWorker, PaneForegroundJob, RuntimeInventory, apply_command_line_provider_sessions,
    provider_session_from_foreground,
};

pub(crate) const REPORT_CHANGE_MIN_INTERVAL: Duration = Duration::from_secs(2);
pub(crate) const DISAGREEMENT_RECHECK_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const AGREEMENT_REVERIFY_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const MAX_RECHECKS_PER_INVENTORY: usize = 4;

/// Re-inspections still allowed for one logical snapshot. Clones share the
/// same allowance, so concurrent per-session snapshots of one fleet view
/// together run at most [`MAX_RECHECKS_PER_INVENTORY`] re-inspections.
/// First-sight inspections never consume it.
#[derive(Debug, Clone)]
pub struct CommandLineRecheckBudget(Arc<AtomicUsize>);

impl Default for CommandLineRecheckBudget {
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(MAX_RECHECKS_PER_INVENTORY)))
    }
}

impl CommandLineRecheckBudget {
    /// Take up to `wanted` re-inspections; returns how many were granted.
    fn take(&self, wanted: usize) -> usize {
        let mut granted = 0;
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                granted = left.min(wanted);
                Some(left - granted)
            });
        granted
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EntryKey {
    session: String,
    terminal_id: String,
}

#[derive(Debug, Clone)]
struct Entry {
    pane_id: String,
    /// Herdr's pane instance at inspection time, when Herdr reports one.
    pane_instance_id: Option<String>,
    provider: String,
    /// Herdr's reported session value at the last inspection attempt.
    reported: Option<String>,
    /// The command-line id found by the last successful inspection.
    derived: Option<String>,
    inspected_at: Instant,
    failed: bool,
    /// Reserved by `plan` for a first-sight inspection that has not been
    /// recorded yet. Concurrent planners skip it and wait for the result.
    pending: bool,
    /// The last (reported, derived) pair that was logged as an override.
    logged: Option<(Option<String>, String)>,
}

impl Entry {
    fn matches(&self, worker: &ObservedWorker) -> bool {
        self.pane_id == worker.pane_id
            && self.pane_instance_id == worker.pane_instance_id
            && worker.provider.as_deref() == Some(self.provider.as_str())
    }

    fn recheck_interval(&self, reported: Option<&str>) -> Duration {
        if self.failed || self.pending {
            DISAGREEMENT_RECHECK_INTERVAL
        } else if self.reported.as_deref() != reported {
            REPORT_CHANGE_MIN_INTERVAL
        } else if self
            .derived
            .as_deref()
            .is_some_and(|derived| Some(derived) != reported)
        {
            DISAGREEMENT_RECHECK_INTERVAL
        } else {
            // Agreement, or a line without an id (a fresh `codex --yolo`)
            // while Herdr's report is unchanged: argv cannot change, so only
            // the slow safety net applies.
            AGREEMENT_REVERIFY_INTERVAL
        }
    }
}

/// Outcome of [`CommandLineSessions::apply`].
#[derive(Debug, Default)]
pub(crate) struct AppliedCommandLineSessions {
    pub overrides: Vec<CommandLineSessionOverride>,
    /// Overrides logged for the first time by this call.
    pub newly_logged: usize,
}

#[derive(Debug, Default)]
pub(crate) struct CommandLineSessions {
    entries: HashMap<EntryKey, Entry>,
}

fn inspectable(worker: &ObservedWorker) -> Option<&str> {
    worker
        .provider
        .as_deref()
        .filter(|provider| COMMAND_LINE_SESSION_PROVIDERS.contains(provider))
}

fn reported_value(worker: &ObservedWorker) -> Option<&str> {
    worker
        .provider_session
        .as_ref()
        .map(|session| session.value.as_str())
}

fn key(inventory: &RuntimeInventory, worker: &ObservedWorker) -> EntryKey {
    EntryKey {
        session: inventory.session.clone(),
        terminal_id: worker.terminal_id.clone(),
    }
}

impl CommandLineSessions {
    /// Choose the panes of `inventory` to inspect now, and forget agents
    /// that left it. Re-inspections are reserved (their timestamp moves to
    /// `now`) so concurrent snapshots do not inspect the same pane twice.
    #[cfg(test)]
    pub(crate) fn plan(&mut self, inventory: &RuntimeInventory, now: Instant) -> Vec<String> {
        self.plan_with_budget(inventory, now, &CommandLineRecheckBudget::default())
    }

    /// [`Self::plan`], spending re-inspections from a `budget` that other
    /// snapshots may share.
    pub(crate) fn plan_with_budget(
        &mut self,
        inventory: &RuntimeInventory,
        now: Instant,
        budget: &CommandLineRecheckBudget,
    ) -> Vec<String> {
        let live: HashSet<(&str, &str, &str, Option<&str>)> = inventory
            .workers
            .iter()
            .filter_map(|worker| {
                inspectable(worker).map(|provider| {
                    (
                        worker.terminal_id.as_str(),
                        worker.pane_id.as_str(),
                        provider,
                        worker.pane_instance_id.as_deref(),
                    )
                })
            })
            .collect();
        self.entries.retain(|key, entry| {
            key.session != inventory.session
                || live.contains(&(
                    key.terminal_id.as_str(),
                    entry.pane_id.as_str(),
                    entry.provider.as_str(),
                    entry.pane_instance_id.as_deref(),
                ))
        });

        let mut first_sight = Vec::new();
        let mut due = Vec::new();
        for worker in &inventory.workers {
            if inspectable(worker).is_none() {
                continue;
            }
            let key = key(inventory, worker);
            match self.entries.get(&key) {
                Some(entry) if entry.matches(worker) => {
                    let interval = entry.recheck_interval(reported_value(worker));
                    if now.saturating_duration_since(entry.inspected_at) >= interval {
                        due.push((entry.inspected_at, key, worker.pane_id.clone()));
                    }
                }
                _ => {
                    self.entries.insert(
                        key,
                        Entry {
                            pane_id: worker.pane_id.clone(),
                            pane_instance_id: worker.pane_instance_id.clone(),
                            provider: worker.provider.clone().unwrap_or_default(),
                            reported: reported_value(worker).map(str::to_owned),
                            derived: None,
                            inspected_at: now,
                            failed: false,
                            pending: true,
                            logged: None,
                        },
                    );
                    first_sight.push(worker.pane_id.clone());
                }
            }
        }
        due.sort_by_key(|(inspected_at, _, _)| *inspected_at);
        due.truncate(budget.take(due.len().min(MAX_RECHECKS_PER_INVENTORY)));
        for (_, key, pane_id) in due {
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.inspected_at = now;
            }
            first_sight.push(pane_id);
        }
        first_sight
    }

    /// Record inspection results for panes of `inventory`. A failed
    /// inspection keeps the previous command-line id (the pane still hosts
    /// the same agent) and is retried after
    /// [`DISAGREEMENT_RECHECK_INTERVAL`].
    pub(crate) fn record(
        &mut self,
        inventory: &RuntimeInventory,
        results: Vec<(String, Result<PaneForegroundJob, String>)>,
        now: Instant,
    ) {
        for (pane_id, result) in results {
            let Some((worker, provider)) = inventory.workers.iter().find_map(|worker| {
                (worker.pane_id == pane_id)
                    .then(|| inspectable(worker).map(|provider| (worker, provider)))
                    .flatten()
            }) else {
                continue;
            };
            let key = key(inventory, worker);
            let previous = self
                .entries
                .remove(&key)
                .filter(|entry| entry.matches(worker));
            let reported = reported_value(worker).map(str::to_owned);
            let entry = match result {
                Ok(job) => Entry {
                    pane_id,
                    pane_instance_id: worker.pane_instance_id.clone(),
                    provider: provider.to_owned(),
                    reported,
                    derived: provider_session_from_foreground(provider, &job),
                    inspected_at: now,
                    failed: false,
                    pending: false,
                    logged: previous.and_then(|entry| entry.logged),
                },
                Err(error) => {
                    tracing::debug!(
                        session = %inventory.session,
                        terminal_id = %worker.terminal_id,
                        pane_id = %pane_id,
                        %error,
                        "Herdr pane process inspection failed; keeping the previous command-line session"
                    );
                    match previous {
                        Some(mut entry) => {
                            entry.inspected_at = now;
                            entry.failed = true;
                            entry.pending = false;
                            entry
                        }
                        None => Entry {
                            pane_id,
                            pane_instance_id: worker.pane_instance_id.clone(),
                            provider: provider.to_owned(),
                            reported,
                            derived: None,
                            inspected_at: now,
                            failed: true,
                            pending: false,
                            logged: None,
                        },
                    }
                }
            };
            self.entries.insert(key, entry);
        }
    }

    /// Whether an agent of `inventory` still awaits a first-sight inspection
    /// that another caller reserved. Its result should be awaited (bounded)
    /// before `apply`, so no caller reconciles an uninspected misreport.
    pub(crate) fn has_pending(&self, inventory: &RuntimeInventory) -> bool {
        inventory.workers.iter().any(|worker| {
            self.entries
                .get(&key(inventory, worker))
                .is_some_and(|entry| entry.pending && entry.matches(worker))
        })
    }

    /// Apply the cached command-line ids to `inventory` (see the module
    /// precedence) and log each distinct override once.
    pub(crate) fn apply(&mut self, inventory: &mut RuntimeInventory) -> AppliedCommandLineSessions {
        let derived: Vec<CommandLineProviderSession> = inventory
            .workers
            .iter()
            .filter_map(|worker| {
                let entry = self.entries.get(&key(inventory, worker))?;
                if !entry.matches(worker) {
                    return None;
                }
                Some(CommandLineProviderSession {
                    terminal_id: worker.terminal_id.clone(),
                    pane_id: entry.pane_id.clone(),
                    provider: entry.provider.clone(),
                    value: entry.derived.clone()?,
                })
            })
            .collect();
        let overrides = apply_command_line_provider_sessions(inventory, &derived);
        let mut newly_logged = 0;
        for applied in &overrides {
            let Some(entry) = self.entries.get_mut(&EntryKey {
                session: inventory.session.clone(),
                terminal_id: applied.terminal_id.clone(),
            }) else {
                continue;
            };
            let pair = (
                applied
                    .reported
                    .as_ref()
                    .map(|session| session.value.clone()),
                applied.derived.value.clone(),
            );
            if entry.logged.as_ref() == Some(&pair) {
                continue;
            }
            newly_logged += 1;
            match &pair.0 {
                None => tracing::info!(
                    session = %inventory.session,
                    terminal_id = %applied.terminal_id,
                    pane_id = %applied.pane_id,
                    provider_session = %pair.1,
                    "Herdr reported no provider session; using the pane's foreground command line"
                ),
                Some(reported) => tracing::warn!(
                    session = %inventory.session,
                    terminal_id = %applied.terminal_id,
                    pane_id = %applied.pane_id,
                    herdr_provider_session = %reported,
                    provider_session = %pair.1,
                    "Herdr's provider session disagrees with the pane's foreground command line; using the command line"
                ),
            }
            entry.logged = Some(pair);
        }
        AppliedCommandLineSessions {
            overrides,
            newly_logged,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    use yard_domain::{
        FocusObservation, ForegroundProcess, ObservedStatus, ObservedWorker, PaneForegroundJob,
        ProviderSessionRef, RuntimeInventory,
    };

    use super::{
        AGREEMENT_REVERIFY_INTERVAL, CommandLineRecheckBudget, CommandLineSessions,
        DISAGREEMENT_RECHECK_INTERVAL, MAX_RECHECKS_PER_INVENTORY, REPORT_CHANGE_MIN_INTERVAL,
    };

    const Y: &str = "01a0b10f-4a75-7841-8e1d-aa6f12919c45";
    const X: &str = "01a0eb18-71ea-7c2b-9d3e-0123456789ab";

    fn session(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn agent(index: usize, provider: Option<&str>, reported: Option<&str>) -> ObservedWorker {
        ObservedWorker {
            runtime_id: format!("term-{index}"),
            pane_instance_id: None,
            terminal_id: format!("term-{index}"),
            workspace_id: "w1".to_owned(),
            tab_id: "w1:t1".to_owned(),
            pane_id: format!("w1:p{index}"),
            name: None,
            provider: provider.map(str::to_owned),
            display_provider: None,
            status: ObservedStatus::Idle,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 1,
            cwd: None,
            foreground_cwd: None,
            tokens: BTreeMap::new(),
            provider_session: reported.map(session),
            revision: 1,
        }
    }

    fn inventory(workers: Vec<ObservedWorker>) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.9.1".to_owned(),
            protocol: 22,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            workers,
            child_agents: Vec::new(),
        }
    }

    fn resumed(pane_id: &str, id: Option<&str>) -> (String, Result<PaneForegroundJob, String>) {
        let argv = match id {
            Some(id) => vec!["codex".to_owned(), "resume".to_owned(), id.to_owned()],
            None => vec!["codex".to_owned()],
        };
        (
            pane_id.to_owned(),
            Ok(PaneForegroundJob {
                pane_id: pane_id.to_owned(),
                process_group_id: Some(42),
                processes: vec![ForegroundProcess {
                    pid: 42,
                    name: "codex".to_owned(),
                    argv: Some(argv),
                }],
            }),
        )
    }

    /// Plan, inspect every planned pane as `codex resume <id>`, and record.
    fn inspect(
        tracker: &mut CommandLineSessions,
        inventory: &RuntimeInventory,
        now: Instant,
        id: Option<&str>,
    ) -> Vec<String> {
        let planned = tracker.plan(inventory, now);
        let results = planned.iter().map(|pane| resumed(pane, id)).collect();
        tracker.record(inventory, results, now);
        planned
    }

    #[test]
    fn first_sight_inspects_every_codex_and_claude_agent_uncapped() {
        let mut workers: Vec<_> = (1..=6)
            .map(|index| agent(index, Some("codex"), None))
            .collect();
        workers.push(agent(7, Some("claude"), Some(X)));
        workers.push(agent(8, Some("gemini"), None));
        workers.push(agent(9, None, None));
        let mut tracker = CommandLineSessions::default();

        let planned = tracker.plan(&inventory(workers), Instant::now());

        assert!(planned.len() > MAX_RECHECKS_PER_INVENTORY);
        assert_eq!(
            planned,
            (1..=7)
                .map(|index| format!("w1:p{index}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inspected_panes_are_not_reinspected_every_snapshot() {
        let start = Instant::now();
        let ms = Duration::from_millis;
        // One Herdr session each: a snapshot is a whole session, so agents it
        // does not list are forgotten.
        let mut missing = inventory(vec![agent(1, Some("codex"), None)]);
        missing.session = "missing".to_owned();
        let mut agreeing = inventory(vec![agent(1, Some("codex"), Some(Y))]);
        agreeing.session = "agreeing".to_owned();
        let mut disagreeing = inventory(vec![agent(1, Some("codex"), Some(X))]);
        disagreeing.session = "disagreeing".to_owned();
        let mut tracker = CommandLineSessions::default();
        for observed in [&missing, &agreeing, &disagreeing] {
            assert_eq!(inspect(&mut tracker, observed, start, Some(Y)).len(), 1);
        }

        for observed in [&missing, &agreeing, &disagreeing] {
            assert!(tracker.plan(observed, start + ms(500)).is_empty());
        }
        // No Herdr session, or Herdr disagrees with the command line.
        let recheck = start + DISAGREEMENT_RECHECK_INTERVAL;
        for observed in [&missing, &disagreeing] {
            assert!(
                tracker
                    .plan(
                        observed,
                        start + DISAGREEMENT_RECHECK_INTERVAL.saturating_sub(ms(1))
                    )
                    .is_empty()
            );
            assert_eq!(tracker.plan(observed, recheck).len(), 1);
        }
        // Herdr agrees with the command line: slow re-verification only.
        assert!(tracker.plan(&agreeing, recheck).is_empty());
        assert!(
            tracker
                .plan(
                    &agreeing,
                    start + AGREEMENT_REVERIFY_INTERVAL.saturating_sub(ms(1))
                )
                .is_empty()
        );
        assert_eq!(
            tracker
                .plan(&agreeing, start + AGREEMENT_REVERIFY_INTERVAL)
                .len(),
            1
        );
    }

    #[test]
    fn first_sight_is_reserved_so_concurrent_snapshots_do_not_repeat_it() {
        let start = Instant::now();
        let observed = inventory(
            (1..=10)
                .map(|index| agent(index, Some("codex"), None))
                .collect(),
        );
        let mut tracker = CommandLineSessions::default();

        assert_eq!(tracker.plan(&observed, start).len(), 10);
        assert!(tracker.has_pending(&observed));
        // Two more snapshots before the first one recorded its results.
        assert!(tracker.plan(&observed, start).is_empty());
        assert!(tracker.plan(&observed, start).is_empty());
        let mut early = observed.clone();
        assert!(tracker.apply(&mut early).overrides.is_empty());

        let results = (1..=10)
            .map(|index| resumed(&format!("w1:p{index}"), Some(Y)))
            .collect();
        tracker.record(&observed, results, start);
        assert!(!tracker.has_pending(&observed));
        let mut applied = observed.clone();
        assert_eq!(tracker.apply(&mut applied).overrides.len(), 10);

        // A reservation whose owner never recorded (cancelled) expires.
        let mut abandoned = CommandLineSessions::default();
        let single = inventory(vec![agent(1, Some("codex"), None)]);
        assert_eq!(abandoned.plan(&single, start).len(), 1);
        assert!(
            abandoned
                .plan(&single, start + REPORT_CHANGE_MIN_INTERVAL)
                .is_empty()
        );
        assert_eq!(
            abandoned
                .plan(&single, start + DISAGREEMENT_RECHECK_INTERVAL)
                .len(),
            1
        );
    }

    #[test]
    fn a_line_without_an_id_is_only_reverified_slowly() {
        // A fresh `codex --yolo` pane: Herdr reports nothing and the command
        // line carries no id. argv cannot change, so no 10 s polling.
        let start = Instant::now();
        let fresh = inventory(vec![agent(1, Some("codex"), None)]);
        let mut tracker = CommandLineSessions::default();
        assert_eq!(inspect(&mut tracker, &fresh, start, None).len(), 1);

        assert!(
            tracker
                .plan(&fresh, start + DISAGREEMENT_RECHECK_INTERVAL)
                .is_empty()
        );
        assert_eq!(
            tracker
                .plan(&fresh, start + AGREEMENT_REVERIFY_INTERVAL)
                .len(),
            1
        );
    }

    #[test]
    fn a_changed_herdr_report_is_reinspected_after_the_floor() {
        let start = Instant::now();
        let mut tracker = CommandLineSessions::default();
        inspect(
            &mut tracker,
            &inventory(vec![agent(1, Some("codex"), Some(Y))]),
            start,
            Some(Y),
        );
        let changed = inventory(vec![agent(1, Some("codex"), Some(X))]);

        assert!(
            tracker
                .plan(
                    &changed,
                    start + REPORT_CHANGE_MIN_INTERVAL.saturating_sub(Duration::from_millis(1))
                )
                .is_empty()
        );
        assert_eq!(
            tracker.plan(&changed, start + REPORT_CHANGE_MIN_INTERVAL),
            vec!["w1:p1".to_owned()]
        );
    }

    #[test]
    fn rechecks_are_capped_per_snapshot_oldest_first() {
        let start = Instant::now();
        let observed = inventory(
            (1..=6)
                .map(|index| agent(index, Some("codex"), None))
                .collect(),
        );
        let mut tracker = CommandLineSessions::default();
        // Inspect pane 6 first and pane 1 last so age order differs from
        // snapshot order.
        for index in (1..=6).rev() {
            let at = start + Duration::from_millis(u64::try_from(6 - index).unwrap());
            let pane = format!("w1:p{index}");
            tracker.record(&observed, vec![resumed(&pane, Some(Y))], at);
        }
        let due = start + DISAGREEMENT_RECHECK_INTERVAL + Duration::from_millis(10);

        let first = tracker.plan(&observed, due);
        let second = tracker.plan(&observed, due);
        let third = tracker.plan(&observed, due);

        assert_eq!(first.len(), MAX_RECHECKS_PER_INVENTORY);
        assert_eq!(first, vec!["w1:p6", "w1:p5", "w1:p4", "w1:p3"]);
        assert_eq!(second, vec!["w1:p2", "w1:p1"]);
        assert!(third.is_empty());
    }

    #[test]
    fn one_budget_caps_rechecks_across_the_sessions_of_a_fleet_view() {
        let start = Instant::now();
        let mut first_session = inventory(
            (1..=3)
                .map(|index| agent(index, Some("codex"), None))
                .collect(),
        );
        first_session.session = "first".to_owned();
        let mut second_session = first_session.clone();
        second_session.session = "second".to_owned();
        let mut tracker = CommandLineSessions::default();
        inspect(&mut tracker, &first_session, start, Some(Y));
        inspect(&mut tracker, &second_session, start, Some(Y));
        let due = start + DISAGREEMENT_RECHECK_INTERVAL + Duration::from_millis(10);
        let budget = CommandLineRecheckBudget::default();

        let first = tracker.plan_with_budget(&first_session, due, &budget.clone());
        let second = tracker.plan_with_budget(&second_session, due, &budget);
        let next_view =
            tracker.plan_with_budget(&second_session, due, &CommandLineRecheckBudget::default());

        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), MAX_RECHECKS_PER_INVENTORY - 3);
        assert_eq!(next_view.len(), 2);
    }

    #[test]
    fn a_reused_pane_id_with_a_new_pane_instance_is_inspected_afresh() {
        let start = Instant::now();
        let mut first = inventory(vec![agent(1, Some("codex"), None)]);
        first.workers[0].pane_instance_id = Some("instance-a".to_owned());
        let mut tracker = CommandLineSessions::default();
        inspect(&mut tracker, &first, start, Some(Y));
        let mut same = first.clone();
        assert!(tracker.plan(&same, start).is_empty());
        assert_eq!(tracker.apply(&mut same).overrides.len(), 1);

        let mut reused = first.clone();
        reused.workers[0].pane_instance_id = Some("instance-b".to_owned());

        assert!(tracker.apply(&mut reused).overrides.is_empty());
        assert_eq!(reused.workers[0].provider_session, None);
        assert_eq!(
            reused.workers[0].pane_instance_id.as_deref(),
            Some("instance-b")
        );
        assert_eq!(tracker.plan(&reused, start), vec!["w1:p1".to_owned()]);
    }

    #[test]
    fn an_agent_that_leaves_the_snapshot_is_inspected_afresh() {
        let start = Instant::now();
        let present = inventory(vec![agent(1, Some("codex"), None)]);
        let mut tracker = CommandLineSessions::default();
        inspect(&mut tracker, &present, start, Some(Y));
        assert!(tracker.plan(&present, start).is_empty());

        assert!(tracker.plan(&inventory(Vec::new()), start).is_empty());
        let mut back = present.clone();
        assert_eq!(tracker.plan(&back, start), vec!["w1:p1".to_owned()]);
        assert!(tracker.apply(&mut back).overrides.is_empty());
    }

    #[test]
    fn a_moved_pane_or_changed_provider_invalidates_the_cached_id() {
        let start = Instant::now();
        let mut tracker = CommandLineSessions::default();
        inspect(
            &mut tracker,
            &inventory(vec![agent(1, Some("codex"), None)]),
            start,
            Some(Y),
        );
        let mut moved = inventory(vec![agent(1, Some("codex"), None)]);
        moved.workers[0].pane_id = "w1:p9".to_owned();
        let mut claude = inventory(vec![agent(1, Some("claude"), None)]);

        assert!(tracker.apply(&mut moved).overrides.is_empty());
        assert_eq!(moved.workers[0].provider_session, None);
        assert!(tracker.apply(&mut claude).overrides.is_empty());
        assert_eq!(tracker.plan(&claude, start), vec!["w1:p1".to_owned()]);
    }

    #[test]
    fn apply_uses_the_command_line_and_logs_each_override_once() {
        let start = Instant::now();
        let mut tracker = CommandLineSessions::default();
        let relaunched = inventory(vec![agent(1, Some("codex"), None)]);
        inspect(&mut tracker, &relaunched, start, Some(Y));

        let mut first = relaunched.clone();
        let applied = tracker.apply(&mut first);
        assert_eq!(first.workers[0].provider_session, Some(session(Y)));
        assert_eq!(applied.overrides.len(), 1);
        assert_eq!(applied.newly_logged, 1);
        let mut again = relaunched.clone();
        let applied = tracker.apply(&mut again);
        assert_eq!(again.workers[0].provider_session, Some(session(Y)));
        assert_eq!(applied.newly_logged, 0);

        // Herdr now misattributes X to this pane; the cached command line wins
        // even before the pane is re-inspected, and the new pair logs once.
        let misreported = inventory(vec![agent(1, Some("codex"), Some(X))]);
        for expected_logs in [1, 0] {
            let mut observed = misreported.clone();
            let applied = tracker.apply(&mut observed);
            assert_eq!(observed.workers[0].provider_session, Some(session(Y)));
            assert_eq!(applied.overrides[0].reported, Some(session(X)));
            assert_eq!(applied.newly_logged, expected_logs);
        }

        // Herdr agrees with the command line: nothing to override.
        let mut agreeing = inventory(vec![agent(1, Some("codex"), Some(Y))]);
        assert!(tracker.apply(&mut agreeing).overrides.is_empty());
    }

    #[test]
    fn a_command_line_without_an_id_keeps_herdr_behavior() {
        let start = Instant::now();
        let mut tracker = CommandLineSessions::default();
        let fresh = inventory(vec![agent(1, Some("codex"), Some(X))]);
        inspect(&mut tracker, &fresh, start, None);

        let mut observed = fresh.clone();
        assert!(tracker.apply(&mut observed).overrides.is_empty());
        assert_eq!(observed.workers[0].provider_session, Some(session(X)));
        let mut neither = inventory(vec![agent(1, Some("codex"), None)]);
        assert!(tracker.apply(&mut neither).overrides.is_empty());
        assert_eq!(neither.workers[0].provider_session, None);
    }

    #[test]
    fn a_failed_inspection_keeps_the_previous_id_and_backs_off() {
        let start = Instant::now();
        let observed = inventory(vec![agent(1, Some("codex"), None)]);
        let mut tracker = CommandLineSessions::default();
        inspect(&mut tracker, &observed, start, Some(Y));
        let failed_at = start + DISAGREEMENT_RECHECK_INTERVAL;
        assert_eq!(tracker.plan(&observed, failed_at).len(), 1);
        tracker.record(
            &observed,
            vec![("w1:p1".to_owned(), Err("pane_not_found".to_owned()))],
            failed_at,
        );

        let mut applied = observed.clone();
        tracker.apply(&mut applied);
        assert_eq!(applied.workers[0].provider_session, Some(session(Y)));
        let mut changed = inventory(vec![agent(1, Some("codex"), Some(X))]);
        assert!(
            tracker
                .plan(&changed, failed_at + REPORT_CHANGE_MIN_INTERVAL)
                .is_empty()
        );
        assert_eq!(
            tracker
                .plan(&changed, failed_at + DISAGREEMENT_RECHECK_INTERVAL)
                .len(),
            1
        );

        // A pane whose first inspection failed has no id to apply.
        let mut unknown = CommandLineSessions::default();
        let other = inventory(vec![agent(2, Some("codex"), None)]);
        assert_eq!(unknown.plan(&other, start).len(), 1);
        unknown.record(
            &other,
            vec![("w1:p2".to_owned(), Err("timeout".to_owned()))],
            start,
        );
        assert!(
            unknown
                .plan(&other, start + REPORT_CHANGE_MIN_INTERVAL)
                .is_empty()
        );
        assert_eq!(tracker.apply(&mut changed).overrides.len(), 1);
    }
}

/// `HerdrInventorySource` against a fake Herdr (CLI discovery script plus a
/// Unix socket serving `session.snapshot` and `pane.process_info`).
#[cfg(test)]
mod source_tests {
    use std::{
        os::unix::fs::PermissionsExt,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };
    use yard_herdr::{HerdrAdapter, HerdrConfig};
    use yard_store::{SqliteProjectStore, YardStore};

    use crate::intervention_service::{InterventionService, InterventionServiceError};
    use crate::inventory_service::{
        CommandLineRecheckBudget, HerdrInventorySource, InventorySource,
    };

    const Y: &str = "01a0b10f-4a75-7841-8e1d-aa6f12919c45";
    const X: &str = "01a0eb18-71ea-7c2b-9d3e-0123456789ab";

    /// What the fake pane hosts.
    #[derive(Clone, Default)]
    enum Pane {
        /// A shell: no agent.
        #[default]
        Shell,
        /// A Codex agent whose Herdr `agent_session` is the given value.
        Codex(Option<String>),
    }

    #[derive(Default)]
    struct FakeHerdr {
        pane: Mutex<Pane>,
        argv: Mutex<Vec<String>>,
        process_info_requests: AtomicUsize,
        /// Milliseconds `pane.process_info` waits before answering.
        process_info_delay_ms: AtomicUsize,
    }

    impl FakeHerdr {
        fn set(&self, pane: Pane, argv: &str) {
            *self.pane.lock().unwrap() = pane;
            *self.argv.lock().unwrap() = argv.split_whitespace().map(str::to_owned).collect();
        }

        fn snapshot(&self) -> Value {
            let agent = match self.pane.lock().unwrap().clone() {
                Pane::Shell => None,
                Pane::Codex(report) => Some(report),
            };
            let session = |report: &Option<String>| {
                report.as_ref().map_or(Value::Null, |value| {
                    json!({"source":"herdr:codex","agent":"codex","kind":"id","value":value})
                })
            };
            let pane_agent = agent.as_ref().map(|_| "codex");
            let pane_session = agent.as_ref().map_or(Value::Null, session);
            let agents: Vec<Value> = agent
                .iter()
                .map(|report| {
                    json!({
                        "terminal_id": "term-b", "agent": "codex", "agent_status": "idle",
                        "agent_session": session(report), "workspace_id": "w1",
                        "tab_id": "w1:t1", "pane_id": "w1:pB", "focused": true,
                        "interactive_ready": true, "revision": 1
                    })
                })
                .collect();
            json!({
                "type": "session_snapshot",
                "snapshot": {
                    "version": "0.9.1", "protocol": 22,
                    "workspaces": [{
                        "workspace_id": "w1", "number": 1, "label": "project", "focused": true,
                        "pane_count": 1, "tab_count": 1, "active_tab_id": "w1:t1",
                        "agent_status": "idle"
                    }],
                    "tabs": [{
                        "tab_id": "w1:t1", "workspace_id": "w1", "number": 1, "label": "tab",
                        "focused": true, "pane_count": 1, "agent_status": "idle"
                    }],
                    "panes": [{
                        "pane_id": "w1:pB", "terminal_id": "term-b", "workspace_id": "w1",
                        "tab_id": "w1:t1", "focused": true, "agent": pane_agent,
                        "agent_status": "idle", "agent_session": pane_session, "revision": 1
                    }],
                    "agents": agents
                }
            })
        }

        fn process_info(&self) -> Value {
            self.process_info_requests.fetch_add(1, Ordering::SeqCst);
            let argv = self.argv.lock().unwrap().clone();
            json!({
                "type": "pane_process_info",
                "process_info": {
                    "pane_id": "w1:pB", "shell_pid": 10, "foreground_process_group_id": 20,
                    "foreground_processes": [{
                        "pid": 20, "name": "codex", "argv": argv, "cmdline": argv.join(" ")
                    }]
                }
            })
        }
    }

    async fn serve(listener: UnixListener, herdr: Arc<FakeHerdr>) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let herdr = Arc::clone(&herdr);
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str() {
                    Some("session.snapshot") => herdr.snapshot(),
                    Some("pane.process_info") => {
                        assert_eq!(request["params"]["pane_id"], "w1:pB");
                        let delay = herdr.process_info_delay_ms.load(Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(delay as u64)).await;
                        herdr.process_info()
                    }
                    other => panic!("unexpected Herdr method {other:?}"),
                };
                let mut response =
                    serde_json::to_vec(&json!({"id": request["id"], "result": result})).unwrap();
                response.push(b'\n');
                writer.write_all(&response).await.unwrap();
            });
        }
    }

    fn fake_herdr_binary(dir: &Path, socket: &Path) -> std::path::PathBuf {
        let binary = dir.join("herdr");
        let sessions = json!({"sessions": [{
            "name": "default", "default": true, "running": true, "socket_path": socket,
            "session_dir": dir
        }]});
        std::fs::write(&binary, format!("#!/bin/sh\nprintf '%s\\n' '{sessions}'\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        binary
    }

    fn codex(report: Option<&str>) -> Pane {
        Pane::Codex(report.map(str::to_owned))
    }

    /// A `HerdrInventorySource` whose CLI and socket are the fake Herdr.
    fn fake_source(temp: &TempDir, herdr: &Arc<FakeHerdr>) -> Arc<HerdrInventorySource> {
        let socket = temp.path().join("herdr.sock");
        tokio::spawn(serve(
            UnixListener::bind(&socket).unwrap(),
            Arc::clone(herdr),
        ));
        Arc::new(HerdrInventorySource::new(HerdrAdapter::new(HerdrConfig {
            binary: fake_herdr_binary(temp.path(), &socket).into_os_string(),
            request_timeout: Duration::from_secs(5),
            ..HerdrConfig::default()
        })))
    }

    #[tokio::test]
    async fn herdr_source_uses_the_foreground_command_line_for_terminal_access() {
        let temp = TempDir::new().unwrap();
        let herdr = Arc::new(FakeHerdr::default());
        let source = fake_source(&temp, &herdr);
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let interventions = InterventionService::new(source.clone(), source.clone(), store.clone());
        let resume_y = format!("codex resume {Y}");
        let requests = || herdr.process_info_requests.load(Ordering::SeqCst);

        // Herdr launched the agent and knows its session: a worker is bound
        // to Y (mainline does not auto-adopt, so it is seeded).
        herdr.set(codex(Some(Y)), &resume_y);
        let inventory = source.inventory("default").await.unwrap();
        assert_eq!(requests(), 1, "a new agent pane is inspected once");
        crate::inventory_service::seed_inventory_workers(
            &temp.path().join("yard.sqlite3"),
            &inventory,
            &["term-b"],
        );
        store.reconcile_runtime_inventory(inventory).await.unwrap();
        let worker = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .remove(0)
            .worker;
        assert_eq!(
            worker
                .runtime
                .as_ref()
                .unwrap()
                .provider_session
                .as_ref()
                .unwrap()
                .value,
            Y
        );
        for _ in 0..3 {
            source.inventory("default").await.unwrap();
        }
        assert_eq!(
            requests(),
            1,
            "an agreeing pane is not re-inspected every snapshot"
        );

        // The user quits Codex and types `codex resume Y` in the same pane;
        // Herdr sees the agent again but reports no session.
        herdr.set(Pane::Shell, "-bash");
        source.inventory("default").await.unwrap();
        herdr.set(codex(None), &resume_y);
        let relaunched = source.inventory("default").await.unwrap();
        assert_eq!(requests(), 2, "the relaunched agent is a new pane occupant");
        assert_eq!(
            relaunched.workers[0]
                .provider_session
                .as_ref()
                .unwrap()
                .value,
            Y
        );
        let reconciled = store.reconcile_runtime_inventory(relaunched).await.unwrap();
        assert_eq!(reconciled.ambiguous_bindings, 0);
        assert_eq!(reconciled.adopted_workers, 0);
        interventions
            .validate_worker_runtime(&worker)
            .await
            .unwrap();

        // Herdr then attributes another pane's session X to this pane: the
        // command line still wins, without re-inspecting within the floor.
        herdr.set(codex(Some(X)), &resume_y);
        let misreported = source.inventory("default").await.unwrap();
        assert_eq!(
            misreported.workers[0]
                .provider_session
                .as_ref()
                .unwrap()
                .value,
            Y
        );
        assert_eq!(
            misreported.panes[0]
                .provider_session
                .as_ref()
                .unwrap()
                .value,
            Y
        );
        interventions
            .validate_worker_runtime(&worker)
            .await
            .unwrap();
        assert_eq!(requests(), 2);

        // A line without an id leaves Herdr's report in charge (unchanged
        // behavior): the stale binding is refused as before.
        herdr.set(Pane::Shell, "-bash");
        source.inventory("default").await.unwrap();
        herdr.set(codex(None), "codex");
        assert!(matches!(
            interventions.validate_worker_runtime(&worker).await,
            Err(InterventionServiceError::RuntimeBindingStale)
        ));
    }

    #[tokio::test]
    async fn fleet_and_descriptor_snapshots_carry_the_command_line_session() {
        let temp = TempDir::new().unwrap();
        let herdr = Arc::new(FakeHerdr::default());
        let source = fake_source(&temp, &herdr);
        // The shared Codex daemon misattributed X to this pane; argv says Y.
        herdr.set(codex(Some(X)), &format!("codex resume {Y}"));
        let (_, descriptors) = source.session_descriptors().await.unwrap();
        let [descriptor] = descriptors.as_slice() else {
            panic!("one fake Herdr session");
        };
        let value = |inventory: &yard_domain::RuntimeInventory| {
            inventory.workers[0]
                .provider_session
                .as_ref()
                .map(|session| session.value.clone())
        };

        let fleet = source
            .fleet_inventory_for_descriptor_with_budget(
                descriptor,
                &CommandLineRecheckBudget::default(),
            )
            .await
            .unwrap();
        let named = source.inventory_for_descriptor(descriptor).await.unwrap();

        assert_eq!(value(&fleet), Some(Y.to_owned()));
        assert_eq!(value(&named), Some(Y.to_owned()));
        assert_eq!(fleet.panes[0].provider_session.as_ref().unwrap().value, Y);
    }

    #[tokio::test]
    async fn an_orchestrator_replacement_codex_pane_verifies_through_the_command_line() {
        let temp = TempDir::new().unwrap();
        let herdr = Arc::new(FakeHerdr::default());
        let source = fake_source(&temp, &herdr);
        herdr.set(codex(Some(X)), &format!("codex resume {Y}"));
        let expected = |inventory: &yard_domain::RuntimeInventory| {
            let worker = &inventory.workers[0];
            yard_domain::WorkerRuntimeBinding {
                adapter: inventory.adapter.clone(),
                session: inventory.session.clone(),
                workspace_id: worker.workspace_id.clone(),
                terminal_id: worker.terminal_id.clone(),
                tab_id: Some(worker.tab_id.clone()),
                pane_id: worker.pane_id.clone(),
                provider_session: worker.provider_session.clone(),
                owns_tab: true,
                observation_state: yard_domain::RuntimeObservationState::Observed,
                process_state: yard_domain::RuntimeProcessState::Running,
                status: worker.status,
                state_change_sequence: worker.state_change_sequence,
                revision: worker.revision,
                version: 1,
                last_observed_at_unix_ms: inventory.observed_at_unix_ms,
            }
        };
        // The replacement is captured from one snapshot and verified against
        // a later one, both through the same enriched source.
        let captured = expected(&source.inventory("default").await.unwrap());
        assert_eq!(captured.provider_session.as_ref().unwrap().value, Y);
        let later = source.inventory("default").await.unwrap();

        let verified =
            crate::orchestrator_replacement_service::verified_replacement_runtime(captured, &later)
                .unwrap();

        assert_eq!(verified.provider_session.unwrap().value, Y);
    }

    #[tokio::test]
    async fn concurrent_snapshots_share_one_first_sight_inspection() {
        let temp = TempDir::new().unwrap();
        let herdr = Arc::new(FakeHerdr::default());
        let source = fake_source(&temp, &herdr);
        herdr.set(codex(None), &format!("codex resume {Y}"));
        herdr.process_info_delay_ms.store(300, Ordering::SeqCst);

        let snapshots: Vec<_> = (0..3)
            .map(|_| {
                let source = Arc::clone(&source);
                tokio::spawn(async move { source.inventory("default").await.unwrap() })
            })
            .collect();
        let mut values = Vec::new();
        for snapshot in snapshots {
            let inventory = snapshot.await.unwrap();
            values.push(
                inventory.workers[0]
                    .provider_session
                    .as_ref()
                    .map(|session| session.value.clone()),
            );
        }

        assert_eq!(herdr.process_info_requests.load(Ordering::SeqCst), 1);
        assert_eq!(values, vec![Some(Y.to_owned()); 3]);
    }
}

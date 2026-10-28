//! The work board: one generated, in-memory picture of every piece of work
//! (assignments, workers, orchestrators, loose Herdr agents) grouped by what
//! the user should do next, plus one flat to-do list.
//!
//! Built only on an explicit Generate. The deterministic board publishes
//! first; status reports then refine it. The board lives in a single
//! in-memory slot and is never written to disk.

pub(crate) mod board;
pub(crate) mod conversation;
pub(crate) mod model;
mod pass;
pub(crate) mod plain;
pub(crate) mod reply;
pub(crate) mod rules;
pub(crate) mod titles;
pub(crate) mod todos;
pub(crate) mod triage;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "model_tests.rs"]
mod model_tests;

#[cfg(test)]
#[path = "real_data.rs"]
mod real_data;

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, RwLock},
    time::{Duration, Instant},
};

use futures_util::{StreamExt, stream};
use serde::Serialize;
use yard_domain::{CoordinationNodeKind, SnapshotCollectionStatus, TerminalOutputFormat};
use yard_store::YardStore;

use crate::{intervention_service::InterventionService, inventory_service::InventorySource};

use board::{
    Built, ChildAgent, Inputs, LivePanes, LooseAgent, PaneInfo, Previous, SnapshotNext, Window,
};

/// Largest `next-actions.md` the board reads.
pub(crate) const MAX_NEXT_ACTIONS_BYTES: u64 = 64 * 1024;
/// Bound on every Herdr read the generator makes.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Concurrent Herdr reads.
pub(crate) const READ_CONCURRENCY: usize = 4;
/// Lines of orchestrator output scanned for a status report.
const REPORT_LINES: u32 = 120;
/// A new generation may start this long after the previous one at the
/// earliest.
pub(crate) const MIN_GENERATE_INTERVAL: Duration = Duration::from_secs(10);
const NEXT_ACTIONS_FILE: &str = "next-actions.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BoardState {
    Empty,
    Idle,
    Running,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Collecting,
    Reports,
    Tails,
    Model,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct Thresholds {
    pub fresh_ms: u64,
    pub review_window_ms: u64,
    pub cold_ms: u64,
    /// The relevance window (main board vs Older work).
    pub recent_ms: u64,
    pub waiting_ms: u64,
    pub suggest_archive_ms: u64,
}

const fn thresholds(window: Window) -> Thresholds {
    Thresholds {
        fresh_ms: rules::FRESH_MS,
        review_window_ms: rules::REVIEW_WINDOW_MS,
        cold_ms: rules::COLD_MS,
        recent_ms: window.recent_ms,
        waiting_ms: window.waiting_ms,
        suggest_archive_ms: window.suggest_archive_ms,
    }
}

/// The published board, as `GET /api/v1/work-board` returns it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Board {
    pub revision: u64,
    pub generation_id: Option<String>,
    pub state: BoardState,
    pub stage: Option<Stage>,
    pub generated_at_unix_ms: Option<u64>,
    pub fingerprint: Option<String>,
    pub mode: &'static str,
    pub model_available: bool,
    pub thresholds: Thresholds,
    /// Non-fatal read problems (counts only, no detail).
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub timings: Timings,
    pub model: Option<ModelSummary>,
    /// Worst-case USD of a model generate right now (`None` when off).
    pub model_estimate_usd: Option<f64>,
    #[serde(flatten)]
    pub built: Option<Built>,
}

/// What the model pass of the last generation did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct ModelSummary {
    pub enabled: bool,
    pub model_id: String,
    pub calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
    pub error: Option<String>,
    /// Nothing changed since the cached results: no call was made.
    pub skipped: bool,
    pub cached_items: usize,
    pub sent_items: usize,
    pub batches_done: usize,
    pub batches_total: usize,
    /// Ambiguous items left out because the cost caps fit only a prefix.
    pub skipped_items: usize,
}

/// How long each stage of the last generation took.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct Timings {
    /// Start to the first published (deterministic) board.
    pub deterministic_ms: Option<u64>,
    pub herdr_ms: Option<u64>,
    pub model_ms: Option<u64>,
    pub total_ms: Option<u64>,
}

fn elapsed_ms(clock: Instant) -> u64 {
    u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartError {
    Running,
    TooSoon,
    ModelDisabled,
    /// The pre-flight estimate is over a cap (message says which).
    CostCap(String),
}

/// Board configuration. The model pass is off unless configured.
#[derive(Clone, Default)]
pub(crate) struct WorkBoardSettings {
    pub model: Option<model::ModelConfig>,
    /// The relevance window (`YARD_BOARD_RECENT_DAYS`, default 3;
    /// `YARD_BOARD_WAITING_DAYS`, 7; `YARD_BOARD_ARCHIVE_DAYS`, 14).
    pub window: Window,
    /// Where provider conversation files are read from (`None`: not read).
    pub conversations: Option<conversation::ConversationRoots>,
    /// Spend already counted against today's cap when the service starts
    /// (always 0 outside tests: the budget is in memory).
    pub spent_today_usd: f64,
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_SETTINGS: std::cell::RefCell<Option<WorkBoardSettings>> =
        const { std::cell::RefCell::new(None) };
}

/// The whole model stage stops after this, keeping what arrived.
pub(crate) const MODEL_DEADLINE: Duration = Duration::from_secs(45);
/// Per-call timeout of the `aws` CLI.
const MODEL_CALL_TIMEOUT: Duration = Duration::from_secs(40);

impl WorkBoardSettings {
    /// The test routers' settings: off, unless a test set
    /// [`TEST_SETTINGS`] on its (current-thread) runtime first.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        TEST_SETTINGS
            .with(|settings| settings.borrow_mut().take())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn with_model(
        model_id: &str,
        runner: Arc<dyn crate::model::ConverseRunner>,
    ) -> Self {
        Self {
            model: Some(model::ModelConfig {
                model_id: model_id.to_owned(),
                runner,
            }),
            spent_today_usd: 0.0,
            ..Self::default()
        }
    }

    /// The relevance window from `YARD_BOARD_RECENT_DAYS`,
    /// `YARD_BOARD_WAITING_DAYS` and `YARD_BOARD_ARCHIVE_DAYS` (whole days,
    /// 1-365; anything else keeps the default).
    pub(crate) fn window_from(var: impl Fn(&str) -> Option<String>) -> Window {
        let days = |name: &str, default: u64| {
            var(name)
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|days| (1..=365).contains(days))
                .map_or(default, |days| days * board::DAY_MS)
        };
        let fallback = Window::default();
        Window {
            recent_ms: days("YARD_BOARD_RECENT_DAYS", fallback.recent_ms),
            waiting_ms: days("YARD_BOARD_WAITING_DAYS", fallback.waiting_ms),
            suggest_archive_ms: days("YARD_BOARD_ARCHIVE_DAYS", fallback.suggest_archive_ms),
        }
    }

    /// `YARD_BOARD_MODEL` (a Bedrock model id) turns the model pass on; it
    /// also needs `YARD_BOARD_AWS_PROFILE` and `YARD_BOARD_AWS_ACCOUNT_ID`
    /// (the account guard). `YARD_BOARD_AWS_REGION` defaults to
    /// `us-west-2`. Anything missing leaves the pass off.
    pub(crate) fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let base = Self {
            window: Self::window_from(|name| var(name)),
            conversations: Some(conversation::ConversationRoots::from_env()),
            ..Self::default()
        };
        let Some(model_id) = var("YARD_BOARD_MODEL") else {
            return base;
        };
        let (Some(profile), Some(account_id)) = (
            var("YARD_BOARD_AWS_PROFILE"),
            var("YARD_BOARD_AWS_ACCOUNT_ID"),
        ) else {
            tracing::warn!(
                "YARD_BOARD_MODEL is set but YARD_BOARD_AWS_PROFILE or YARD_BOARD_AWS_ACCOUNT_ID is missing; the work board model pass stays off"
            );
            return base;
        };
        if account_id.len() != 12 || !account_id.bytes().all(|byte| byte.is_ascii_digit()) {
            tracing::warn!(
                "YARD_BOARD_AWS_ACCOUNT_ID is not a 12-digit account id; the work board model pass stays off"
            );
            return base;
        }
        let region = var("YARD_BOARD_AWS_REGION").unwrap_or_else(|| "us-west-2".to_owned());
        let runner = crate::model::AwsConverse::new(
            PathBuf::from("aws"),
            profile,
            region,
            account_id,
            std::env::temp_dir(),
            MODEL_CALL_TIMEOUT,
        );
        Self {
            model: Some(model::ModelConfig {
                model_id,
                runner: Arc::new(runner),
            }),
            ..base
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorkBoardService {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<dyn YardStore>,
    source: Arc<dyn InventorySource>,
    interventions: InterventionService,
    knowledge_root: PathBuf,
    settings: WorkBoardSettings,
    board: RwLock<Board>,
    run: Mutex<RunState>,
    budget: model::Budget,
    lru: Mutex<model::Lru>,
    triage: triage::TriageStore,
    window: Window,
}

#[derive(Default)]
struct RunState {
    running: bool,
    last_started: Option<Instant>,
    previous: Option<Previous>,
    /// The inputs of the newest published board, so a triage decision can
    /// re-place cards without a new generation.
    inputs: Option<Inputs>,
}

/// Why a triage decision was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecideError {
    UnknownItem,
    Write(String),
}

struct RunningGuard(Arc<Inner>, String);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        let mut board = self
            .0
            .board
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if board.state == BoardState::Running && board.generation_id.as_deref() == Some(&self.1) {
            board.state = BoardState::Failed;
            board.stage = None;
            board.error = Some("The generation stopped unexpectedly".to_owned());
            board.revision += 1;
        }
        drop(board);
        lock(&self.0.run).running = false;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

impl WorkBoardService {
    pub(crate) fn new(
        store: Arc<dyn YardStore>,
        source: Arc<dyn InventorySource>,
        interventions: InterventionService,
        knowledge_root: PathBuf,
        settings: WorkBoardSettings,
    ) -> Self {
        let model_available = settings.model.is_some();
        let window = settings.window;
        let triage =
            triage::TriageStore::load(Some(knowledge_root.with_file_name(triage::TRIAGE_FILE)));
        let budget = model::Budget::default();
        budget.add(settings.spent_today_usd);
        Self {
            inner: Arc::new(Inner {
                store,
                source,
                interventions,
                knowledge_root,
                settings,
                board: RwLock::new(Board {
                    revision: 0,
                    generation_id: None,
                    state: BoardState::Empty,
                    stage: None,
                    generated_at_unix_ms: None,
                    fingerprint: None,
                    mode: "deterministic",
                    model_available,
                    thresholds: thresholds(window),
                    warnings: Vec::new(),
                    error: None,
                    timings: Timings::default(),
                    model: None,
                    model_estimate_usd: None,
                    built: None,
                }),
                run: Mutex::new(RunState::default()),
                budget,
                lru: Mutex::new(model::Lru::default()),
                triage,
                window,
            }),
        }
    }

    /// Record a Keep / Skip / Clear decision on a card and re-place the
    /// cards of the current board (no new generation, no Herdr read).
    pub(crate) fn decide(&self, item_id: &str, choice: triage::Choice) -> Result<u64, DecideError> {
        let known = self
            .inner
            .board
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .built
            .as_ref()
            .is_some_and(|built| built.items.iter().any(|item| item.id == item_id));
        if !known && choice != triage::Choice::Clear {
            return Err(DecideError::UnknownItem);
        }
        let result = self.inner.triage.decide(item_id, choice, now_ms());
        let decisions = self.inner.triage.snapshot();
        let inputs = {
            let mut run = lock(&self.inner.run);
            run.inputs.as_mut().map(|inputs| {
                inputs.triage.clone_from(&decisions);
                inputs.clone()
            })
        };
        if let Some(inputs) = inputs {
            self.publish(|board| {
                if let Some(built) = board.built.take() {
                    let mut rebuilt = board::finish(&inputs, built.items);
                    rebuilt.fingerprint = built.fingerprint;
                    board.built = Some(rebuilt);
                }
            });
        }
        result.map_err(|error| {
            DecideError::Write(crate::redact::clean_text(&error.to_string(), 160))
        })?;
        Ok(self.revision())
    }

    pub(crate) fn board(&self) -> Board {
        let mut board = self
            .inner
            .board
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        board.model_estimate_usd = self.model_estimate_usd();
        board
    }

    pub(crate) fn revision(&self) -> u64 {
        self.inner
            .board
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }

    /// Cards on the cached board a model pass would write for.
    fn cached_ambiguous(&self) -> usize {
        self.inner
            .board
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .built
            .as_ref()
            .map_or(model::MAX_MODEL_ITEMS, |built| {
                built
                    .items
                    .iter()
                    .filter(|item| pass::model_candidate(item))
                    .count()
                    .min(model::MAX_MODEL_ITEMS)
            })
    }

    /// The worst-case cost of a model generate now (after the pass trims to
    /// the per-generate cap), if the pass is on.
    pub(crate) fn model_estimate_usd(&self) -> Option<f64> {
        let config = self.inner.settings.model.as_ref()?;
        Some(model::worst_case_within_cap(
            &config.model_id,
            self.cached_ambiguous(),
        ))
    }

    /// Start one generation in the background (single-flight, rate limited).
    pub(crate) fn start(
        &self,
        use_model: bool,
        project_ids: Option<Vec<String>>,
    ) -> Result<String, StartError> {
        if use_model {
            // The pass trims to the stalest items that fit the caps, so POST
            // refuses only when not even one worst-case item would fit.
            let Some(config) = self.inner.settings.model.as_ref() else {
                return Err(StartError::ModelDisabled);
            };
            let estimate = model::worst_case(&config.model_id, 1);
            if estimate > model::MAX_GENERATE_USD {
                return Err(StartError::CostCap(
                    model::CapError::PerGenerate(estimate).to_string(),
                ));
            }
            if self.inner.budget.spent_today() + estimate > model::DAILY_USD {
                return Err(StartError::CostCap(
                    model::CapError::Daily(estimate).to_string(),
                ));
            }
        }
        let previous = {
            let mut run = lock(&self.inner.run);
            if run.running {
                return Err(StartError::Running);
            }
            if run
                .last_started
                .is_some_and(|at| at.elapsed() < MIN_GENERATE_INTERVAL)
            {
                return Err(StartError::TooSoon);
            }
            run.running = true;
            run.last_started = Some(Instant::now());
            run.previous.clone()
        };
        let generation_id = uuid::Uuid::now_v7().to_string();
        self.publish(|board| {
            board.generation_id = Some(generation_id.clone());
            board.state = BoardState::Running;
            board.stage = Some(Stage::Collecting);
            board.error = None;
            board.warnings.clear();
        });
        let service = self.clone();
        let id = generation_id.clone();
        tokio::spawn(async move {
            // Cleared on drop, so a panic in a stage cannot wedge the board.
            let _running = RunningGuard(Arc::clone(&service.inner), id.clone());
            service.run(&id, project_ids, previous, use_model).await;
        });
        Ok(generation_id)
    }

    fn publish(&self, update: impl FnOnce(&mut Board)) {
        let mut board = self
            .inner
            .board
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut board);
        board.revision += 1;
    }

    async fn run(
        &self,
        generation_id: &str,
        project_ids: Option<Vec<String>>,
        previous: Option<Previous>,
        use_model: bool,
    ) {
        let clock = Instant::now();
        let started_ms = now_ms();
        let mut warnings = Vec::new();
        // Stage 1, the critical path: store and snapshot files only, no
        // Herdr round-trip and no model call.
        let mut inputs = match self.collect(project_ids.as_deref(), &mut warnings).await {
            Ok(inputs) => inputs,
            Err(error) => {
                let message = crate::redact::clean_text(&error, 200);
                self.publish(|board| {
                    board.state = BoardState::Failed;
                    board.stage = None;
                    board.error = Some(message);
                });
                return;
            }
        };
        inputs.now_ms = started_ms;
        let built = board::build(&inputs, previous.as_ref());
        lock(&self.inner.run).inputs = Some(inputs.clone());
        let deterministic_ms = elapsed_ms(clock);
        self.publish_built(
            generation_id,
            built,
            Stage::Reports,
            &warnings,
            BoardState::Running,
        );
        self.publish(|board| {
            board.timings = Timings {
                deterministic_ms: Some(deterministic_ms),
                ..Timings::default()
            };
        });

        // Stage 2: bounded Herdr reads (status reports, loose agents) in
        // parallel; the board above is already visible.
        let herdr_clock = Instant::now();
        let mut report_warnings = Vec::new();
        let mut loose_warnings = Vec::new();
        let (reports, agents) = tokio::join!(
            self.reports(&inputs, &mut report_warnings),
            self.herdr_agents(&mut loose_warnings)
        );
        warnings.extend(report_warnings);
        warnings.extend(loose_warnings);
        inputs.reports = reports;
        inputs.live_panes = agents.live;
        inputs.pane_info = agents.panes;
        inputs.children = agents.children;
        if project_ids.is_none() {
            inputs.loose_agents = agents.loose;
        }
        inputs.conversations = self.conversations(&inputs, &mut warnings).await;
        // A decision made while this generation ran is not lost.
        inputs.triage = self.inner.triage.snapshot();
        let built = board::build(&inputs, previous.as_ref());
        lock(&self.inner.run).inputs = Some(inputs.clone());
        lock(&self.inner.run).previous = Some(Previous {
            generated_at_ms: started_ms,
            marks: board::marks(&built.items),
        });
        let herdr_ms = elapsed_ms(herdr_clock);
        let (next_stage, state) = if use_model {
            (Stage::Tails, BoardState::Running)
        } else {
            (Stage::Done, BoardState::Idle)
        };
        self.publish_built(generation_id, built.clone(), next_stage, &warnings, state);
        self.publish(|board| {
            board.timings.herdr_ms = Some(herdr_ms);
            board.timings.total_ms = Some(elapsed_ms(clock));
            board.mode = "deterministic";
            board.model = None;
        });
        if use_model {
            self.model_pass(
                generation_id,
                &mut inputs,
                previous.as_ref(),
                built,
                &warnings,
                started_ms,
            )
            .await;
            self.publish(|board| board.timings.total_ms = Some(elapsed_ms(clock)));
        }
    }

    fn publish_built(
        &self,
        generation_id: &str,
        built: Built,
        next_stage: Stage,
        warnings: &[String],
        state: BoardState,
    ) {
        self.publish(|board| {
            if board.generation_id.as_deref() != Some(generation_id) {
                return;
            }
            board.fingerprint = Some(built.fingerprint.clone());
            board.generated_at_unix_ms = Some(now_ms());
            board.built = Some(built);
            board.stage = (state == BoardState::Running).then_some(next_stage);
            board.state = state;
            board.warnings = warnings.to_vec();
        });
    }

    /// Read every durable input. Herdr and snapshot reads that fail become
    /// warnings; a store failure fails the generation.
    async fn collect(
        &self,
        project_ids: Option<&[String]>,
        warnings: &mut Vec<String>,
    ) -> Result<Inputs, String> {
        let store = &self.inner.store;
        let wanted = |id: &str| project_ids.is_none_or(|ids| ids.iter().any(|wanted| wanted == id));
        let projects = store
            .list_projects()
            .await
            .map_err(|error| error.to_string())?
            .projects
            .into_iter()
            .filter(|project| wanted(&project.id))
            .collect::<Vec<_>>();
        let mut assignments = Vec::new();
        for project in &projects {
            match store.list_project_assignments(&project.id).await {
                Ok(list) => assignments.extend(list.assignments),
                Err(_) => warnings.push(format!(
                    "assignments of one project unreadable ({})",
                    project.id
                )),
            }
        }
        let workers = store
            .list_worker_candidates()
            .await
            .map_err(|error| error.to_string())?
            .workers
            .into_iter()
            .filter(|candidate| {
                project_ids.is_none() || candidate.project_id.as_deref().is_some_and(wanted)
            })
            .collect::<Vec<_>>();
        let now = now_ms();
        // Only an assignment's latest command counts: a retried prompt that
        // succeeded clears an earlier failure.
        let failed_commands = store
            .assignments_with_failed_last_command(now.saturating_sub(rules::REVIEW_WINDOW_MS))
            .await
            .unwrap_or_else(|_| {
                warnings.push("recent command failures unreadable".to_owned());
                HashSet::new()
            });
        let snapshots = self.snapshots(&projects, warnings).await;
        Ok(Inputs {
            now_ms: now,
            projects,
            assignments,
            workers,
            reports: HashMap::new(),
            failed_commands,
            snapshots,
            loose_agents: Vec::new(),
            tail_revisions: HashMap::new(),
            live_panes: None,
            pane_info: HashMap::new(),
            conversations: HashMap::new(),
            children: Vec::new(),
            triage: self.inner.triage.snapshot(),
            window: self.inner.window,
        })
    }

    /// The newest complete snapshot's `next-actions.md` per project.
    async fn snapshots(
        &self,
        projects: &[yard_domain::Project],
        warnings: &mut Vec<String>,
    ) -> HashMap<String, SnapshotNext> {
        let wanted = projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<HashSet<_>>();
        let Ok(nodes) = self.inner.store.list_coordination_nodes().await else {
            warnings.push("knowledge nodes unreadable".to_owned());
            return HashMap::new();
        };
        let mut newest: HashMap<String, (u64, String, String, String)> = HashMap::new();
        for node in nodes
            .nodes
            .iter()
            .filter(|node| node.kind == CoordinationNodeKind::KnowledgeStore)
        {
            let Ok(snapshots) = self.inner.store.list_coordination_snapshots(&node.id).await else {
                warnings.push("snapshots of one knowledge node unreadable".to_owned());
                continue;
            };
            for snapshot in snapshots.snapshots {
                for project in snapshot
                    .projects
                    .iter()
                    .filter(|project| {
                        project.collection_status == SnapshotCollectionStatus::Collected
                    })
                    .filter(|project| wanted.contains(project.project_id.as_str()))
                {
                    let entry = newest.entry(project.project_id.clone());
                    let candidate = (
                        snapshot.created_at_unix_ms,
                        node.id.clone(),
                        snapshot.id.clone(),
                        project.folder_path.clone(),
                    );
                    match entry {
                        std::collections::hash_map::Entry::Occupied(mut slot) => {
                            if slot.get().0 < candidate.0 {
                                slot.insert(candidate);
                            }
                        }
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(candidate);
                        }
                    }
                }
            }
        }
        let mut out = HashMap::new();
        for (project_id, (_, node_id, snapshot_id, folder)) in newest {
            match read_next_actions(&self.inner.knowledge_root, &node_id, Path::new(&folder)) {
                Ok(Some(text)) => {
                    out.insert(
                        project_id,
                        SnapshotNext {
                            node_id,
                            snapshot_id,
                            lines: todos::parse_next_actions(&text),
                        },
                    );
                }
                Ok(None) => {}
                Err(reason) => warnings.push(format!("one snapshot skipped: {reason}")),
            }
        }
        out
    }

    /// Every agent pane in Herdr's running sessions: which Yard workers are
    /// live (and their Herdr status), the names and conversation of each
    /// pane, subagents, and the candidates for loose agents (the board
    /// shows the panes no card claims). `live: None` when the sessions are
    /// unreadable.
    async fn herdr_agents(&self, warnings: &mut Vec<String>) -> HerdrAgents {
        let Ok(Ok(sessions)) =
            tokio::time::timeout(READ_TIMEOUT, self.inner.source.sessions()).await
        else {
            warnings.push("Herdr sessions unreadable".to_owned());
            return HerdrAgents::default();
        };
        let mut live = LivePanes {
            running_sessions: sessions
                .sessions
                .iter()
                .filter(|session| session.running)
                .map(|session| session.name.clone())
                .collect(),
            ..LivePanes::default()
        };
        let mut out = HerdrAgents::default();
        // Inventories are read in parallel (READ_CONCURRENCY at a time), each
        // bounded by READ_TIMEOUT.
        let source = Arc::clone(&self.inner.source);
        let inventories = futures_util::stream::iter(
            sessions
                .sessions
                .iter()
                .filter(|session| session.running)
                .map(|session| session.name.clone())
                .collect::<Vec<_>>(),
        )
        .map(|name| {
            let source = Arc::clone(&source);
            async move { tokio::time::timeout(READ_TIMEOUT, source.inventory(&name)).await }
        })
        .buffered(READ_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        for read in inventories {
            let Ok(Ok(inventory)) = read else {
                warnings.push("one Herdr session unreadable".to_owned());
                continue;
            };
            agents_of(&inventory, &mut live, &mut out);
        }
        out.loose.sort_by(|left, right| {
            (&left.session, &left.pane_id).cmp(&(&right.session, &right.pane_id))
        });
        out.live = Some(live);
        out
    }

    /// The conversations of every pane with a provider session, read from
    /// the provider files off the async runtime (bounded by `READ_TIMEOUT`).
    async fn conversations(
        &self,
        inputs: &Inputs,
        warnings: &mut Vec<String>,
    ) -> HashMap<String, conversation::Conversation> {
        let Some(roots) = self.inner.settings.conversations.clone() else {
            return HashMap::new();
        };
        let mut sessions = inputs
            .pane_info
            .values()
            .filter_map(|pane| Some((pane.provider.clone()?, pane.provider_session.clone()?)))
            .collect::<Vec<_>>();
        sessions.sort();
        sessions.dedup();
        let read = tokio::task::spawn_blocking(move || roots.read(&sessions));
        if let Ok(Ok(conversations)) = tokio::time::timeout(READ_TIMEOUT, read).await {
            conversations
        } else {
            warnings.push("agent conversations unreadable".to_owned());
            HashMap::new()
        }
    }

    /// The passive status report of each project orchestrator (no prompt is
    /// sent): a bounded read of its recent output.
    async fn reports(
        &self,
        inputs: &Inputs,
        warnings: &mut Vec<String>,
    ) -> HashMap<String, yard_domain::OrchestratorStatusReport> {
        let ids = inputs
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>();
        let interventions = self.inner.interventions.clone();
        let results = stream::iter(ids)
            .map(move |project_id| {
                let interventions = interventions.clone();
                async move {
                    let read = tokio::time::timeout(
                        READ_TIMEOUT,
                        interventions.read_orchestrator_output(
                            &project_id,
                            REPORT_LINES,
                            TerminalOutputFormat::Text,
                        ),
                    )
                    .await;
                    (project_id, read)
                }
            })
            .buffer_unordered(READ_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        let mut reports = HashMap::new();
        let mut unreadable = 0_usize;
        for (project_id, read) in results {
            match read {
                Ok(Ok(output)) => {
                    if let Some(report) = output.status_report {
                        reports.insert(project_id, report);
                    }
                }
                _ => unreadable += 1,
            }
        }
        if unreadable > 0 {
            warnings.push(format!("{unreadable} orchestrator output(s) unreadable"));
        }
        reports
    }
}

/// What stage 2 learns from Herdr's inventories.
#[derive(Default)]
pub(super) struct HerdrAgents {
    pub(super) live: Option<LivePanes>,
    pub(super) panes: HashMap<(String, String), PaneInfo>,
    pub(super) children: Vec<ChildAgent>,
    pub(super) loose: Vec<LooseAgent>,
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// One inventory's agent panes, pane names and subagents.
pub(super) fn agents_of(
    inventory: &yard_domain::RuntimeInventory,
    live: &mut LivePanes,
    out: &mut HerdrAgents,
) {
    let session = inventory.session.clone();
    live.read_sessions.insert(session.clone());
    let tabs = inventory
        .tabs
        .iter()
        .map(|tab| (tab.runtime_id.as_str(), tab.label.clone()))
        .collect::<HashMap<_, _>>();
    let workspaces = inventory
        .workspaces
        .iter()
        .map(|workspace| (workspace.runtime_id.as_str(), workspace.label.clone()))
        .collect::<HashMap<_, _>>();
    let pane_labels = inventory
        .panes
        .iter()
        .map(|pane| (pane.runtime_id.as_str(), pane.label.clone()))
        .collect::<HashMap<_, _>>();
    for worker in &inventory.workers {
        live.panes
            .insert((session.clone(), worker.pane_id.clone()), worker.status);
        live.panes
            .insert((session.clone(), worker.terminal_id.clone()), worker.status);
        let info = PaneInfo {
            pane_label: non_empty(
                pane_labels
                    .get(worker.pane_id.as_str())
                    .and_then(Option::as_ref),
            ),
            tab_label: non_empty(tabs.get(worker.tab_id.as_str())),
            workspace_label: non_empty(workspaces.get(worker.workspace_id.as_str())),
            agent_name: non_empty(worker.name.as_ref()),
            provider: non_empty(worker.provider.as_ref()).map(|value| value.to_ascii_lowercase()),
            provider_session: worker
                .provider_session
                .as_ref()
                .map(|session| session.value.clone()),
            cwd: worker.cwd.clone(),
        };
        out.panes
            .insert((session.clone(), worker.pane_id.clone()), info.clone());
        out.panes
            .insert((session.clone(), worker.terminal_id.clone()), info);
        out.loose.push(LooseAgent {
            session: session.clone(),
            pane_id: worker.pane_id.clone(),
            terminal_id: worker.terminal_id.clone(),
            name: worker.name.clone().unwrap_or_default(),
            status: worker.status,
            state_change_sequence: worker.state_change_sequence,
        });
    }
    for child in &inventory.child_agents {
        out.children.push(ChildAgent {
            id: child.runtime_id.clone(),
            parent_session: child.parent_provider_session.value.clone(),
            title: child
                .description
                .clone()
                .or_else(|| child.name.clone())
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Subagent".to_owned()),
            status: child.status,
            updated_at_ms: child.updated_at_unix_ms,
        });
    }
}

/// Read one snapshot's `next-actions.md` without following symlinks or
/// leaving `<knowledge root>/nodes/<node id>`. `Ok(None)` when absent.
pub(crate) fn read_next_actions(
    knowledge_root: &Path,
    node_id: &str,
    folder: &Path,
) -> Result<Option<String>, &'static str> {
    if node_id.is_empty() || node_id.contains(['/', '\\']) || node_id == "." || node_id == ".." {
        return Err("unsafe node id");
    }
    let Ok(root) = fs::canonicalize(knowledge_root.join("nodes").join(node_id)) else {
        return Ok(None);
    };
    let Ok(metadata) = fs::symlink_metadata(folder) else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("snapshot folder is not a plain directory");
    }
    let folder = fs::canonicalize(folder).map_err(|_| "snapshot folder unreadable")?;
    if !folder.starts_with(&root) {
        return Err("snapshot folder outside its knowledge node");
    }
    let file = folder.join(NEXT_ACTIONS_FILE);
    let Ok(metadata) = fs::symlink_metadata(&file) else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("next-actions.md is not a plain file");
    }
    if metadata.len() > MAX_NEXT_ACTIONS_BYTES {
        return Err("next-actions.md over 64 KiB");
    }
    let mut text = String::new();
    fs::File::open(&file)
        .and_then(|file| file.take(MAX_NEXT_ACTIONS_BYTES).read_to_string(&mut text))
        .map_err(|_| "next-actions.md unreadable")?;
    Ok(Some(text))
}

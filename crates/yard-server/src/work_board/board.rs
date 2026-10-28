//! The deterministic board: a pure function from collected inputs to items,
//! project headers, counts and to-dos. No I/O here.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use yard_domain::{
    Assignment, AssignmentLifecycle, ObservedStatus, OrchestratorStatusReport, Project,
    RuntimeObservationState, RuntimeProcessState, Worker, WorkerAvailability, WorkerCandidate,
    WorkerRuntimeBinding,
};

use super::conversation::Conversation;
use super::plain;
use super::reply::{self, Reply};
use super::rules::{
    self, Column, Facts, Heat, ItemKind, Placement, ReportFacts, Rule, Section, Verdict,
};
use super::titles::{self, TitleSources};
use super::todos::{self, SnapshotLine, Todo, TodoKind, TodoSource, Urgency};

const MAX_TITLE_CHARS: usize = 80;
const MAX_LINE_CHARS: usize = 160;
const MAX_NEXT_ACTIONS: usize = 5;

/// A Herdr agent that no Yard worker is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LooseAgent {
    pub session: String,
    pub pane_id: String,
    pub terminal_id: String,
    pub name: String,
    pub status: ObservedStatus,
    pub state_change_sequence: u64,
}

/// What Herdr shows about one agent pane besides its status: the names a
/// title can come from, its provider and conversation, where it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PaneInfo {
    /// The pane's terminal title, when Herdr reports one.
    pub pane_label: Option<String>,
    pub tab_label: Option<String>,
    pub workspace_label: Option<String>,
    /// The Herdr agent name (Yard's launch name, or the user's).
    pub agent_name: Option<String>,
    pub provider: Option<String>,
    /// The provider's session (conversation) id.
    pub provider_session: Option<String>,
    pub cwd: Option<String>,
}

/// A subagent a provider runs inside a card's conversation (Claude Task
/// agents, Codex child agents): a helper, never a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildAgent {
    pub id: String,
    pub parent_session: String,
    pub title: String,
    pub status: ObservedStatus,
    pub updated_at_ms: u64,
}

/// A user's triage decision on one card (kept in a small JSON file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    /// Keep: pinned to the main board. Otherwise "skip": reviewed, stays in
    /// Older work and is not offered for review again until it has new
    /// activity.
    pub keep: bool,
    pub at_ms: u64,
}

/// The relevance window: what the main board shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct Window {
    /// Activity this recent puts a card on the main board.
    pub recent_ms: u64,
    /// A card waiting on the user stays on the main board this long.
    pub waiting_ms: u64,
    /// Untouched this long, Older work suggests archiving it.
    pub suggest_archive_ms: u64,
}

pub(crate) const DAY_MS: u64 = 24 * 60 * 60 * 1000;

impl Default for Window {
    fn default() -> Self {
        Self {
            recent_ms: 3 * DAY_MS,
            waiting_ms: 7 * DAY_MS,
            suggest_archive_ms: 14 * DAY_MS,
        }
    }
}

/// Why a card is on the main board or in Older work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Relevance {
    /// Activity inside the recent window (or working now).
    Recent,
    /// Waiting on the user, inside the waiting window.
    Waiting,
    /// Kept on the board by the user.
    Pinned,
    /// Everything else: the collapsed "Older work" section.
    Older,
}

/// A subagent row under "Show helpers".
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct HelperRow {
    pub id: String,
    pub title: String,
    /// The card whose conversation runs it, when it is on the board.
    pub parent_id: Option<String>,
    pub parent_title: Option<String>,
    pub status: String,
    pub age_ms: u64,
}

/// The newest complete snapshot of one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotNext {
    pub node_id: String,
    pub snapshot_id: String,
    pub lines: Vec<SnapshotLine>,
}

impl SnapshotNext {
    pub(crate) fn has_open_checkbox(&self) -> bool {
        self.lines.iter().any(|line| line.checkbox)
    }
}

/// The agent panes Herdr reports right now, read once per generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LivePanes {
    /// Every running session Herdr listed.
    pub running_sessions: HashSet<String>,
    /// Running sessions whose inventory was read (others are trusted to
    /// Yard's own observation).
    pub read_sessions: HashSet<String>,
    /// Herdr's status of every agent pane, keyed by `(session, pane id)` and
    /// by `(session, terminal id)`.
    pub panes: HashMap<(String, String), ObservedStatus>,
}

impl LivePanes {
    /// Herdr's status of a bound runtime whose session inventory was read,
    /// `None` when Herdr does not list it. The terminal id is the identity;
    /// the (short, reusable) pane id also counts when Yard itself observes
    /// the runtime running or the binding has no terminal id.
    fn listed(&self, runtime: &WorkerRuntimeBinding, observed: bool) -> Option<ObservedStatus> {
        let key = |id: &str| (runtime.session.clone(), id.to_owned());
        if !runtime.terminal_id.is_empty()
            && let Some(status) = self.panes.get(&key(&runtime.terminal_id))
        {
            return Some(*status);
        }
        if observed || runtime.terminal_id.is_empty() {
            return self.panes.get(&key(&runtime.pane_id)).copied();
        }
        None
    }
}

/// Whether a worker is one of the agents the user has open in Herdr now,
/// and its Herdr status. When Herdr's inventory of the runtime's session
/// was read, Herdr decides (so an ambiguous, missing or stale Yard
/// observation of a listed pane still counts as live); a session Herdr does
/// not list as running is not live; otherwise (Herdr not read yet, or that
/// inventory unreadable) Yard's own observation decides.
fn herdr_live(inputs: &Inputs, worker: &Worker) -> Option<ObservedStatus> {
    let runtime = worker.runtime.as_ref()?;
    let observed = worker_live(worker);
    match inputs.live_panes.as_ref() {
        Some(panes) if panes.read_sessions.contains(&runtime.session) => {
            panes.listed(runtime, observed)
        }
        Some(panes) if !panes.running_sessions.contains(&runtime.session) => None,
        _ => observed.then_some(runtime.status),
    }
}

/// The `(session, id)` keys a card for this worker claims: its terminal id,
/// and its pane id when Yard observes the runtime (or there is no terminal
/// id). Used for one card per pane and to tell loose agents apart.
fn pane_keys(worker: &Worker) -> Vec<(String, String)> {
    let Some(runtime) = worker.runtime.as_ref() else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    if !runtime.terminal_id.is_empty() {
        keys.push((runtime.session.clone(), runtime.terminal_id.clone()));
    }
    if worker_live(worker) || runtime.terminal_id.is_empty() {
        keys.push((runtime.session.clone(), runtime.pane_id.clone()));
    }
    keys
}

/// Everything the deterministic board is built from.
#[derive(Debug, Clone, Default)]
pub(crate) struct Inputs {
    pub now_ms: u64,
    pub projects: Vec<Project>,
    pub assignments: Vec<Assignment>,
    pub workers: Vec<WorkerCandidate>,
    pub reports: HashMap<String, OrchestratorStatusReport>,
    /// Assignments whose latest command (in the review window) failed or
    /// was ambiguous; a later success clears it.
    pub failed_commands: HashSet<String>,
    pub snapshots: HashMap<String, SnapshotNext>,
    /// Every agent pane in the Herdr inventories read for this generation;
    /// the ones no card claims are shown as loose agents.
    pub loose_agents: Vec<LooseAgent>,
    /// Terminal-output revisions read for this generation, by item id.
    pub tail_revisions: HashMap<String, u64>,
    /// Herdr's agent panes now; `None` until read (or when unreadable), and
    /// then only Yard's own runtime observation decides what is live.
    pub live_panes: Option<LivePanes>,
    /// Pane names and conversation ids, keyed like `LivePanes::panes`.
    pub pane_info: HashMap<(String, String), PaneInfo>,
    /// Conversations read from the provider files, by session id.
    pub conversations: HashMap<String, Conversation>,
    /// Subagents Herdr reports.
    pub children: Vec<ChildAgent>,
    /// The user's Keep / Skip decisions, by item id.
    pub triage: HashMap<String, Decision>,
    pub window: Window,
}

/// What the previous generation remembered per item, for age correction.
#[derive(Debug, Clone, Default)]
pub(crate) struct Previous {
    pub generated_at_ms: u64,
    pub marks: HashMap<String, Mark>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mark {
    pub state_change_sequence: u64,
    pub tail_revision: Option<u64>,
    /// Loose agents only (Herdr gives them no timestamp): when the board
    /// last saw their state change, carried across generations.
    pub last_changed_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    OpenTerminal,
    OpenProject,
    MarkDone,
    Resume,
    EndSession,
    ArchiveProject,
    CleanUp,
    Hide,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AiNote {
    pub verdict: Verdict,
    pub confidence: f64,
    pub evidence: String,
    pub quote: String,
    pub next_actions: Vec<String>,
    /// The AI's 3-7 word title (used only when nothing better exists).
    pub title: String,
    /// The AI's three plain lines (empty when it gave none).
    pub about: String,
    pub status: String,
    pub your_move: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Item {
    pub id: String,
    pub kind: ItemKind,
    pub column: Column,
    pub section: Option<Section>,
    pub rule: Rule,
    pub chips: Vec<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub worker_id: Option<String>,
    pub assignment_id: Option<String>,
    pub title: String,
    /// Where the title came from (see `titles`).
    pub title_source: &'static str,
    pub objective: String,
    pub status_line: String,
    /// The three plain lines of the card face: what the work is, where it
    /// is at (the web adds "2 h ago" when the age is known), and what the
    /// user should do ("Nothing — it's working").
    pub about: String,
    pub status: String,
    pub your_move: String,
    /// Whether `age_ms` is a real age (otherwise ages are not shown).
    pub activity_known: bool,
    /// Main board (recent, waiting, pinned) or Older work.
    pub relevance: Relevance,
    /// Why this is a helper (hidden unless "Show helpers"), if it is one.
    pub helper: Option<&'static str>,
    pub pinned: bool,
    /// Skipped in a review and unchanged since.
    pub reviewed: bool,
    /// Untouched longer than the suggest-archive threshold.
    pub suggest_archive: bool,
    pub provider: Option<String>,
    /// What is happening now, in plain words (see `plain`).
    pub summary: String,
    /// What the user should do next, in plain words; empty when nothing.
    pub next_step: String,
    /// Plain labels of `chips` (the chips stay as stable codes).
    pub labels: Vec<String>,
    /// Technical detail for the card's Details disclosure (store reasons,
    /// availability and Herdr status codes).
    pub details: Vec<String>,
    pub next_actions: Vec<String>,
    pub age_ms: u64,
    pub age_source: &'static str,
    pub heat: Heat,
    pub ai: Option<AiNote>,
    pub actions: Vec<Action>,
    /// An orchestrator shown in its project header, not as a card.
    pub rollup: bool,
    /// Waiting on the model pass.
    pub pending: bool,
    pub dead_count: Option<usize>,
    #[serde(skip)]
    pub facts: Facts,
    /// The R0-R7 placement before the post-pass.
    #[serde(skip)]
    pub base: Placement,
    #[serde(skip)]
    pub mark: Mark,
    /// The deterministic "About" line (the AI's replaces it).
    #[serde(skip)]
    pub about_base: String,
    /// Last activity, Unix ms, when known.
    #[serde(skip)]
    pub activity_at: Option<u64>,
    /// The provider conversation id (subagents attach by it).
    #[serde(skip)]
    pub session_id: Option<String>,
    /// The conversation digest for the AI pass.
    #[serde(skip)]
    pub digest: String,
    /// The agent's latest message (unredacted opening; `plain` cuts and
    /// redacts what it shows of it).
    #[serde(skip)]
    pub last_agent: Option<String>,
    /// Where the chat runs (workspace or folder name), to match a project.
    #[serde(skip)]
    pub folder: Option<String>,
    /// Needs you: "answer" (a question, a blocker, a failure) or "review"
    /// (finished work to look at); `None` in other columns.
    pub need: Option<&'static str>,
    /// What the agent's latest message is (see `reply`).
    #[serde(skip)]
    pub reply: Option<Reply>,
    /// Titles of the subagents working under it now.
    #[serde(skip)]
    pub subagents: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Counts {
    pub doing: usize,
    pub needs_you: usize,
    pub stale_needs_work: usize,
    pub stale_likely_done: usize,
    pub close: usize,
    pub closed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ProjectHeader {
    pub id: String,
    pub name: String,
    pub status_line: String,
    /// The orchestrator's plain summary (what is happening in the project).
    pub summary: String,
    pub orchestrator_column: Column,
    pub counts: Counts,
}

/// Worker records left off the board because no live Herdr agent backs
/// them (ended, archived, dead or missing runtimes). Counts only; the
/// web links them to the existing cleanup dialog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Hidden {
    pub not_live: usize,
    /// Of those, `unavailable` records (runtime gone, not resumable).
    pub dead_runtimes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Built {
    pub items: Vec<Item>,
    /// Cards in Older work (not helpers).
    pub older: usize,
    /// Older-work cards a review would still offer (not skipped since).
    pub to_review: usize,
    /// Helper cards plus subagents.
    pub helpers: usize,
    pub helper_rows: Vec<HelperRow>,
    pub window: Window,
    pub projects: Vec<ProjectHeader>,
    pub counts: Counts,
    pub todos: Vec<Todo>,
    pub hidden: Hidden,
    pub fingerprint: String,
}

/// The `GET /workers` fingerprint the web compares to spot an outdated
/// board: highest runtime change sequence and highest worker update time.
pub(crate) fn fingerprint(workers: &[WorkerCandidate]) -> String {
    let sequence = workers
        .iter()
        .filter_map(|candidate| candidate.worker.runtime.as_ref())
        .map(|runtime| runtime.state_change_sequence)
        .max()
        .unwrap_or(0);
    let updated = workers
        .iter()
        .map(|candidate| candidate.worker.updated_at_unix_ms)
        .max()
        .unwrap_or(0);
    format!("{sequence}-{updated}")
}

fn worker_live(worker: &Worker) -> bool {
    worker.runtime.as_ref().is_some_and(|runtime| {
        runtime.observation_state == RuntimeObservationState::Observed
            && runtime.process_state != RuntimeProcessState::Exited
    })
}

fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    crate::redact::clean_text(line, max)
}

/// Whether a worker is one of the agents the user has open in Herdr now
/// (see [`herdr_live`]); an ended record never is.
pub(crate) fn on_board(
    inputs: &Inputs,
    worker: &Worker,
    availability: Option<WorkerAvailability>,
) -> bool {
    availability != Some(WorkerAvailability::Ended) && herdr_live(inputs, worker).is_some()
}

struct Ctx<'a> {
    inputs: &'a Inputs,
    previous: Option<&'a Previous>,
    candidates: HashMap<&'a str, &'a WorkerCandidate>,
    projects: HashMap<&'a str, &'a Project>,
    /// Profile and role names, lower-cased: never a title.
    profiles: HashSet<String>,
}

/// One agent's pane names and conversation, as far as they are known.
#[derive(Default)]
struct Talk<'a> {
    pane: Option<&'a PaneInfo>,
    session: Option<String>,
    conversation: Option<&'a Conversation>,
    provider: Option<String>,
}

impl<'a> Talk<'a> {
    fn title(&self) -> Option<&'a str> {
        self.conversation
            .and_then(|conversation| conversation.title.as_deref())
    }

    fn pane_label(&self) -> Option<&'a str> {
        self.pane.and_then(|pane| pane.pane_label.as_deref())
    }

    fn agent_name(&self) -> Option<&'a str> {
        self.pane.and_then(|pane| pane.agent_name.as_deref())
    }

    fn tab_label(&self) -> Option<&'a str> {
        self.pane.and_then(|pane| pane.tab_label.as_deref())
    }

    /// The workspace, else the working directory's last component.
    fn place(&self) -> Option<String> {
        let pane = self.pane?;
        pane.workspace_label
            .clone()
            .filter(|label| !label.trim().is_empty())
            .or_else(|| {
                pane.cwd.as_deref().and_then(|cwd| {
                    std::path::Path::new(cwd)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
            })
    }

    fn first_user(&self) -> Option<&'a str> {
        self.conversation
            .and_then(|conversation| conversation.first_user.as_deref())
    }

    fn last_user(&self) -> Option<&'a str> {
        self.conversation
            .and_then(|conversation| conversation.last_user.as_deref())
    }

    fn last_agent(&self) -> Option<String> {
        self.conversation
            .and_then(|conversation| conversation.last_agent.clone())
    }

    fn digest(&self) -> String {
        self.conversation
            .map(|conversation| conversation.digest.clone())
            .unwrap_or_default()
    }
}

/// Last activity and its source. With a known conversation: its newest
/// message (user or agent) or a newer `events` time (task updated, report
/// left); otherwise the newest of `events` and `records` (Yard record
/// times, which also move on bookkeeping). Working now is activity now.
fn activity(
    talk: &Talk<'_>,
    working: bool,
    now_ms: u64,
    events: &[(u64, &'static str)],
    records: &[(u64, &'static str)],
) -> Option<(u64, &'static str)> {
    if working {
        return Some((now_ms, "working now"));
    }
    let newest = |times: &mut dyn Iterator<Item = (u64, &'static str)>| {
        times.filter(|(at, _)| *at > 0).max_by_key(|(at, _)| *at)
    };
    match talk
        .conversation
        .and_then(|conversation| conversation.last_activity_ms)
    {
        Some(at) => newest(
            &mut std::iter::once((at.min(now_ms), "conversation")).chain(events.iter().copied()),
        ),
        None => newest(&mut events.iter().chain(records).copied()),
    }
}

impl<'a> Ctx<'a> {
    /// A runtime's pane names and conversation.
    fn talk(&self, runtime: Option<&WorkerRuntimeBinding>) -> Talk<'a> {
        let inputs: &'a Inputs = self.inputs;
        let Some(runtime) = runtime else {
            return Talk::default();
        };
        let pane = [&runtime.terminal_id, &runtime.pane_id]
            .into_iter()
            .filter(|id| !id.is_empty())
            .find_map(|id| inputs.pane_info.get(&(runtime.session.clone(), id.clone())));
        let session = pane
            .and_then(|pane| pane.provider_session.clone())
            .or_else(|| {
                runtime
                    .provider_session
                    .as_ref()
                    .map(|session| session.value.clone())
            });
        let provider = pane.and_then(|pane| pane.provider.clone()).or_else(|| {
            runtime
                .provider_session
                .as_ref()
                .map(|session| session.provider.clone())
        });
        Talk {
            pane,
            conversation: session.as_ref().and_then(|id| inputs.conversations.get(id)),
            session,
            provider,
        }
    }

    /// The card title; a lowercase agent or tab name takes the capitals
    /// the conversation itself uses ("orbit review" -> "ORBIT review").
    fn title(&self, sources: &TitleSources<'_>, talk: &Talk<'_>) -> (String, &'static str) {
        let (title, source) = titles::choose(sources, &self.profiles);
        if matches!(source, titles::FROM_AGENT | titles::FROM_TAB) {
            // Its own conversation, then the names the board already shows
            // (projects, conversation titles).
            let mut context = format!(
                "{} {}",
                talk.first_user().unwrap_or_default(),
                talk.digest()
            );
            for project in &self.inputs.projects {
                context.push(' ');
                context.push_str(&project.name);
            }
            for title in self
                .inputs
                .conversations
                .values()
                .filter_map(|conversation| conversation.title.as_deref())
            {
                context.push(' ');
                context.push_str(title);
            }
            return (titles::capitals_like(&title, &context), source);
        }
        (title, source)
    }
}

impl Ctx<'_> {
    /// The worker's Herdr status when it is live in Herdr now.
    fn live(&self, worker: &Worker) -> Option<ObservedStatus> {
        herdr_live(self.inputs, worker)
    }

    fn on_board(&self, worker: &Worker) -> bool {
        let availability = self
            .candidates
            .get(worker.id.as_str())
            .map(|candidate| candidate.availability);
        on_board(self.inputs, worker, availability)
    }

    /// Age and its source, corrected by a change since the last generation.
    fn age(
        &self,
        id: &str,
        mut last_activity: u64,
        mut source: &'static str,
        mark: Mark,
    ) -> (u64, &'static str) {
        if let Some(previous) = self.previous
            && let Some(old) = previous.marks.get(id)
        {
            let tail_moved = matches!(
                (old.tail_revision, mark.tail_revision),
                (Some(old), Some(new)) if old != new
            );
            if (tail_moved || old.state_change_sequence != mark.state_change_sequence)
                && previous.generated_at_ms > last_activity
            {
                last_activity = previous.generated_at_ms;
                source = if tail_moved {
                    "terminal output changed"
                } else {
                    "runtime status changed"
                };
            }
        }
        (self.inputs.now_ms.saturating_sub(last_activity), source)
    }

    fn snapshot_open(&self, project_id: Option<&str>) -> bool {
        project_id
            .and_then(|id| self.inputs.snapshots.get(id))
            .is_some_and(SnapshotNext::has_open_checkbox)
    }
}

/// Assignment precedence when one worker has several: open work first, then
/// the newest.
fn assignment_rank(assignment: &Assignment) -> (u8, u64) {
    use AssignmentLifecycle as L;
    let open = match assignment.lifecycle {
        L::Active | L::Allocating | L::HandingOff => 2,
        L::Failed
            if assignment.completion_receipt.is_none() && assignment.cancellation.is_none() =>
        {
            2
        }
        L::Completed => 1,
        _ => 0,
    };
    (open, assignment.updated_at_unix_ms)
}

#[allow(clippy::too_many_lines)]
fn assignment_item(ctx: &Ctx<'_>, assignment: &Assignment) -> Item {
    let candidate = ctx.candidates.get(assignment.worker.id.as_str()).copied();
    let worker = candidate.map_or(&assignment.worker, |candidate| &candidate.worker);
    let project = ctx.projects.get(assignment.project_id.as_str());
    let receipt = assignment.completion_receipt.as_ref();
    let id = format!("a:{}", assignment.id);
    let mark = Mark {
        state_change_sequence: worker
            .runtime
            .as_ref()
            .map_or(0, |runtime| runtime.state_change_sequence),
        tail_revision: ctx.inputs.tail_revisions.get(&id).copied(),
        last_changed_ms: None,
    };
    let status = ctx.live(worker);
    let live = status.is_some();
    let talk = ctx.talk(worker.runtime.as_ref());
    let (last, source) = activity(
        &talk,
        status == Some(ObservedStatus::Working),
        ctx.inputs.now_ms,
        &[
            (assignment.updated_at_unix_ms, "assignment updated"),
            (
                receipt.map_or(0, |receipt| receipt.created_at_unix_ms),
                "receipt",
            ),
        ],
        &[(worker.updated_at_unix_ms, "worker updated")],
    )
    .unwrap_or((assignment.updated_at_unix_ms, "assignment updated"));
    let (age_ms, age_source) = ctx.age(&id, last, source, mark);
    let receipt_blockers = receipt
        .map(|receipt| receipt.unresolved_blockers.clone())
        .unwrap_or_default();
    let facts = Facts {
        kind: ItemKind::Assignment,
        availability: candidate.map(|candidate| candidate.availability),
        lifecycle: Some(assignment.lifecycle),
        has_disposition: receipt.is_some() || assignment.cancellation.is_some(),
        has_receipt: receipt.is_some(),
        worker_live: live,
        herdr: status.unwrap_or(ObservedStatus::Unknown),
        bound_visible: live,
        question_pending: false,
        report: None,
        last_command_failed: ctx.inputs.failed_commands.contains(&assignment.id),
        age_ms,
        explicit_next: !receipt_blockers.is_empty()
            || ctx.snapshot_open(Some(&assignment.project_id)),
    };
    let objective = first_line(&assignment.objective, MAX_LINE_CHARS);
    let status_line = match assignment.lifecycle {
        AssignmentLifecycle::Completed => receipt.map_or_else(
            || objective.clone(),
            |receipt| first_line(&receipt.summary, MAX_LINE_CHARS),
        ),
        AssignmentLifecycle::Failed => assignment.attempt.error.as_deref().map_or_else(
            || objective.clone(),
            |error| first_line(error, MAX_LINE_CHARS),
        ),
        _ => objective.clone(),
    };
    let place = project
        .map(|project| project.name.clone())
        .or_else(|| talk.place());
    let (title, title_source) = ctx.title(
        &TitleSources {
            display_name: worker.display_name.as_deref(),
            terminal_title: talk.pane_label(),
            conversation_title: talk.title(),
            agent_name: talk.agent_name(),
            tab_label: talk.tab_label(),
            objective: &assignment.objective,
            role: None,
            provider: talk.provider.as_deref(),
            place: place.as_deref(),
        },
        &talk,
    );
    // A task's About never says "no task": the title came from somewhere.
    let about = plain::about_conversation(talk.title(), &title)
        .or_else(|| plain::about_task(&assignment.objective, &title))
        .or_else(|| plain::about_request(talk.first_user(), &title))
        .unwrap_or_default();
    new_item(
        id,
        facts,
        mark,
        ItemParts {
            title_source,
            about,
            activity: Some(ctx.inputs.now_ms.saturating_sub(age_ms)),
            session_id: talk.session.clone(),
            provider: talk.provider.clone(),
            digest: talk.digest(),
            last_agent: talk.last_agent(),
            folder: talk.place(),
            helper: None,
            project_id: Some(assignment.project_id.clone()),
            project_name: project.map(|project| project.name.clone()),
            worker_id: Some(worker.id.clone()),
            assignment_id: Some(assignment.id.clone()),
            title,
            objective,
            status_line,
            next_actions: receipt_blockers
                .iter()
                .map(|blocker| crate::redact::clean_text(blocker, todos::MAX_TODO_CHARS))
                .take(MAX_NEXT_ACTIONS)
                .collect(),
            age_source,
            details: vec![format!("Task state: {:?}", assignment.lifecycle)],
        },
    )
}

struct ItemParts {
    title_source: &'static str,
    about: String,
    /// Last activity, Unix ms, when known.
    activity: Option<u64>,
    session_id: Option<String>,
    provider: Option<String>,
    digest: String,
    last_agent: Option<String>,
    folder: Option<String>,
    helper: Option<&'static str>,
    project_id: Option<String>,
    project_name: Option<String>,
    worker_id: Option<String>,
    assignment_id: Option<String>,
    title: String,
    objective: String,
    status_line: String,
    next_actions: Vec<String>,
    age_source: &'static str,
    details: Vec<String>,
}

fn new_item(id: String, facts: Facts, mark: Mark, parts: ItemParts) -> Item {
    let base = rules::classify(&facts);
    Item {
        id,
        kind: facts.kind,
        column: base.column,
        section: base.section,
        rule: base.rule,
        chips: base.chips.clone(),
        project_id: parts.project_id,
        project_name: parts.project_name,
        worker_id: parts.worker_id,
        assignment_id: parts.assignment_id,
        title: parts.title,
        title_source: parts.title_source,
        objective: parts.objective,
        status_line: parts.status_line,
        about: String::new(),
        status: String::new(),
        your_move: String::new(),
        activity_known: parts.activity.is_some(),
        relevance: Relevance::Older,
        helper: parts.helper,
        pinned: false,
        reviewed: false,
        suggest_archive: false,
        provider: parts.provider,
        summary: String::new(),
        next_step: String::new(),
        labels: Vec::new(),
        details: parts.details,
        next_actions: parts.next_actions,
        age_ms: facts.age_ms,
        age_source: parts.age_source,
        heat: rules::heat(facts.age_ms),
        ai: None,
        actions: Vec::new(),
        rollup: false,
        pending: false,
        dead_count: None,
        facts,
        base,
        mark,
        about_base: parts.about,
        activity_at: parts.activity,
        session_id: parts.session_id,
        digest: parts.digest,
        last_agent: parts.last_agent,
        folder: parts.folder,
        need: None,
        reply: None,
        subagents: Vec::new(),
    }
}

#[allow(clippy::too_many_lines)]
fn worker_item(ctx: &Ctx<'_>, candidate: &WorkerCandidate) -> Item {
    let worker = &candidate.worker;
    let id = format!("w:{}", worker.id);
    let mark = Mark {
        state_change_sequence: worker
            .runtime
            .as_ref()
            .map_or(0, |runtime| runtime.state_change_sequence),
        tail_revision: ctx.inputs.tail_revisions.get(&id).copied(),
        last_changed_ms: None,
    };
    let status = ctx.live(worker);
    let live = status.is_some();
    let talk = ctx.talk(worker.runtime.as_ref());
    let (last, source) = activity(
        &talk,
        status == Some(ObservedStatus::Working),
        ctx.inputs.now_ms,
        &[],
        &[(worker.updated_at_unix_ms, "worker updated")],
    )
    .unwrap_or((worker.updated_at_unix_ms, "worker updated"));
    let (age_ms, age_source) = ctx.age(&id, last, source, mark);
    let facts = Facts {
        availability: Some(candidate.availability),
        worker_live: live,
        herdr: status.unwrap_or(ObservedStatus::Unknown),
        bound_visible: live,
        age_ms,
        explicit_next: ctx.snapshot_open(candidate.project_id.as_deref()),
        ..Facts::neutral(ItemKind::Worker)
    };
    let role = yard_role(candidate.availability);
    // Yard-level agents (the Yard orchestrator, workstreams, an orchestrator
    // of a project not on the board) show their Herdr status, others what
    // they are available for, never the raw
    // reservation reason (which goes to Details, like the availability).
    let status_line = if role.is_some() {
        plain::herdr_phrase(facts.herdr, live)
    } else {
        plain::availability_phrase(candidate.availability)
    };
    let mut details = vec![format!("Yard availability: {:?}", candidate.availability)];
    details.extend(
        candidate
            .reason
            .as_deref()
            .map(|reason| format!("Yard says: {}", first_line(reason, MAX_LINE_CHARS))),
    );
    let project_name = candidate
        .project_id
        .as_deref()
        .and_then(|id| ctx.projects.get(id))
        .map(|project| project.name.clone())
        .or_else(|| {
            matches!(
                candidate.availability,
                WorkerAvailability::YardOrchestrator | WorkerAvailability::CoordinationNode
            )
            .then(|| "Yard".to_owned())
        });
    let place = project_name.clone().or_else(|| talk.place());
    // A Yard-level agent is named for its role, never its profile.
    let (title, title_source) = ctx.title(
        &TitleSources {
            display_name: worker.display_name.as_deref(),
            terminal_title: talk.pane_label(),
            conversation_title: talk.title(),
            agent_name: talk.agent_name(),
            tab_label: talk.tab_label(),
            objective: "",
            role,
            provider: talk.provider.as_deref(),
            place: place.as_deref(),
        },
        &talk,
    );
    let about = match candidate.availability {
        WorkerAvailability::YardOrchestrator
        | WorkerAvailability::CoordinationNode
        | WorkerAvailability::Orchestrator => {
            let base = plain::availability_phrase(candidate.availability);
            plain::about_conversation(talk.title(), &title)
                .filter(|topic| plain::plain_topic(topic))
                .map_or_else(
                    || base.to_owned(),
                    |about| format!("{base}; latest topic: {}", plain::lower_first(&about)),
                )
        }
        _ => plain::about_conversation(talk.title(), &title)
            .or_else(|| plain::about_request(talk.first_user(), &title))
            .unwrap_or_default(),
    };
    // An agent no person talks to (only tool traffic in its conversation)
    // is a helper, unless it is a Yard-level agent.
    let helper = if role.is_some() {
        None
    } else if talk.conversation.is_some_and(Conversation::is_yard_summary) {
        Some(plain::HELPER_SUMMARY)
    } else if talk
        .conversation
        .is_some_and(|conversation| !conversation.maybe_human())
    {
        Some(plain::HELPER_NO_CONVERSATION)
    } else {
        None
    };
    new_item(
        id,
        facts,
        mark,
        ItemParts {
            title_source,
            about,
            activity: Some(ctx.inputs.now_ms.saturating_sub(age_ms)),
            session_id: talk.session.clone(),
            provider: talk.provider.clone(),
            digest: talk.digest(),
            last_agent: talk.last_agent(),
            folder: talk.place(),
            helper,
            project_name,
            project_id: candidate.project_id.clone(),
            worker_id: Some(worker.id.clone()),
            assignment_id: candidate.assignment_id.clone(),
            title,
            objective: String::new(),
            status_line: status_line.to_owned(),
            next_actions: Vec::new(),
            age_source,
            details,
        },
    )
}

fn report_facts(
    report: &OrchestratorStatusReport,
    age_ms: u64,
    activity_after: bool,
) -> ReportFacts {
    ReportFacts {
        state: report.state,
        next_empty: report.next.trim().is_empty(),
        blockers_empty: report
            .blockers
            .iter()
            .all(|blocker| blocker.trim().is_empty()),
        age_ms,
        activity_after,
    }
}

/// An assignment's unresolved receipt blockers, cleaned for display.
fn receipt_blockers(assignment: &Assignment) -> Vec<String> {
    assignment
        .completion_receipt
        .as_ref()
        .map(|receipt| {
            receipt
                .unresolved_blockers
                .iter()
                .map(|blocker| crate::redact::clean_text(blocker, todos::MAX_TODO_CHARS))
                .filter(|blocker| !blocker.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A project orchestrator's card. `folded` is an open assignment its
/// worker runs (one card per pane): its lifecycle, receipt, failed-command
/// state and receipt blockers become this card's facts, chips and next
/// actions instead of being dropped.
#[allow(clippy::too_many_lines)]
fn orchestrator_item(ctx: &Ctx<'_>, project: &Project, folded: Option<&Assignment>) -> Item {
    let candidate = ctx
        .candidates
        .get(project.orchestrator.id.as_str())
        .copied();
    let worker = candidate.map_or(&project.orchestrator, |candidate| &candidate.worker);
    let id = format!("o:{}", project.id);
    let mark = Mark {
        state_change_sequence: worker
            .runtime
            .as_ref()
            .map_or(0, |runtime| runtime.state_change_sequence),
        tail_revision: None,
        last_changed_ms: None,
    };
    let report = ctx.inputs.reports.get(&project.id);
    let status = ctx.live(worker);
    let live = status.is_some();
    let talk = ctx.talk(worker.runtime.as_ref());
    let (last, source) = activity(
        &talk,
        status == Some(ObservedStatus::Working),
        ctx.inputs.now_ms,
        &[],
        &[(worker.updated_at_unix_ms, "orchestrator updated")],
    )
    .unwrap_or((worker.updated_at_unix_ms, "orchestrator updated"));
    let (age_ms, age_source) = ctx.age(&id, last, source, mark);
    let herdr = status.unwrap_or(ObservedStatus::Unknown);
    // Activity after the report (which has no timestamp of its own, so it is
    // dated by the orchestrator's last update): the orchestrator is working
    // now, its runtime state moved since the previous board, or an
    // assignment in the project changed after the orchestrator did.
    let sequence_moved = ctx
        .previous
        .and_then(|previous| previous.marks.get(&id))
        .is_some_and(|old| old.state_change_sequence != mark.state_change_sequence);
    let assignment_after = ctx.inputs.assignments.iter().any(|assignment| {
        assignment.project_id == project.id
            && assignment.updated_at_unix_ms > worker.updated_at_unix_ms
    });
    let activity_after = herdr == ObservedStatus::Working || sequence_moved || assignment_after;
    let folded_blockers = folded.map(receipt_blockers).unwrap_or_default();
    let facts = Facts {
        availability: candidate.map(|candidate| candidate.availability),
        worker_live: live,
        herdr,
        bound_visible: live,
        // The report has no timestamp of its own: it is as old as the
        // orchestrator's last recorded change.
        report: report.map(|report| report_facts(report, age_ms, activity_after)),
        age_ms,
        explicit_next: report.is_some_and(|report| !report.next.trim().is_empty())
            || !folded_blockers.is_empty(),
        lifecycle: folded.map(|assignment| assignment.lifecycle),
        has_disposition: folded.is_some_and(|assignment| {
            assignment.completion_receipt.is_some() || assignment.cancellation.is_some()
        }),
        has_receipt: folded.is_some_and(|assignment| assignment.completion_receipt.is_some()),
        last_command_failed: folded
            .is_some_and(|assignment| ctx.inputs.failed_commands.contains(&assignment.id)),
        ..Facts::neutral(ItemKind::Orchestrator)
    };
    let failed_line = folded
        .filter(|assignment| assignment.lifecycle == AssignmentLifecycle::Failed)
        .and_then(|assignment| assignment.attempt.error.as_deref())
        .map(|error| first_line(error, MAX_LINE_CHARS));
    let status_line = failed_line.unwrap_or_else(|| {
        report.map_or_else(
            || plain::herdr_phrase(facts.herdr, live).to_owned(),
            |report| first_line(&report.last, MAX_LINE_CHARS),
        )
    });
    let mut next_actions = folded_blockers;
    if let Some(report) = report {
        next_actions.extend(
            report
                .blockers
                .iter()
                .map(|blocker| crate::redact::clean_text(blocker, todos::MAX_TODO_CHARS))
                .filter(|blocker| !blocker.is_empty()),
        );
        next_actions.extend(todos::split_bullets(&report.next));
    }
    next_actions.truncate(MAX_NEXT_ACTIONS);
    // Titled by what it is on now (its open task, else the last thing a
    // person asked it), with the role as About; the role alone is the
    // fallback title, and then About says only what the title does not.
    let goal = folded
        .and_then(|assignment| plain::about_task(&assignment.objective, ""))
        .or_else(|| plain::about_request(talk.last_user(), ""))
        // Yard's own bookkeeping ("Continue orchestration for …") is not a goal.
        .filter(|goal| !goal.ends_with('\u{2026}') && plain::plain_topic(goal));
    let topic =
        plain::about_conversation(talk.title(), "").filter(|topic| plain::plain_topic(topic));
    let (orchestrator_title, orchestrator_source, orchestrator_about) = match goal {
        Some(goal) => (
            crate::redact::clean_text(&format!("Coordinator: {goal}"), MAX_TITLE_CHARS),
            titles::FROM_GOAL,
            format!("Coordinates the {} project", project.name),
        ),
        None => (
            crate::redact::clean_text(&format!("{} orchestrator", project.name), MAX_TITLE_CHARS),
            titles::FROM_ROLE,
            topic.unwrap_or_default(),
        ),
    };
    let mut item = new_item(
        id,
        facts,
        mark,
        ItemParts {
            title_source: orchestrator_source,
            about: orchestrator_about,
            activity: Some(ctx.inputs.now_ms.saturating_sub(age_ms)),
            session_id: talk.session.clone(),
            provider: talk.provider.clone(),
            digest: talk.digest(),
            last_agent: talk.last_agent(),
            folder: talk.place(),
            helper: None,
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            worker_id: Some(worker.id.clone()),
            assignment_id: folded.map(|assignment| assignment.id.clone()),
            title: orchestrator_title,
            objective: String::new(),
            status_line,
            next_actions,
            age_source,
            details: folded
                .map(|assignment| vec![format!("Task state: {:?}", assignment.lifecycle)])
                .unwrap_or_default(),
        },
    );
    item.rollup = item.column != Column::NeedsYou;
    item
}

#[allow(clippy::too_many_lines)]
fn loose_item(ctx: &Ctx<'_>, agent: &LooseAgent) -> Item {
    let id = format!("l:{}:{}", agent.session, agent.pane_id);
    // Herdr reports no timestamp for unbound agents, so the board remembers
    // when it last saw the state change: an unchanged sequence keeps the old
    // time, a changed one dates the change to the previous generation, and a
    // first sighting starts at now ("first seen"). Those are not ages a
    // person can use; only the conversation (or working now) dates it.
    let now = ctx.inputs.now_ms;
    let (last_changed, mut age_source) = match ctx
        .previous
        .and_then(|previous| previous.marks.get(&id).map(|old| (previous, old)))
    {
        Some((_, old)) if old.state_change_sequence == agent.state_change_sequence => (
            old.last_changed_ms.unwrap_or(now),
            "unchanged since first seen",
        ),
        Some((previous, _)) => (previous.generated_at_ms.min(now), "runtime status changed"),
        None => (now, "first seen now"),
    };
    let mark = Mark {
        state_change_sequence: agent.state_change_sequence,
        tail_revision: None,
        last_changed_ms: Some(last_changed),
    };
    let pane = [&agent.terminal_id, &agent.pane_id]
        .into_iter()
        .find_map(|key| {
            ctx.inputs
                .pane_info
                .get(&(agent.session.clone(), key.clone()))
        });
    let session = pane.and_then(|pane| pane.provider_session.clone());
    let talk = Talk {
        pane,
        conversation: session
            .as_ref()
            .and_then(|id| ctx.inputs.conversations.get(id)),
        session,
        provider: pane.and_then(|pane| pane.provider.clone()),
    };
    let known = activity(
        &talk,
        agent.status == ObservedStatus::Working,
        now,
        &[],
        &[],
    );
    let age_ms = known.map_or_else(
        || now.saturating_sub(last_changed),
        |(at, source)| {
            age_source = source;
            now.saturating_sub(at)
        },
    );
    let facts = Facts {
        herdr: agent.status,
        age_ms,
        ..Facts::neutral(ItemKind::LooseAgent)
    };
    let place = talk.place();
    let agent_name = talk.agent_name().or(Some(agent.name.as_str()));
    let (title, title_source) = ctx.title(
        &TitleSources {
            terminal_title: talk.pane_label(),
            conversation_title: talk.title(),
            agent_name,
            tab_label: talk.tab_label(),
            provider: talk.provider.as_deref(),
            place: place.as_deref(),
            ..TitleSources::default()
        },
        &talk,
    );
    // Yard's summary workers, its temporary sessions with nobody talking to
    // them, and panes with no conversation are helpers. A Yard-started
    // session someone talks to is real work (it lost its Yard record).
    let talked_to = talk.conversation.is_some_and(Conversation::maybe_human);
    let helper = if talk.conversation.is_some_and(Conversation::is_yard_summary) {
        Some(plain::HELPER_SUMMARY)
    } else if agent_name.is_some_and(plain::temporary_session_name) && !talked_to {
        Some(plain::HELPER_TEMPORARY)
    } else if talk
        .conversation
        .is_some_and(|conversation| !conversation.maybe_human())
        || (talk.session.is_none() && title_source == titles::UNTITLED)
    {
        Some(plain::HELPER_NO_CONVERSATION)
    } else {
        None
    };
    let about = plain::about_conversation(talk.title(), &title)
        .or_else(|| plain::about_request(talk.first_user(), &title))
        .unwrap_or_default();
    new_item(
        id,
        facts,
        mark,
        ItemParts {
            title_source,
            about,
            activity: known.map(|_| now.saturating_sub(age_ms)),
            session_id: talk.session.clone(),
            provider: talk.provider.clone(),
            digest: talk.digest(),
            last_agent: talk.last_agent(),
            folder: talk.place(),
            helper,
            project_id: None,
            project_name: None,
            worker_id: None,
            assignment_id: None,
            title,
            objective: String::new(),
            status_line: plain::herdr_phrase(agent.status, true).to_owned(),
            next_actions: Vec::new(),
            age_source,
            details: vec![format!(
                "Herdr agent not linked to a Yard worker (status: {:?})",
                agent.status
            )],
        },
    )
}

/// Re-run the post-pass of one item: the model verdict when there is one,
/// else the deterministic tiebreak.
pub(crate) fn settle(item: &mut Item, snapshot_open: bool) {
    let placement = match &item.ai {
        Some(ai) => rules::apply_verdict(&item.base, ai.verdict, ai.confidence),
        None => rules::tiebreak(&item.base, &item.facts, snapshot_open),
    };
    item.column = placement.column;
    item.section = placement.section;
    item.rule = placement.rule;
    item.chips = placement.chips;
    item.reply = reply::classify(item.last_agent.as_deref());
    by_reply(item);
    if item.kind == ItemKind::Orchestrator {
        item.rollup = item.column != Column::NeedsYou;
    }
}

/// A quiet card is placed by what the agent last said: a blocker or a
/// question waits on the user (so the waiting window keeps it on the
/// board), a completion is probably done, and work it says is left is not.
/// Coordinators and helpers keep their place; nothing moves into Done.
fn by_reply(item: &mut Item) {
    if item.column != Column::Stale || plain::is_coordinator(item) {
        return;
    }
    let chip = |item: &mut Item, chip: &str| {
        if !item.chips.iter().any(|seen| seen == chip) {
            item.chips.insert(0, chip.to_owned());
        }
    };
    match &item.reply {
        Some(Reply::Blocked(_)) => {
            item.column = Column::NeedsYou;
            item.section = None;
            chip(item, plain::CHIP_BLOCKED_ON_YOU);
        }
        Some(Reply::Question) => {
            item.column = Column::NeedsYou;
            item.section = None;
            chip(item, plain::CHIP_ASKED_YOU);
        }
        Some(Reply::Done) => item.section = Some(Section::LikelyDone),
        Some(Reply::Remaining(_)) if item.section == Some(Section::LikelyDone) => {
            item.section = Some(Section::NeedsWork);
        }
        _ => {}
    }
}

fn actions(item: &Item) -> Vec<Action> {
    use AssignmentLifecycle as L;
    // Completion receipts (and dispositions) accept only an active
    // assignment; a failed one is opened and investigated, never "marked done".
    let assignment_open = matches!(item.facts.lifecycle, Some(L::Active));
    let has_terminal = item.facts.worker_live || item.kind == ItemKind::LooseAgent;
    let mut actions = Vec::new();
    let mut push = |action| {
        if !actions.contains(&action) {
            actions.push(action);
        }
    };
    match item.kind {
        ItemKind::DeadAggregate => push(Action::CleanUp),
        ItemKind::LooseAgent => {
            push(Action::OpenTerminal);
            push(Action::Hide);
        }
        ItemKind::Orchestrator => {
            // Open (its terminal) leads: on a Needs-you card it is how the
            // user answers it.
            if has_terminal {
                push(Action::OpenTerminal);
            }
            push(Action::OpenProject);
            // An open assignment folded into the orchestrator card.
            if assignment_open && item.assignment_id.is_some() {
                push(Action::MarkDone);
            }
        }
        ItemKind::Assignment | ItemKind::Worker => {
            let resumable = item.facts.availability == Some(WorkerAvailability::Resumable);
            if item.section == Some(Section::LikelyDone) && assignment_open {
                push(Action::MarkDone);
            }
            if item.column == Column::Close {
                push(Action::EndSession);
            }
            if has_terminal {
                push(Action::OpenTerminal);
            }
            if resumable && item.column != Column::Closed {
                push(Action::Resume);
            }
            if assignment_open && item.column != Column::Closed {
                push(Action::MarkDone);
            }
            if item.kind == ItemKind::Worker
                && item.facts.availability == Some(WorkerAvailability::UnassignedLive)
            {
                push(Action::EndSession);
            }
            if item.project_id.is_some() {
                push(Action::OpenProject);
            }
        }
    }
    actions
}

/// Build the deterministic board. Every item runs R0-R7, then the post-pass
/// with the tiebreak (model results are merged later by [`refresh`]).
pub(crate) fn build(inputs: &Inputs, previous: Option<&Previous>) -> Built {
    let ctx = Ctx {
        inputs,
        previous,
        candidates: inputs
            .workers
            .iter()
            .map(|candidate| (candidate.worker.id.as_str(), candidate))
            .collect(),
        projects: inputs
            .projects
            .iter()
            .map(|project| (project.id.as_str(), project))
            .collect(),
        profiles: profile_names(inputs),
    };
    let mut items = Vec::new();
    // One card per worker: its most relevant assignment.
    let mut best: HashMap<&str, &Assignment> = HashMap::new();
    for assignment in &inputs.assignments {
        let entry = best
            .entry(assignment.worker.id.as_str())
            .or_insert(assignment);
        if assignment_rank(assignment) > assignment_rank(entry) {
            *entry = assignment;
        }
    }
    // Only agents the user has open in Herdr now are cards; every other
    // worker record is counted in `hidden` (see `hidden`).
    // One card per Herdr pane: orchestrators claim theirs first, so an
    // assignment run by a project's orchestrator worker (folded into its
    // card) or a second record bound to an already shown pane is not a
    // second card.
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    let mut claim = |worker: &Worker| -> bool {
        let keys = pane_keys(worker);
        if keys.iter().any(|key| claimed.contains(key)) {
            return false;
        }
        claimed.extend(keys);
        true
    };
    let orchestrator_ids = inputs
        .projects
        .iter()
        .map(|project| project.orchestrator.id.as_str())
        .collect::<HashSet<_>>();
    for project in &inputs.projects {
        let worker = ctx
            .candidates
            .get(project.orchestrator.id.as_str())
            .map_or(&project.orchestrator, |candidate| &candidate.worker);
        if ctx.on_board(worker) && claim(worker) {
            let folded = best
                .get(worker.id.as_str())
                .copied()
                .filter(|assignment| assignment_rank(assignment).0 == 2);
            items.push(orchestrator_item(&ctx, project, folded));
        }
    }
    let mut covered: HashSet<&str> = HashSet::new();
    let mut chosen = best.into_values().collect::<Vec<_>>();
    chosen.sort_by(|left, right| left.id.cmp(&right.id));
    for assignment in chosen {
        covered.insert(assignment.worker.id.as_str());
        let worker = ctx
            .candidates
            .get(assignment.worker.id.as_str())
            .map_or(&assignment.worker, |candidate| &candidate.worker);
        if !orchestrator_ids.contains(worker.id.as_str()) && ctx.on_board(worker) && claim(worker) {
            items.push(assignment_item(&ctx, assignment));
        }
    }
    // The Yard orchestrator and workstream (coordination-node) agents first,
    // then every other live worker without an assignment.
    let mut rest = inputs
        .workers
        .iter()
        .filter(|candidate| {
            let id = candidate.worker.id.as_str();
            !covered.contains(id)
                && !orchestrator_ids.contains(id)
                && candidate.availability != WorkerAvailability::Unavailable
                && ctx.on_board(&candidate.worker)
        })
        .collect::<Vec<_>>();
    rest.sort_by_key(|candidate| yard_role(candidate.availability).is_none());
    for candidate in rest {
        if claim(&candidate.worker) {
            items.push(worker_item(&ctx, candidate));
        }
    }
    // A Herdr agent pane no card claims (unbound, or bound to a record that
    // is not on the board) is a loose agent.
    for agent in inputs.loose_agents.iter().filter(|agent| {
        !claimed.contains(&(agent.session.clone(), agent.terminal_id.clone()))
            && !claimed.contains(&(agent.session.clone(), agent.pane_id.clone()))
    }) {
        items.push(loose_item(&ctx, agent));
    }
    finish(inputs, items)
}

/// Every profile and role name Yard knows, lower-cased (never a title).
fn profile_names(inputs: &Inputs) -> HashSet<String> {
    let mut names = HashSet::new();
    for candidate in &inputs.workers {
        names.extend(
            candidate
                .profile_name
                .iter()
                .map(|name| name.to_lowercase()),
        );
        names.extend(
            candidate
                .default_role
                .iter()
                .map(|role| role.to_lowercase()),
        );
    }
    for assignment in &inputs.assignments {
        names.insert(assignment.profile_name.to_lowercase());
        names.insert(assignment.role.to_lowercase());
    }
    names.retain(|name| !name.trim().is_empty());
    names
}

/// The board label of a Yard-level agent (not a project worker), if any.
const fn yard_role(availability: WorkerAvailability) -> Option<&'static str> {
    match availability {
        WorkerAvailability::YardOrchestrator => Some("Yard orchestrator"),
        WorkerAvailability::CoordinationNode => Some("Workstream"),
        WorkerAvailability::Orchestrator => Some("Orchestrator"),
        _ => None,
    }
}

/// Worker records with no card because no live Herdr agent backs them
/// (ended, archived, dead or missing runtimes). A live record without a
/// card (a second record on an already shown pane) is not counted, and
/// neither are the Yard orchestrator and coordination nodes, which the
/// cleanup dialog cannot remove.
fn hidden(inputs: &Inputs, items: &[Item]) -> Hidden {
    let shown = items
        .iter()
        .filter_map(|item| item.worker_id.as_deref())
        .collect::<HashSet<_>>();
    let availability = inputs
        .workers
        .iter()
        .map(|candidate| (candidate.worker.id.as_str(), candidate.availability))
        .collect::<HashMap<_, _>>();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Hidden::default();
    let mut add = |worker: &Worker| {
        let id = worker.id.as_str();
        let state = availability.get(id).copied();
        if shown.contains(id)
            || !seen.insert(id.to_owned())
            || matches!(
                state,
                Some(WorkerAvailability::YardOrchestrator | WorkerAvailability::CoordinationNode)
            )
            || on_board(inputs, worker, state)
        {
            return;
        }
        out.not_live += 1;
        if state == Some(WorkerAvailability::Unavailable) {
            out.dead_runtimes += 1;
        }
    };
    for candidate in &inputs.workers {
        add(&candidate.worker);
    }
    for assignment in &inputs.assignments {
        add(&assignment.worker);
    }
    for project in &inputs.projects {
        add(&project.orchestrator);
    }
    out
}

/// Re-settle every item and recompute actions, headers, counts and to-dos.
#[allow(clippy::too_many_lines)]
pub(crate) fn finish(inputs: &Inputs, mut items: Vec<Item>) -> Built {
    for item in &mut items {
        let open = item
            .project_id
            .as_deref()
            .and_then(|id| inputs.snapshots.get(id))
            .is_some_and(SnapshotNext::has_open_checkbox);
        settle(item, open);
    }
    // Chats join their folder's project first, so the archive-project hint
    // below counts them too.
    attach_by_folder(inputs, &mut items);
    // Archive-project hint: every item of the project is closable or closed.
    let mut closable: HashMap<&str, bool> = HashMap::new();
    for item in &items {
        if let Some(project_id) = item.project_id.as_deref()
            && item.kind != ItemKind::Orchestrator
        {
            let ok = matches!(item.column, Column::Close | Column::Closed);
            closable
                .entry(project_id)
                .and_modify(|all| *all &= ok)
                .or_insert(ok);
        }
    }
    let closable = closable
        .into_iter()
        .filter(|(_, all)| *all)
        .map(|(id, _)| id.to_owned())
        .collect::<HashSet<_>>();
    for item in &mut items {
        // Idle Yard coordinators are helpers (never a "still has work"
        // card); one that wakes up comes back.
        if plain::idle_coordinator(item) {
            item.helper = Some(plain::HELPER_IDLE);
        } else if item.helper == Some(plain::HELPER_IDLE) {
            item.helper = None;
        }
        item.actions = actions(item);
        if item.column == Column::Close
            && item
                .project_id
                .as_ref()
                .is_some_and(|id| closable.contains(id))
        {
            item.actions.push(Action::ArchiveProject);
        }
        relate(item, inputs);
        item.subagents = item
            .session_id
            .as_deref()
            .map(|session| {
                inputs
                    .children
                    .iter()
                    .filter(|child| {
                        child.parent_session == session && child.status == ObservedStatus::Working
                    })
                    .map(|child| crate::redact::clean_text(&child.title, MAX_TITLE_CHARS))
                    .collect()
            })
            .unwrap_or_default();
        describe(item);
    }
    sort(&mut items, &inputs.projects);
    let counts = count(items.iter().filter(|item| on_main(item)));
    // Only projects with a card on the main board get a header.
    let projects = inputs
        .projects
        .iter()
        .filter(|project| {
            items.iter().any(|item| {
                !item.rollup && on_main(item) && item.project_id.as_deref() == Some(&project.id)
            })
        })
        .map(|project| {
            let orchestrator = items.iter().find(|item| {
                item.kind == ItemKind::Orchestrator
                    && item.project_id.as_deref() == Some(&project.id)
            });
            ProjectHeader {
                id: project.id.clone(),
                name: crate::redact::clean_text(&project.name, MAX_TITLE_CHARS),
                status_line: orchestrator
                    .map(|item| item.status_line.clone())
                    .unwrap_or_default(),
                summary: orchestrator
                    .map(|item| item.summary.clone())
                    .unwrap_or_default(),
                orchestrator_column: orchestrator.map_or(Column::Stale, |item| item.column),
                counts: count(items.iter().filter(|item| {
                    on_main(item) && item.project_id.as_deref() == Some(&project.id)
                })),
            }
        })
        .collect();
    // The full list: the web applies the per-agent / per-project / total
    // caps after the user's local dismissals, so a dismissed to-do never
    // takes a slot.
    let main = items
        .iter()
        .filter(|item| on_main(item))
        .cloned()
        .collect::<Vec<_>>();
    let todos = extract_todos(inputs, &main);
    let hidden = hidden(inputs, &items);
    let helper_rows = helper_rows(inputs, &items);
    let cards = items
        .iter()
        .filter(|item| !item.rollup && item.column != Column::Closed);
    let older = cards
        .clone()
        .filter(|item| item.helper.is_none() && item.relevance == Relevance::Older)
        .count();
    let to_review = cards
        .clone()
        .filter(|item| {
            item.helper.is_none() && item.relevance == Relevance::Older && !item.reviewed
        })
        .count();
    let helpers = cards.filter(|item| item.helper.is_some()).count() + helper_rows.len();
    Built {
        older,
        to_review,
        helpers,
        helper_rows,
        window: inputs.window,
        items,
        projects,
        counts,
        todos,
        hidden,
        fingerprint: fingerprint(&inputs.workers),
    }
}

/// A chat with no project whose working folder is a project's (its name,
/// or the folder its orchestrator runs in) joins that project: chip,
/// filter and header. Yard-level agents keep their own "Yard" place.
fn attach_by_folder(inputs: &Inputs, items: &mut [Item]) {
    let folder_of = |cwd: &str| {
        std::path::Path::new(cwd.trim_end_matches('/'))
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_lowercase)
    };
    let mut by_folder: HashMap<String, (&str, &str)> = HashMap::new();
    for project in &inputs.projects {
        let entry = (project.id.as_str(), project.name.as_str());
        by_folder
            .entry(project.name.trim().to_lowercase())
            .or_insert(entry);
        if let Some(runtime) = project.orchestrator.runtime.as_ref()
            && let Some(cwd) = [&runtime.terminal_id, &runtime.pane_id]
                .into_iter()
                .find_map(|id| inputs.pane_info.get(&(runtime.session.clone(), id.clone())))
                .and_then(|pane| pane.cwd.as_deref())
                .and_then(folder_of)
        {
            by_folder.entry(cwd).or_insert(entry);
        }
    }
    for item in items.iter_mut().filter(|item| {
        item.project_id.is_none()
            && item.project_name.is_none()
            && matches!(item.kind, ItemKind::LooseAgent | ItemKind::Worker)
    }) {
        let Some(folder) = item.folder.as_deref().and_then(folder_of) else {
            continue;
        };
        if let Some((id, name)) = by_folder.get(&folder) {
            item.project_id = Some((*id).to_owned());
            item.project_name = Some(crate::redact::clean_text(name, MAX_TITLE_CHARS));
            item.details
                .push("Project: matched by its working folder".to_owned());
        }
    }
}

/// On the main board: not a helper and not in Older work.
pub(crate) fn on_main(item: &Item) -> bool {
    item.helper.is_none() && item.relevance != Relevance::Older
}

/// The relevance window: recent activity (or working now), waiting on the
/// user inside the waiting window, or kept by the user; else Older work.
/// A card whose age is unknown is recent only while it works. Also marks
/// skipped-and-unchanged cards as reviewed and very old ones for archive.
pub(crate) fn relate(item: &mut Item, inputs: &Inputs) {
    let window = inputs.window;
    let age = item.activity_at.map(|at| inputs.now_ms.saturating_sub(at));
    let decision = inputs.triage.get(&item.id);
    item.pinned = decision.is_some_and(|decision| decision.keep);
    item.reviewed = decision.is_some_and(|decision| {
        !decision.keep && item.activity_at.is_none_or(|at| at <= decision.at_ms)
    });
    item.suggest_archive = age.is_some_and(|age| age >= window.suggest_archive_ms);
    let working = item.facts.herdr == ObservedStatus::Working;
    item.relevance = if working || age.is_some_and(|age| age < window.recent_ms) {
        Relevance::Recent
    } else if item.column == Column::NeedsYou && age.is_some_and(|age| age < window.waiting_ms) {
        Relevance::Waiting
    } else if item.pinned {
        Relevance::Pinned
    } else {
        Relevance::Older
    };
}

/// The plain-language face of a settled item: labels, what is happening
/// and what to do next (templates in `plain`, never raw status fields),
/// and the three card lines (the AI's when it wrote them).
pub(crate) fn describe(item: &mut Item) {
    if item.title_source == titles::UNTITLED
        && let Some(title) = item
            .ai
            .as_ref()
            .map(|ai| ai.title.trim())
            .filter(|title| !title.is_empty())
    {
        item.title = title.to_owned();
        item.title_source = titles::FROM_AI;
    }
    item.summary = plain::summary(item);
    item.next_step = plain::next_step(item);
    item.labels = plain::labels(item);
    let ai = item.ai.clone();
    let pick = |text: Option<&str>, fallback: String| {
        text.map(str::trim)
            .filter(|text| !text.is_empty())
            .map_or(fallback, str::to_owned)
    };
    // Nothing known beyond the title: no About line (never a placeholder).
    item.need = plain::need(item);
    item.about = pick(
        ai.as_ref().map(|ai| ai.about.as_str()),
        item.about_base.clone(),
    );
    item.status = pick(
        ai.as_ref().map(|ai| ai.status.as_str()),
        plain::status(item),
    );
    item.your_move = if item.column == Column::Doing
        || (item.suggest_archive && item.relevance == Relevance::Older)
    {
        plain::your_move(item)
    } else {
        pick(
            ai.as_ref().map(|ai| ai.your_move.as_str()),
            plain::your_move(item),
        )
    };
}

/// Subagents as helper rows, attached to the card whose conversation runs
/// them when it is on the board.
fn helper_rows(inputs: &Inputs, items: &[Item]) -> Vec<HelperRow> {
    let mut rows = inputs
        .children
        .iter()
        .map(|child| {
            let parent = items
                .iter()
                .find(|item| item.session_id.as_deref() == Some(child.parent_session.as_str()));
            HelperRow {
                id: crate::redact::clean_text(&child.id, 160),
                title: crate::redact::clean_text(&child.title, MAX_TITLE_CHARS),
                parent_id: parent.map(|item| item.id.clone()),
                parent_title: parent.map(|item| item.title.clone()),
                status: plain::herdr_phrase(child.status, true).to_owned(),
                age_ms: inputs.now_ms.saturating_sub(child.updated_at_ms),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.age_ms
            .cmp(&right.age_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    rows
}

fn count<'a>(items: impl Iterator<Item = &'a Item>) -> Counts {
    let mut counts = Counts::default();
    for item in items.filter(|item| !item.rollup) {
        match (item.column, item.section) {
            (Column::Doing, _) => counts.doing += 1,
            (Column::NeedsYou, _) => counts.needs_you += 1,
            (Column::Stale, Some(Section::LikelyDone)) => counts.stale_likely_done += 1,
            (Column::Stale, _) => counts.stale_needs_work += 1,
            (Column::Close, _) => counts.close += 1,
            (Column::Closed, _) => counts.closed += 1,
        }
    }
    counts
}

const fn column_order(column: Column) -> u8 {
    match column {
        Column::NeedsYou => 0,
        Column::Doing => 1,
        Column::Stale => 2,
        Column::Close => 3,
        Column::Closed => 4,
    }
}

/// Within a column cards are grouped by project (project list order, cards
/// without a project last). Then Needs you: answer before review, then oldest
/// first; Doing: newest first; others oldest first. Ties by id, so the
/// order is stable.
fn sort(items: &mut [Item], projects: &[Project]) {
    let rank = projects
        .iter()
        .enumerate()
        .map(|(index, project)| (project.id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let group = |item: &Item| {
        item.project_id
            .as_deref()
            .and_then(|id| rank.get(id).copied())
            .unwrap_or(usize::MAX)
    };
    items.sort_by(|left, right| {
        column_order(left.column)
            .cmp(&column_order(right.column))
            // Needs you is one queue, oldest first (like the to-dos);
            // other columns group by project.
            .then_with(|| {
                if left.column == Column::NeedsYou {
                    std::cmp::Ordering::Equal
                } else {
                    group(left).cmp(&group(right))
                }
            })
            // Questions and blockers before finished work to review.
            .then_with(|| {
                if left.column == Column::NeedsYou {
                    (left.need == Some(plain::NEED_REVIEW))
                        .cmp(&(right.need == Some(plain::NEED_REVIEW)))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| match left.column {
                Column::Doing | Column::Closed => left.age_ms.cmp(&right.age_ms),
                _ => right.age_ms.cmp(&left.age_ms),
            })
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn todo(text: String, kind: TodoKind, urgency: Urgency, item: Option<&Item>, age_ms: u64) -> Todo {
    let mut sources = Vec::new();
    if let Some(item) = item {
        sources.push(TodoSource {
            kind: "item".to_owned(),
            id: item.id.clone(),
        });
    }
    Todo {
        id: todos::todo_id(&text),
        text,
        kind,
        urgency,
        project_id: item.and_then(|item| item.project_id.clone()),
        sources,
        age_ms,
    }
}

#[allow(clippy::too_many_lines)]
fn extract_todos(inputs: &Inputs, items: &[Item]) -> Vec<Todo> {
    let mut out = Vec::new();
    for item in items.iter().filter(|item| item.column != Column::Closed) {
        let age = item.age_ms;
        let who = plain::who(item);
        let first = out.len();
        if item.column == Column::NeedsYou {
            for chip in &item.chips {
                let kind = match chip.as_str() {
                    "failed" | "ambiguous runtime" | "command failed" => TodoKind::Investigate,
                    "blocked" => TodoKind::Unblock,
                    "question" | "waiting on you" | plain::CHIP_ASKED_YOU => TodoKind::Reply,
                    plain::CHIP_BLOCKED_ON_YOU => TodoKind::Unblock,
                    "ready for review" => TodoKind::Review,
                    _ => continue,
                };
                let Some(text) = plain::chip_todo(chip, item, &who) else {
                    continue;
                };
                // Finished work to review is "Soon": questions and blockers
                // are what to do now.
                let urgency = if item.need == Some(plain::NEED_REVIEW) {
                    Urgency::Soon
                } else {
                    Urgency::Now
                };
                out.push(todo(text, kind, urgency, Some(item), age));
            }
        }
        if item.kind == ItemKind::Orchestrator
            && let Some(report) = item
                .project_id
                .as_deref()
                .and_then(|id| inputs.reports.get(id))
        {
            for blocker in report
                .blockers
                .iter()
                .filter(|blocker| !blocker.trim().is_empty())
            {
                let text = plain::blocker_todo(
                    &who,
                    &crate::redact::clean_text(blocker, todos::MAX_TODO_CHARS),
                );
                out.push(todo(text, TodoKind::Unblock, Urgency::Now, Some(item), age));
            }
            // The orchestrator's own next steps are its work, not the
            // user's: one row, and only when it went quiet (working, it
            // needs nothing; needing you, the now to-do comes first).
            if item.column == Column::Stale
                && let Some(text) =
                    plain::steps_todo(&who, &todos::split_bullets(&report.next), true)
            {
                out.push(todo(text, TodoKind::Next, Urgency::Soon, Some(item), age));
            }
        }
        // Every Needs-you card yields a to-do (now, or soon for a review),
        // even one whose chip (e.g.
        // "needs attention" with no blockers) maps to nothing above.
        if item.column == Column::NeedsYou
            && !out[first..]
                .iter()
                .any(|todo: &Todo| todo.urgency == Urgency::Now || todo.kind == TodoKind::Review)
        {
            let text = plain::check_todo(&who, &first_line(&item.status_line, MAX_LINE_CHARS));
            out.push(todo(
                text,
                TodoKind::Investigate,
                Urgency::Now,
                Some(item),
                age,
            ));
        }
        if item.kind == ItemKind::Orchestrator
            && let Some(folded) = item.assignment_id.as_deref().and_then(|id| {
                inputs
                    .assignments
                    .iter()
                    .find(|assignment| assignment.id == id)
            })
        {
            for blocker in receipt_blockers(folded) {
                out.push(todo(
                    plain::follow_up_todo(&who, &blocker),
                    TodoKind::ReceiptBlocker,
                    Urgency::Soon,
                    Some(item),
                    age,
                ));
            }
        }
        if item.kind == ItemKind::Assignment && item.column == Column::Stale {
            // A quiet agent's unfinished steps: one row naming the agent.
            if let Some(text) = plain::steps_todo(&who, &item.next_actions, true) {
                out.push(todo(text, TodoKind::Next, Urgency::Soon, Some(item), age));
            }
        } else if item.kind == ItemKind::Assignment {
            for blocker in &item.next_actions {
                out.push(todo(
                    plain::follow_up_todo(&who, blocker),
                    TodoKind::ReceiptBlocker,
                    Urgency::Soon,
                    Some(item),
                    age,
                ));
            }
        }
        if item.section == Some(Section::LikelyDone) || item.column == Column::Close {
            let text = plain::close_todo(item, &who);
            let kind = if item.kind == ItemKind::DeadAggregate {
                TodoKind::CleanUp
            } else {
                TodoKind::Close
            };
            if !item.rollup {
                out.push(todo(text, kind, Urgency::Later, Some(item), age));
            }
        } else if item.kind == ItemKind::LooseAgent && item.column == Column::Stale {
            out.push(todo(
                plain::loose_todo(&who),
                TodoKind::Triage,
                Urgency::Later,
                Some(item),
                age,
            ));
        }
    }
    let live_projects = items
        .iter()
        .filter(|item| item.column != Column::Closed)
        .filter_map(|item| item.project_id.as_deref())
        .collect::<HashSet<_>>();
    for (project_id, snapshot) in sorted(&inputs.snapshots) {
        if !live_projects.contains(project_id.as_str()) {
            continue;
        }
        let age = inputs
            .projects
            .iter()
            .find(|project| &project.id == project_id)
            .map_or(0, |project| {
                inputs.now_ms.saturating_sub(project.updated_at_unix_ms)
            });
        for line in &snapshot.lines {
            let urgency = if line.checkbox {
                Urgency::Soon
            } else {
                Urgency::Later
            };
            out.push(Todo {
                id: todos::todo_id(&line.text),
                text: line.text.clone(),
                kind: TodoKind::Snapshot,
                urgency,
                project_id: Some(project_id.clone()),
                sources: vec![
                    TodoSource {
                        kind: "project".to_owned(),
                        id: project_id.clone(),
                    },
                    TodoSource {
                        kind: "node".to_owned(),
                        id: snapshot.node_id.clone(),
                    },
                ],
                age_ms: age,
            });
        }
    }
    out.extend(model_todos(items));
    let mut out = todos::dedupe(out);
    out.sort_by(|left, right| {
        left.urgency
            .cmp(&right.urgency)
            .then_with(|| right.age_ms.cmp(&left.age_ms))
    });
    out
}

/// Model-suggested next actions (cited and validated earlier) of each item:
/// one row per agent naming it, like the agent's own steps.
fn model_todos(items: &[Item]) -> Vec<Todo> {
    items
        .iter()
        .filter(|item| item.column != Column::Closed)
        .filter_map(|item| {
            let ai = item.ai.as_ref()?;
            let text = plain::steps_todo(
                &plain::who(item),
                &ai.next_actions,
                item.column == Column::Stale,
            )?;
            Some(todo(
                text,
                TodoKind::Model,
                Urgency::Soon,
                Some(item),
                item.age_ms,
            ))
        })
        .collect()
}

fn sorted<V>(map: &HashMap<String, V>) -> Vec<(&String, &V)> {
    let mut entries = map.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    entries
}

/// Marks for the next generation's age correction.
pub(crate) fn marks(items: &[Item]) -> HashMap<String, Mark> {
    items
        .iter()
        .map(|item| (item.id.clone(), item.mark))
        .collect()
}

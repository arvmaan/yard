//! Answer a blocked agent from Slack: opaque single-use action ids and the
//! guarded unblock flow.
//!
//! A button carries only a random 128-bit id. The id maps, server-side and
//! for [`ACTION_TTL`], to {agent, terminal identity, prompt fingerprint,
//! option}. Using any button of a card consumes the whole card. Before a
//! key is typed, [`unblock`] re-checks, in order: the agent still exists
//! with the same worker and terminal identity (terminal id, provider
//! session, pane, tab) and is still `blocked`; the pane, re-read now, still
//! shows a prompt with the SAME fingerprint; the option is still offered
//! (never a standing grant). Keys then go through the terminal lease path
//! ([`AgentConsole::type_keys`]), which re-validates the identity itself
//! and, once the lease is held, re-reads the pane against the [`KeyPlan`]
//! (same fingerprint and cursor; the cursor on the option before Enter).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use yard_domain::OrchestratorStatusReport;

use super::prompt::{KEY_ENTER, OptionRole, ParsedScreen, parse_screen};

/// Buttons work for this long after they are posted.
pub const ACTION_TTL: Duration = Duration::from_secs(5 * 60);
/// "Retry" and "Check again" on a question's error and timeout cards work
/// this long (still single-use): they are posted after a long wait, and
/// "Check again" is meant for the whole late window.
pub const ASK_ACTION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// "Answer" on a digest works this long (still single-use): a digest is
/// read later, and the button only re-reads the agent and posts a freshly
/// guarded prompt card.
pub const DIGEST_ACTION_TTL: Duration = Duration::from_secs(9 * 60 * 60);
/// A free-text reply in a question's thread is accepted this long.
pub const QUESTION_TTL: Duration = Duration::from_secs(30 * 60);
/// Used and expired ids are remembered this long to explain a late click.
const TOMBSTONE_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_CARDS: usize = 256;

/// A Yard agent Slack can answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentTarget {
    Assignment {
        project_id: String,
        assignment_id: String,
    },
    ProjectOrchestrator {
        project_id: String,
    },
    YardOrchestrator,
    CoordinationNode {
        node_id: String,
    },
}

/// The terminal an answer is meant for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalIdentity {
    pub worker_id: String,
    pub terminal_id: String,
    pub pane_id: String,
    pub tab_id: Option<String>,
    /// `provider:kind:value` of the agent's provider session.
    pub provider_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSnapshot {
    pub identity: TerminalIdentity,
    /// Herdr reports the agent `blocked` (observed, process not exited).
    pub blocked: bool,
    /// Whether an agent runs in the bound pane (see [`AgentHealth`]).
    pub health: AgentHealth,
}

/// What the durable binding says about the agent in its pane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentHealth {
    Working,
    Idle,
    /// Observed state unknown or ambiguous; treated as reachable.
    #[default]
    Unknown,
    /// No agent runs there; the reason is shown to the owner.
    NotRunning(String),
}

impl AgentHealth {
    /// A short word for progress messages.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Unknown => "thinking",
            Self::NotRunning(_) => "not running",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsoleError {
    /// The agent, assignment or binding is gone or no longer active.
    #[error("{0}")]
    Gone(String),
    /// The durable binding or the lease no longer matches.
    #[error("{0}")]
    IdentityChanged(String),
    /// Terminal control is held elsewhere (open in Yard).
    #[error("{0}")]
    Busy(String),
    /// The pane, re-read while holding the lease, no longer matches the
    /// prompt (or cursor) the keys were computed for.
    #[error("{0}")]
    PromptChanged(String),
    /// Yard itself must not type into this pane (a managed pane whose
    /// lease needs recovery, or an isolated summary worker). The message is
    /// shown to the owner as is.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
}

/// Keys for one option and what the pane must still show when they are
/// typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPlan {
    pub fingerprint: String,
    pub option_index: usize,
    pub role: OptionRole,
    /// Arrows from the cursor seen when planning, then Enter.
    pub keys: Vec<&'static str>,
}

impl KeyPlan {
    fn changed(what: &str) -> ConsoleError {
        ConsoleError::PromptChanged(what.to_owned())
    }

    /// Re-check a snapshot and screen read after the lease was taken: still
    /// blocked, same prompt, same option and the same keys from the cursor
    /// shown now.
    ///
    /// # Errors
    ///
    /// [`ConsoleError::PromptChanged`] when anything differs.
    pub fn check_start(&self, blocked: bool, screen: &str) -> Result<(), ConsoleError> {
        if !blocked {
            return Err(Self::changed("the agent is no longer blocked"));
        }
        let ParsedScreen::Choice(choice) = parse_screen(screen) else {
            return Err(Self::changed("the prompt is gone"));
        };
        let same = choice.fingerprint == self.fingerprint
            && choice
                .offered(self.option_index)
                .is_some_and(|option| option.role == self.role)
            && choice.keys_for(self.option_index).as_ref() == Some(&self.keys);
        if same {
            Ok(())
        } else {
            Err(Self::changed("the prompt or its cursor changed"))
        }
    }

    /// Before Enter: the same prompt with the cursor on the option.
    ///
    /// # Errors
    ///
    /// [`ConsoleError::PromptChanged`] otherwise.
    pub fn check_before_enter(&self, screen: &str) -> Result<(), ConsoleError> {
        match parse_screen(screen) {
            ParsedScreen::Choice(choice)
                if choice.fingerprint == self.fingerprint
                    && choice.cursor == self.option_index
                    && choice.offered(self.option_index).is_some() =>
            {
                Ok(())
            }
            _ => Err(Self::changed("the cursor is not on the option")),
        }
    }

    /// Whether key `index` is the final Enter after arrows.
    #[must_use]
    pub fn is_confirming(&self, index: usize) -> bool {
        index > 0 && index + 1 == self.keys.len() && self.keys[index] == KEY_ENTER
    }
}

/// Yard's guarded paths to an agent's terminal (production:
/// [`super::console::YardConsole`]).
#[async_trait]
pub trait AgentConsole: Send + Sync {
    /// Durable identity and blocked state.
    async fn snapshot(&self, target: &AgentTarget) -> Result<AgentSnapshot, ConsoleError>;
    /// Whether Yard would accept input for this agent now: the same
    /// refusals as [`Self::send_prompt`] (an isolated summary worker, or a
    /// managed pane whose lease needs recovery), checked before asking.
    async fn sendable(&self, _target: &AgentTarget) -> Result<(), ConsoleError> {
        Ok(())
    }
    /// The ANSI-stripped tail of the pane, read now.
    async fn screen(&self, target: &AgentTarget) -> Result<String, ConsoleError>;
    /// Take the terminal lease, require `identity`, re-read the agent and
    /// its pane against `plan` ([`KeyPlan::check_start`], and
    /// [`KeyPlan::check_before_enter`] before a final Enter), type each key
    /// as its own write, release.
    async fn type_keys(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        plan: &KeyPlan,
    ) -> Result<(), ConsoleError>;
    /// Deliver owner-written text through the guarded prompt path; returns
    /// the prompt command's id.
    async fn send_prompt(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        text: &str,
        actor: &str,
    ) -> Result<String, ConsoleError>;
    /// An orchestrator's status report for `command_id`, when its pane
    /// shows one now (read through the guarded output path). Workers have
    /// no status protocol: `Ok(None)`.
    async fn status_report(
        &self,
        target: &AgentTarget,
        command_id: &str,
    ) -> Result<Option<OrchestratorStatusReport>, ConsoleError>;
    /// One look while waiting for `command_id`'s answer: its report and
    /// whether the agent UI lost its session, from one guarded read.
    async fn watch(
        &self,
        target: &AgentTarget,
        command_id: &str,
    ) -> Result<crate::slack::relay::Watched, ConsoleError> {
        Ok(crate::slack::relay::Watched {
            report: self.status_report(target, command_id).await?,
            session_lost: None,
        })
    }
}

/// One option button, as registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub target: AgentTarget,
    pub identity: TerminalIdentity,
    pub fingerprint: String,
    /// On-screen option index.
    pub option_index: usize,
    pub role: OptionRole,
    /// Raw label (redact before display).
    pub label: String,
    /// Display name of the agent (already cleaned).
    pub title: String,
    card: u64,
    expires_at: Instant,
}

/// A question waiting for a free-text reply in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuestion {
    pub target: AgentTarget,
    pub identity: TerminalIdentity,
    pub fingerprint: String,
    /// Display name of the agent (already cleaned).
    pub title: String,
    expires_at: Instant,
}

/// A navigation button (help, status and project cards): it re-runs a
/// read-only command, or posts an agent's current prompt card through the
/// guarded prompt path. Like answer buttons it is an opaque, single-use id
/// that expires after [`ACTION_TTL`]; Slack never supplies the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavAction {
    Status,
    Blocked,
    Review,
    ProjectStatus {
        project_id: String,
    },
    /// Post what this agent shows now (re-read; buttons only for a
    /// blocked prompt).
    Answer {
        target: AgentTarget,
        /// Display name (already cleaned).
        title: String,
    },
    /// How to ask the Superintendent.
    AskHint,
    /// Send the same question again as a new command. The text never
    /// leaves Yard: Slack only returns this id.
    Retry {
        target: AgentTarget,
        /// "the Superintendent" / "the X orchestrator" (already cleaned).
        to: String,
        question: String,
    },
    /// Re-read the latest status report for `command_id` and relay it if
    /// it now answers that question.
    CheckAgain {
        target: AgentTarget,
        to: String,
        command_id: String,
        /// The question's progress message (updated once answered).
        progress_ts: Option<String>,
        asked_at: Instant,
    },
    /// A quiet-hours control from the settings card (mute, unmute,
    /// digest now).
    Quiet(super::controls::Control),
}

impl NavAction {
    /// The audit name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Status => "nav_status",
            Self::Blocked => "nav_blocked",
            Self::Review => "nav_review",
            Self::ProjectStatus { .. } => "nav_status_project",
            Self::Answer { .. } => "nav_answer",
            Self::AskHint => "nav_ask_hint",
            Self::Retry { .. } => "nav_retry",
            Self::CheckAgain { .. } => "nav_check_again",
            Self::Quiet(control) => match control {
                super::controls::Control::Settings => "nav_settings",
                super::controls::Control::Mute(_) => "nav_mute",
                super::controls::Control::Unmute => "nav_unmute",
                super::controls::Control::Digest => "nav_digest",
            },
        }
    }

    /// How long this button works after it is posted.
    #[must_use]
    pub const fn ttl(&self) -> Duration {
        match self {
            Self::Retry { .. } | Self::CheckAgain { .. } => ASK_ACTION_TTL,
            _ => ACTION_TTL,
        }
    }
}

/// Why a navigation button did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavError {
    Expired,
    AlreadyUsed,
}

#[derive(Debug, Clone)]
struct PendingNav {
    action: NavAction,
    expires_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeError {
    Unknown,
    Expired(AgentTarget),
    AlreadyUsed(AgentTarget),
}

#[derive(Debug, Clone)]
struct Tombstone {
    target: AgentTarget,
    expired: bool,
    at: Instant,
}

/// A card to register: the prompt as parsed and the buttons offered.
#[derive(Debug, Clone)]
pub struct CardSpec<'a> {
    pub target: &'a AgentTarget,
    pub identity: &'a TerminalIdentity,
    pub fingerprint: &'a str,
    /// Display name of the agent (already cleaned).
    pub title: &'a str,
    /// `(on-screen index, role, raw label)` per button.
    pub options: Vec<(usize, OptionRole, String)>,
}

/// Opaque action ids → pending answers. Single-use; a card's buttons are
/// consumed together.
#[derive(Debug, Default)]
pub struct ActionRegistry {
    actions: HashMap<String, PendingAction>,
    /// Thread root ts → question awaiting a free-text reply.
    questions: HashMap<String, PendingQuestion>,
    /// Question threads answered or expired, for [`TOMBSTONE_TTL`].
    closed_questions: HashMap<String, Instant>,
    tombstones: HashMap<String, Tombstone>,
    navs: HashMap<String, PendingNav>,
    /// Used (`false`) or expired (`true`) navigation ids.
    nav_tombstones: HashMap<String, (bool, Instant)>,
    next_card: u64,
}

/// 32 lowercase hex characters from the OS RNG.
///
/// # Errors
///
/// Returns an error when the OS RNG is unavailable (no ids are issued).
pub fn random_token() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(super::prompt::hex(&bytes))
}

impl ActionRegistry {
    fn prune(&mut self, now: Instant) {
        let expired = self
            .actions
            .iter()
            .filter(|(_, action)| now >= action.expires_at)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(action) = self.actions.remove(&id) {
                self.tombstones.insert(
                    id,
                    Tombstone {
                        target: action.target,
                        expired: true,
                        at: now,
                    },
                );
            }
        }
        let expired = self
            .questions
            .iter()
            .filter(|(_, question)| now >= question.expires_at)
            .map(|(root, _)| root.clone())
            .collect::<Vec<_>>();
        for root in expired {
            self.questions.remove(&root);
            self.closed_questions.insert(root, now);
        }
        self.closed_questions
            .retain(|_, at| now.saturating_duration_since(*at) < TOMBSTONE_TTL);
        while self.closed_questions.len() > MAX_CARDS {
            let Some(oldest) = self
                .closed_questions
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(root, _)| root.clone())
            else {
                break;
            };
            self.closed_questions.remove(&oldest);
        }
        self.tombstones
            .retain(|_, tombstone| now.saturating_duration_since(tombstone.at) < TOMBSTONE_TTL);
        let expired = self
            .navs
            .iter()
            .filter(|(_, nav)| now >= nav.expires_at)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.navs.remove(&id);
            self.nav_tombstones.insert(id, (true, now));
        }
        self.nav_tombstones
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < TOMBSTONE_TTL);
        while self.navs.len() > MAX_CARDS * 4 {
            let Some(oldest) = self
                .navs
                .iter()
                .min_by_key(|(_, nav)| nav.expires_at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.navs.remove(&oldest);
        }
        while self.nav_tombstones.len() > MAX_CARDS * 4 {
            let Some(oldest) = self
                .nav_tombstones
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.nav_tombstones.remove(&oldest);
        }
        // Bound memory: drop the oldest cards first.
        while self.actions.len() > MAX_CARDS * 4 {
            let Some(oldest) = self.actions.values().map(|action| action.card).min() else {
                break;
            };
            self.actions.retain(|_, action| action.card != oldest);
        }
    }

    /// Register one card's buttons; returns one id per option, in order.
    ///
    /// # Errors
    ///
    /// Returns an error when the OS RNG fails; nothing is registered.
    pub fn issue(&mut self, card: &CardSpec<'_>, now: Instant) -> Result<Vec<String>, String> {
        self.prune(now);
        self.next_card = self.next_card.wrapping_add(1);
        let ids = card
            .options
            .iter()
            .map(|_| random_token())
            .collect::<Result<Vec<_>, _>>()?;
        for (id, (index, role, label)) in ids.iter().zip(&card.options) {
            self.actions.insert(
                id.clone(),
                PendingAction {
                    target: card.target.clone(),
                    identity: card.identity.clone(),
                    fingerprint: card.fingerprint.to_owned(),
                    option_index: *index,
                    role: *role,
                    label: label.clone(),
                    title: card.title.to_owned(),
                    card: self.next_card,
                    expires_at: now + ACTION_TTL,
                },
            );
        }
        Ok(ids)
    }

    /// Register one navigation button; returns its id.
    ///
    /// # Errors
    ///
    /// Returns an error when the OS RNG fails; nothing is registered.
    pub fn issue_nav(&mut self, action: NavAction, now: Instant) -> Result<String, String> {
        let ttl = action.ttl();
        self.issue_nav_for(action, now, ttl)
    }

    /// [`Self::issue_nav`] with an explicit lifetime (a digest's buttons).
    ///
    /// # Errors
    ///
    /// When no random id can be made.
    pub fn issue_nav_for(
        &mut self,
        action: NavAction,
        now: Instant,
        ttl: Duration,
    ) -> Result<String, String> {
        self.prune(now);
        let id = random_token()?;
        let expires_at = now + ttl;
        self.navs
            .insert(id.clone(), PendingNav { action, expires_at });
        Ok(id)
    }

    /// Consume a navigation id (only that button: the rest of its card
    /// stays usable). `None` when `id` is not a navigation id at all.
    pub fn take_nav(&mut self, id: &str, now: Instant) -> Option<Result<NavAction, NavError>> {
        self.prune(now);
        if let Some(nav) = self.navs.remove(id) {
            self.nav_tombstones.insert(id.to_owned(), (false, now));
            return Some(Ok(nav.action));
        }
        self.nav_tombstones.get(id).map(|(expired, _)| {
            Err(if *expired {
                NavError::Expired
            } else {
                NavError::AlreadyUsed
            })
        })
    }

    /// Consume `id` and every sibling on its card.
    ///
    /// # Errors
    ///
    /// [`TakeError`] for unknown, expired or already used ids.
    pub fn take(&mut self, id: &str, now: Instant) -> Result<PendingAction, TakeError> {
        self.prune(now);
        let Some(action) = self.actions.remove(id) else {
            return Err(match self.tombstones.get(id) {
                Some(tombstone) if tombstone.expired => {
                    TakeError::Expired(tombstone.target.clone())
                }
                Some(tombstone) => TakeError::AlreadyUsed(tombstone.target.clone()),
                None => TakeError::Unknown,
            });
        };
        let siblings = self
            .actions
            .iter()
            .filter(|(_, other)| other.card == action.card)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for used in siblings.into_iter().chain([id.to_owned()]) {
            self.actions.remove(&used);
            self.tombstones.insert(
                used,
                Tombstone {
                    target: action.target.clone(),
                    expired: false,
                    at: now,
                },
            );
        }
        Ok(action)
    }

    /// Expect a free-text reply in the thread rooted at `thread_ts`
    /// (replaces an earlier question there).
    pub fn expect_reply(
        &mut self,
        thread_ts: &str,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        fingerprint: &str,
        title: &str,
        now: Instant,
    ) {
        self.prune(now);
        if self.questions.len() >= MAX_CARDS
            && let Some(oldest) = self
                .questions
                .iter()
                .min_by_key(|(_, question)| question.expires_at)
                .map(|(ts, _)| ts.clone())
        {
            self.questions.remove(&oldest);
        }
        self.questions.insert(
            thread_ts.to_owned(),
            PendingQuestion {
                target: target.clone(),
                identity: identity.clone(),
                fingerprint: fingerprint.to_owned(),
                title: title.to_owned(),
                expires_at: now + QUESTION_TTL,
            },
        );
    }

    /// Consume the question awaiting a reply in `thread_ts`.
    pub fn take_question(&mut self, thread_ts: &str, now: Instant) -> Option<PendingQuestion> {
        self.prune(now);
        let question = self.questions.remove(thread_ts)?;
        self.closed_questions.insert(thread_ts.to_owned(), now);
        Some(question)
    }

    /// Whether `thread_ts` is a question card already answered or expired
    /// (a later message there is not a new question for anyone).
    pub fn question_closed(&mut self, thread_ts: &str, now: Instant) -> bool {
        self.prune(now);
        self.closed_questions.contains_key(thread_ts)
    }
}

/// Why nothing was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Unknown,
    Expired,
    AlreadyUsed,
    /// The agent or its assignment is gone or no longer active.
    Gone(String),
    /// Different worker, terminal, pane, tab or provider session.
    TerminalChanged,
    NoLongerBlocked,
    /// The screen shows a different prompt (or none).
    PromptChanged,
    /// The option is no longer offered (or never was).
    NotOffered,
    /// The terminal is being controlled elsewhere (open in Yard).
    Busy(String),
    /// Yard refuses to type into this pane; the reason is shown as is.
    Unavailable(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Keys were typed / the reply was delivered.
    Sent,
    Refused {
        refusal: Refusal,
        /// What the agent shows now, when it is still the same terminal.
        current: Option<ParsedScreen>,
    },
}

/// Prefix on every owner reply delivered from Slack.
pub const SLACK_PROVENANCE: &str = "From Slack (owner):";

impl From<TakeError> for Refusal {
    fn from(error: TakeError) -> Self {
        match error {
            TakeError::Unknown => Self::Unknown,
            TakeError::Expired(_) => Self::Expired,
            TakeError::AlreadyUsed(_) => Self::AlreadyUsed,
        }
    }
}

fn refused(refusal: Refusal) -> Outcome {
    Outcome::Refused {
        refusal,
        current: None,
    }
}

pub(crate) fn console_refusal(error: ConsoleError) -> Refusal {
    match error {
        ConsoleError::Gone(message) => Refusal::Gone(message),
        ConsoleError::IdentityChanged(_) => Refusal::TerminalChanged,
        ConsoleError::Busy(message) => Refusal::Busy(message),
        ConsoleError::PromptChanged(_) => Refusal::PromptChanged,
        ConsoleError::Unavailable(message) => Refusal::Unavailable(message),
        ConsoleError::Failed(message) => Refusal::Failed(message),
    }
}

/// Same worker and terminal, per the durable binding, read now.
async fn same_terminal(
    console: &dyn AgentConsole,
    target: &AgentTarget,
    identity: &TerminalIdentity,
) -> Result<AgentSnapshot, Refusal> {
    let snapshot = console.snapshot(target).await.map_err(console_refusal)?;
    if snapshot.identity != *identity {
        return Err(Refusal::TerminalChanged);
    }
    Ok(snapshot)
}

/// Re-check everything, then type the option's keys.
pub async fn unblock(console: &dyn AgentConsole, action: &PendingAction) -> Outcome {
    let snapshot = match same_terminal(console, &action.target, &action.identity).await {
        Ok(snapshot) => snapshot,
        Err(refusal) => return refused(refusal),
    };
    let screen = match console.screen(&action.target).await {
        Ok(screen) => parse_screen(&screen),
        Err(error) => return refused(console_refusal(error)),
    };
    if !snapshot.blocked {
        return Outcome::Refused {
            refusal: Refusal::NoLongerBlocked,
            current: Some(screen),
        };
    }
    let ParsedScreen::Choice(choice) = &screen else {
        return Outcome::Refused {
            refusal: Refusal::PromptChanged,
            current: Some(screen),
        };
    };
    if choice.fingerprint != action.fingerprint {
        return Outcome::Refused {
            refusal: Refusal::PromptChanged,
            current: Some(screen),
        };
    }
    let keys = choice
        .offered(action.option_index)
        .filter(|option| option.role == action.role)
        .and_then(|_| choice.keys_for(action.option_index));
    let Some(keys) = keys else {
        return refused(Refusal::NotOffered);
    };
    let plan = KeyPlan {
        fingerprint: action.fingerprint.clone(),
        option_index: action.option_index,
        role: action.role,
        keys,
    };
    match console
        .type_keys(&action.target, &action.identity, &plan)
        .await
    {
        Ok(()) => Outcome::Sent,
        Err(error) => refused(console_refusal(error)),
    }
}

/// Deliver the owner's free-text reply to the question it answers, marked
/// as coming from Slack, through the guarded prompt path.
pub async fn reply(
    console: &dyn AgentConsole,
    question: &PendingQuestion,
    text: &str,
    actor: &str,
) -> Outcome {
    if let Err(refusal) = same_terminal(console, &question.target, &question.identity).await {
        return refused(refusal);
    }
    let screen = match console.screen(&question.target).await {
        Ok(screen) => parse_screen(&screen),
        Err(error) => return refused(console_refusal(error)),
    };
    match &screen {
        ParsedScreen::Question { fingerprint, .. } if *fingerprint == question.fingerprint => {}
        _ => {
            return Outcome::Refused {
                refusal: Refusal::PromptChanged,
                current: Some(screen),
            };
        }
    }
    let text = format!("{SLACK_PROVENANCE} {}", text.trim());
    match console
        .send_prompt(&question.target, &question.identity, &text, actor)
        .await
    {
        Ok(_) => Outcome::Sent,
        Err(error) => refused(console_refusal(error)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use yard_domain::OrchestratorStatusReport;

    use super::{
        ACTION_TTL, ActionRegistry, AgentConsole, AgentSnapshot, AgentTarget, CardSpec,
        ConsoleError, Outcome, Refusal, SLACK_PROVENANCE, TakeError, TerminalIdentity, reply,
        unblock,
    };
    use crate::slack::prompt::{
        KEY_DOWN, KEY_ENTER, OptionRole, ParsedScreen, parse_screen,
        tests::{CLAUDE_MENU, CLAUDE_PERMISSION, CLAUDE_QUESTION},
    };

    /// A scripted agent terminal that records what was typed or prompted.
    pub(crate) struct FakeConsole {
        pub snapshot: Mutex<Result<AgentSnapshot, ConsoleError>>,
        pub screen: Mutex<String>,
        pub typed: Mutex<Vec<Vec<&'static str>>>,
        pub prompts: Mutex<Vec<(String, String)>>,
        pub type_error: Mutex<Option<ConsoleError>>,
        pub prompt_error: Mutex<Option<ConsoleError>>,
        /// Command ids whose status report was looked for.
        pub report_reads: Mutex<Vec<String>>,
        pub prompt_targets: Mutex<Vec<AgentTarget>>,
        /// Returned by `watch` while set.
        pub watch_error: Mutex<Option<ConsoleError>>,
        /// Returned by `sendable` while set.
        pub send_refusal: Mutex<Option<ConsoleError>>,
    }

    pub(crate) fn identity() -> TerminalIdentity {
        TerminalIdentity {
            worker_id: "worker-1".to_owned(),
            terminal_id: "term-1".to_owned(),
            pane_id: "w1:p3".to_owned(),
            tab_id: Some("w1:t1".to_owned()),
            provider_session: Some("claude:session:abc".to_owned()),
        }
    }

    pub(crate) fn target() -> AgentTarget {
        AgentTarget::Assignment {
            project_id: "project-1".to_owned(),
            assignment_id: "assignment-1".to_owned(),
        }
    }

    impl FakeConsole {
        pub(crate) fn blocked_on(screen: &str) -> Self {
            Self {
                snapshot: Mutex::new(Ok(AgentSnapshot {
                    identity: identity(),
                    blocked: true,
                    health: super::AgentHealth::Working,
                })),
                screen: Mutex::new(screen.to_owned()),
                typed: Mutex::default(),
                prompts: Mutex::default(),
                type_error: Mutex::default(),
                prompt_error: Mutex::default(),
                report_reads: Mutex::default(),
                prompt_targets: Mutex::default(),
                watch_error: Mutex::default(),
                send_refusal: Mutex::default(),
            }
        }

        fn typed(&self) -> Vec<Vec<&'static str>> {
            self.typed.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentConsole for FakeConsole {
        async fn snapshot(&self, _: &AgentTarget) -> Result<AgentSnapshot, ConsoleError> {
            self.snapshot.lock().unwrap().clone()
        }

        async fn sendable(&self, _: &AgentTarget) -> Result<(), ConsoleError> {
            self.send_refusal
                .lock()
                .unwrap()
                .clone()
                .map_or(Ok(()), Err)
        }

        async fn screen(&self, _: &AgentTarget) -> Result<String, ConsoleError> {
            Ok(self.screen.lock().unwrap().clone())
        }

        async fn type_keys(
            &self,
            _: &AgentTarget,
            identity: &TerminalIdentity,
            plan: &super::KeyPlan,
        ) -> Result<(), ConsoleError> {
            assert_eq!(*identity, super::tests::identity());
            if let Some(error) = self.type_error.lock().unwrap().clone() {
                return Err(error);
            }
            self.typed.lock().unwrap().push(plan.keys.clone());
            Ok(())
        }

        async fn send_prompt(
            &self,
            target: &AgentTarget,
            _: &TerminalIdentity,
            text: &str,
            actor: &str,
        ) -> Result<String, ConsoleError> {
            if let Some(error) = self.prompt_error.lock().unwrap().clone() {
                return Err(error);
            }
            self.prompt_targets.lock().unwrap().push(target.clone());
            let mut prompts = self.prompts.lock().unwrap();
            prompts.push((text.to_owned(), actor.to_owned()));
            Ok(format!("command-{}", prompts.len()))
        }

        async fn status_report(
            &self,
            _: &AgentTarget,
            command_id: &str,
        ) -> Result<Option<OrchestratorStatusReport>, ConsoleError> {
            self.report_reads
                .lock()
                .unwrap()
                .push(command_id.to_owned());
            Ok(crate::slack::relay::scan_report(
                &self.screen.lock().unwrap(),
                command_id,
            ))
        }

        async fn watch(
            &self,
            _: &AgentTarget,
            command_id: &str,
        ) -> Result<crate::slack::relay::Watched, ConsoleError> {
            if let Some(error) = self.watch_error.lock().unwrap().clone() {
                return Err(error);
            }
            self.report_reads
                .lock()
                .unwrap()
                .push(command_id.to_owned());
            Ok(crate::slack::relay::watched(
                &self.screen.lock().unwrap(),
                command_id,
            ))
        }
    }

    /// Register the offered buttons of `screen`; `(ids, fingerprint)`.
    pub(crate) fn register(
        registry: &mut ActionRegistry,
        screen: &str,
        now: Instant,
    ) -> (Vec<String>, String) {
        let ParsedScreen::Choice(choice) = parse_screen(screen) else {
            panic!("choice prompt");
        };
        let target = target();
        let identity = identity();
        let spec = CardSpec {
            target: &target,
            identity: &identity,
            fingerprint: &choice.fingerprint,
            title: "Worker",
            options: choice
                .options
                .iter()
                .map(|option| (option.index, option.role, option.label.clone()))
                .collect(),
        };
        (registry.issue(&spec, now).unwrap(), choice.fingerprint)
    }

    #[test]
    fn navigation_ids_are_opaque_single_use_and_expire_per_button() {
        let mut registry = ActionRegistry::default();
        let now = Instant::now();
        let status = registry.issue_nav(super::NavAction::Status, now).unwrap();
        let review = registry.issue_nav(super::NavAction::Review, now).unwrap();
        assert!(crate::slack::inbound::is_action_token(&status));
        assert_ne!(status, review);
        assert_eq!(registry.take_nav("f".repeat(32).as_str(), now), None);
        assert_eq!(
            registry.take_nav(&status, now),
            Some(Ok(super::NavAction::Status))
        );
        assert_eq!(
            registry.take_nav(&status, now),
            Some(Err(super::NavError::AlreadyUsed))
        );
        // Its sibling on the same card still works until it expires.
        let later = now + super::ACTION_TTL;
        assert_eq!(
            registry.take_nav(&review, later),
            Some(Err(super::NavError::Expired))
        );
        // A navigation id is never an answer id.
        let fresh = registry
            .issue_nav(super::NavAction::Blocked, later)
            .unwrap();
        assert_eq!(registry.take(&fresh, later), Err(super::TakeError::Unknown));
    }

    #[test]
    fn quiet_control_buttons_are_single_use_and_expire_like_navigation() {
        use crate::slack::controls::Control;
        let mut registry = ActionRegistry::default();
        let now = Instant::now();
        let mute = registry
            .issue_nav(super::NavAction::Quiet(Control::Unmute), now)
            .unwrap();
        let digest = registry
            .issue_nav(super::NavAction::Quiet(Control::Digest), now)
            .unwrap();
        assert_eq!(
            registry.take_nav(&mute, now),
            Some(Ok(super::NavAction::Quiet(Control::Unmute)))
        );
        assert_eq!(
            registry.take_nav(&mute, now),
            Some(Err(super::NavError::AlreadyUsed))
        );
        assert_eq!(
            registry.take_nav(&digest, now + super::ACTION_TTL),
            Some(Err(super::NavError::Expired))
        );
        assert_eq!(
            super::NavAction::Quiet(Control::Digest).name(),
            "nav_digest"
        );
    }

    #[test]
    fn question_retry_and_check_again_buttons_outlive_the_short_ttl() {
        let mut registry = ActionRegistry::default();
        let now = Instant::now();
        let check = super::NavAction::CheckAgain {
            target: target(),
            to: "the Superintendent".to_owned(),
            command_id: "command-1".to_owned(),
            progress_ts: None,
            asked_at: now,
        };
        let retry = super::NavAction::Retry {
            target: target(),
            to: "the Superintendent".to_owned(),
            question: "why?".to_owned(),
        };
        let ids = [check.clone(), retry.clone(), super::NavAction::Status]
            .map(|nav| registry.issue_nav(nav, now).unwrap());
        // 20 minutes after a timeout card: Check again and Retry still work.
        let later = now + Duration::from_secs(20 * 60);
        assert_eq!(registry.take_nav(&ids[0], later), Some(Ok(check)));
        assert_eq!(registry.take_nav(&ids[1], later), Some(Ok(retry)));
        assert_eq!(
            registry.take_nav(&ids[2], later),
            Some(Err(super::NavError::Expired))
        );
        // Still single-use, and they do expire.
        assert_eq!(
            registry.take_nav(&ids[0], later),
            Some(Err(super::NavError::AlreadyUsed))
        );
        let old = registry
            .issue_nav(
                super::NavAction::Retry {
                    target: target(),
                    to: "x".to_owned(),
                    question: "y".to_owned(),
                },
                now,
            )
            .unwrap();
        assert_eq!(
            registry.take_nav(&old, now + super::ASK_ACTION_TTL),
            Some(Err(super::NavError::Expired))
        );
    }

    #[test]
    fn a_digests_answer_button_works_for_the_working_day_once() {
        let mut registry = ActionRegistry::default();
        let now = Instant::now();
        let answer = super::NavAction::Answer {
            target: target(),
            title: "Checkout · alpha".to_owned(),
        };
        let digest = registry
            .issue_nav_for(answer.clone(), now, super::DIGEST_ACTION_TTL)
            .unwrap();
        let card = registry.issue_nav(answer.clone(), now).unwrap();
        // 20 minutes after the morning digest: its Answer still works; a
        // notification card's (short TTL) does not.
        let later = now + Duration::from_secs(20 * 60);
        assert_eq!(registry.take_nav(&digest, later), Some(Ok(answer.clone())));
        assert_eq!(
            registry.take_nav(&card, later),
            Some(Err(super::NavError::Expired))
        );
        assert_eq!(
            registry.take_nav(&digest, later),
            Some(Err(super::NavError::AlreadyUsed))
        );
        let old = registry
            .issue_nav_for(answer, now, super::DIGEST_ACTION_TTL)
            .unwrap();
        assert_eq!(
            registry.take_nav(&old, now + super::DIGEST_ACTION_TTL),
            Some(Err(super::NavError::Expired))
        );
    }

    #[test]
    fn action_ids_are_opaque_single_use_and_expire() {
        let mut registry = ActionRegistry::default();
        let now = Instant::now();
        let (ids, _) = register(&mut registry, CLAUDE_PERMISSION, now);
        assert_eq!(ids.len(), 2);
        assert!(
            ids.iter()
                .all(|id| crate::slack::inbound::is_action_token(id))
        );
        assert_ne!(ids[0], ids[1]);
        let allow = registry.take(&ids[0], now).unwrap();
        assert_eq!((allow.option_index, allow.role), (0, OptionRole::Allow));
        assert_eq!(
            registry.take(&ids[0], now),
            Err(TakeError::AlreadyUsed(target()))
        );
        assert_eq!(
            registry.take(&ids[1], now),
            Err(TakeError::AlreadyUsed(target())),
            "one answer per card"
        );
        let (ids, _) = register(&mut registry, CLAUDE_PERMISSION, now);
        let later = now + ACTION_TTL + Duration::from_secs(1);
        assert_eq!(
            registry.take(&ids[1], later),
            Err(TakeError::Expired(target()))
        );
        assert_eq!(
            registry.take("0123456789abcdef0123456789abcdef", now),
            Err(TakeError::Unknown)
        );
    }

    fn take(registry: &mut ActionRegistry, id: &str) -> super::PendingAction {
        registry.take(id, Instant::now()).unwrap()
    }

    #[tokio::test]
    async fn a_valid_click_types_the_keys_for_that_option_only() {
        let console = FakeConsole::blocked_on(CLAUDE_PERMISSION);
        let mut registry = ActionRegistry::default();
        let (ids, _) = register(&mut registry, CLAUDE_PERMISSION, Instant::now());
        let deny = take(&mut registry, &ids[1]);
        assert_eq!(unblock(&console, &deny).await, Outcome::Sent);
        assert_eq!(console.typed(), [vec![KEY_DOWN, KEY_DOWN, KEY_ENTER]]);
        // Menu choice: the cursor moved in Yard since the card was posted;
        // keys follow the cursor as it is now.
        let console = FakeConsole::blocked_on(CLAUDE_MENU);
        let (ids, _) = register(&mut registry, CLAUDE_MENU, Instant::now());
        *console.screen.lock().unwrap() = CLAUDE_MENU
            .replace("❯ 1. Yard server", "  1. Yard server")
            .replace("  2. Herdr plugin", "❯ 2. Herdr plugin");
        let herdr = take(&mut registry, &ids[1]);
        assert_eq!(unblock(&console, &herdr).await, Outcome::Sent);
        assert_eq!(console.typed(), [vec![KEY_ENTER]]);
    }

    #[tokio::test]
    async fn refuses_when_anything_changed_and_types_nothing() {
        let mut registry = ActionRegistry::default();
        let mut cases: Vec<(FakeConsole, Refusal, bool)> = Vec::new();
        // The prompt on screen is a different one.
        let console = FakeConsole::blocked_on(CLAUDE_MENU);
        cases.push((console, Refusal::PromptChanged, true));
        // Same prompt text, but a different option list.
        let console =
            FakeConsole::blocked_on(&CLAUDE_PERMISSION.replace("3. No, and tell", "3. No, tell"));
        cases.push((console, Refusal::PromptChanged, true));
        // The agent answered already (prompt gone, not blocked).
        let console = FakeConsole::blocked_on(crate::slack::prompt::tests::WORKING);
        console.snapshot.lock().unwrap().as_mut().unwrap().blocked = false;
        cases.push((console, Refusal::NoLongerBlocked, true));
        // Different pane, tab, terminal, worker or provider session.
        for change in 0..5 {
            let console = FakeConsole::blocked_on(CLAUDE_PERMISSION);
            {
                let mut snapshot = console.snapshot.lock().unwrap();
                let identity = &mut snapshot.as_mut().unwrap().identity;
                match change {
                    0 => identity.pane_id = "w1:p4".to_owned(),
                    1 => identity.tab_id = None,
                    2 => identity.terminal_id = "term-2".to_owned(),
                    3 => identity.worker_id = "worker-2".to_owned(),
                    _ => identity.provider_session = Some("claude:session:new".to_owned()),
                }
            }
            cases.push((console, Refusal::TerminalChanged, false));
        }
        // The assignment ended.
        let console = FakeConsole::blocked_on(CLAUDE_PERMISSION);
        *console.snapshot.lock().unwrap() =
            Err(ConsoleError::Gone("assignment is not active".to_owned()));
        cases.push((
            console,
            Refusal::Gone("assignment is not active".to_owned()),
            false,
        ));
        // The lease check at typing time sees a rebinding.
        let console = FakeConsole::blocked_on(CLAUDE_PERMISSION);
        *console.type_error.lock().unwrap() =
            Some(ConsoleError::IdentityChanged("rebound".to_owned()));
        cases.push((console, Refusal::TerminalChanged, false));
        // The owner has the terminal open in Yard.
        let console = FakeConsole::blocked_on(CLAUDE_PERMISSION);
        *console.type_error.lock().unwrap() = Some(ConsoleError::Busy("control owned".to_owned()));
        cases.push((console, Refusal::Busy("control owned".to_owned()), false));
        for (console, expected, shows_current) in cases {
            let (ids, _) = register(&mut registry, CLAUDE_PERMISSION, Instant::now());
            let allow = take(&mut registry, &ids[0]);
            let Outcome::Refused { refusal, current } = unblock(&console, &allow).await else {
                panic!("{expected:?}: must refuse");
            };
            assert_eq!(refusal, expected);
            assert_eq!(current.is_some(), shows_current, "{expected:?}");
            assert!(console.typed().is_empty(), "{expected:?}: nothing typed");
        }
    }

    #[tokio::test]
    async fn standing_grants_are_never_sent_even_if_registered() {
        let console = FakeConsole::blocked_on(CLAUDE_PERMISSION);
        let mut registry = ActionRegistry::default();
        let (_, fingerprint) = register(&mut registry, CLAUDE_PERMISSION, Instant::now());
        let target = target();
        let identity = identity();
        // A forged registration of "Yes, and don't ask again" (index 1).
        for role in [OptionRole::Allow, OptionRole::Choice] {
            let ids = registry
                .issue(
                    &CardSpec {
                        target: &target,
                        identity: &identity,
                        fingerprint: &fingerprint,
                        title: "Worker",
                        options: vec![(1, role, "Yes, and don't ask again".to_owned())],
                    },
                    Instant::now(),
                )
                .unwrap();
            let forged = take(&mut registry, &ids[0]);
            assert_eq!(
                unblock(&console, &forged).await,
                Outcome::Refused {
                    refusal: Refusal::NotOffered,
                    current: None
                }
            );
        }
        assert!(console.typed().is_empty());
    }

    #[tokio::test]
    async fn free_text_replies_carry_provenance_and_recheck_the_question() {
        let console = FakeConsole::blocked_on(CLAUDE_QUESTION);
        console.snapshot.lock().unwrap().as_mut().unwrap().blocked = false;
        let ParsedScreen::Question { fingerprint, .. } = parse_screen(CLAUDE_QUESTION) else {
            panic!("question");
        };
        let mut registry = ActionRegistry::default();
        let now = Instant::now();
        registry.expect_reply(
            "1799999900.000100",
            &target(),
            &identity(),
            &fingerprint,
            "Worker",
            now,
        );
        assert!(!registry.question_closed("1799999900.000100", now));
        let question = registry.take_question("1799999900.000100", now).unwrap();
        assert!(
            registry.take_question("1799999900.000100", now).is_none(),
            "single use"
        );
        assert!(
            registry.question_closed("1799999900.000100", now),
            "a later message in its thread is not a new question"
        );
        assert!(!registry.question_closed("1799999999.000100", now));
        registry.expect_reply(
            "1799999901.000100",
            &target(),
            &identity(),
            &fingerprint,
            "W",
            now,
        );
        let later = now + super::QUESTION_TTL;
        assert!(registry.take_question("1799999901.000100", later).is_none());
        assert!(
            registry.question_closed("1799999901.000100", later),
            "expired"
        );
        assert!(
            !registry.question_closed("1799999901.000100", later + super::TOMBSTONE_TTL),
            "forgotten after an hour"
        );
        assert_eq!(
            reply(
                &console,
                &question,
                "  yes, delete it ",
                "slack:owner:U01OWNER"
            )
            .await,
            Outcome::Sent
        );
        assert_eq!(
            console.prompts.lock().unwrap().as_slice(),
            [(
                format!("{SLACK_PROVENANCE} yes, delete it"),
                "slack:owner:U01OWNER".to_owned()
            )]
        );
        *console.screen.lock().unwrap() = CLAUDE_QUESTION.replace("worktree?", "branch?");
        let Outcome::Refused { refusal, .. } = reply(&console, &question, "yes", "a").await else {
            panic!("changed question");
        };
        assert_eq!(refusal, Refusal::PromptChanged);
        assert_eq!(console.prompts.lock().unwrap().len(), 1);
    }

    #[test]
    fn key_plans_are_rechecked_against_the_pane_seen_under_the_lease() {
        let ParsedScreen::Choice(menu) = parse_screen(CLAUDE_MENU) else {
            panic!("menu");
        };
        let plan = super::KeyPlan {
            fingerprint: menu.fingerprint.clone(),
            option_index: 1,
            role: OptionRole::Choice,
            keys: menu.keys_for(1).unwrap(),
        };
        assert_eq!(plan.keys, [KEY_DOWN, KEY_ENTER]);
        assert!(plan.check_start(true, CLAUDE_MENU).is_ok());
        assert!(plan.check_start(false, CLAUDE_MENU).is_err(), "unblocked");
        // The cursor moved before the lease: the same keys would pick
        // another option.
        let moved = CLAUDE_MENU
            .replace("❯ 1. Yard server", "  1. Yard server")
            .replace("  2. Herdr plugin", "❯ 2. Herdr plugin");
        assert!(matches!(
            plan.check_start(true, &moved),
            Err(ConsoleError::PromptChanged(_))
        ));
        // A different prompt came up in the gap.
        assert!(plan.check_start(true, CLAUDE_PERMISSION).is_err());
        // Before Enter the cursor must be on the option.
        assert!(plan.check_before_enter(&moved).is_ok());
        assert!(plan.check_before_enter(CLAUDE_MENU).is_err());
        assert!(plan.check_before_enter(CLAUDE_PERMISSION).is_err());
        assert!(!plan.is_confirming(0) && plan.is_confirming(1));
        assert_eq!(
            super::console_refusal(ConsoleError::PromptChanged(String::new())),
            Refusal::PromptChanged
        );
    }
}

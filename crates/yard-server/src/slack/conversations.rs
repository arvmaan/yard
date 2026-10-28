//! Slack conversations, keyed by thread root ts: a top-level owner message
//! starts one, a reply in a thread Yard does not know is adopted as one
//! (seeded with what Yard knows about that thread), and every later owner
//! message in the thread is the next turn to the same agent with the
//! earlier turns.
//!
//! Bounded: at most [`MAX_CONVERSATIONS`] (least recently used dropped),
//! [`IDLE_TTL`] idle, [`KEEP_TURNS`] turns each, every question and answer
//! capped at [`TURN_CHARS`], and [`MAX_QUEUED`] messages waiting. One turn
//! per conversation runs at a time, for at most its busy bound
//! ([`Conversations::new`]); a message meanwhile is queued and sent, in
//! order, as the next turn.
//!
//! Kept across restarts in `slack-conversations.json` beside the other
//! Slack state ([`CONVERSATIONS_FILE`]: mode 0600, written atomically off
//! the lock, at most [`MAX_FILE_BYTES`] (least recently used dropped first),
//! every text redacted with [`redact_command`] before it is written). The
//! file names the workspace, DM channel and owner it belongs to; under
//! another identity it is dropped ([`Conversations::bind`]). A turn in
//! flight at a restart is watched again by the new process
//! ([`Conversations::resume`]), and its thread waits for it as before.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{actions::AgentTarget, held, message::redact_command, prompt::cap};

/// The conversations file, beside `slack-threads.json`.
pub const CONVERSATIONS_FILE: &str = "slack-conversations.json";
/// A larger conversations file is ignored, so none larger is written.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Messages waiting per conversation; more are not queued (and the owner
/// is told so). A turn in flight that has not been sent yet still counts:
/// it can go back to the queue (see [`Conversations::requeue`]), and an
/// accepted message must never be dropped to make room for it.
pub const MAX_QUEUED: usize = 5;
/// Conversations remembered at once.
pub const MAX_CONVERSATIONS: usize = 64;
/// A conversation unused this long is forgotten.
pub const IDLE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Turns replayed to the agent (and kept) per conversation.
pub const KEEP_TURNS: usize = 6;
/// Each stored question and answer, in characters.
pub const TURN_CHARS: usize = 1_500;

/// Who a conversation talks to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAgent {
    /// The dedicated Slack Superintendent, resolved (and recovered) for
    /// every turn; the main Superintendent answers when it can't start.
    SlackSuperintendent,
    /// The main Superintendent or a project orchestrator.
    Fixed(AgentTarget),
}

/// One question and, once relayed, its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    token: u64,
    pub command_id: Option<String>,
    pub question: String,
    pub answer: Option<String>,
}

impl Turn {
    /// A detached turn (prompt building and tests).
    #[must_use]
    pub fn new(question: &str, answer: Option<&str>) -> Self {
        Self {
            token: 0,
            command_id: None,
            question: question.to_owned(),
            answer: answer.map(str::to_owned),
        }
    }
}

/// How a turn is audited: the first message of a thread, or a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    Ask,
    FollowUp,
}

impl TurnKind {
    /// Audit kind and action.
    #[must_use]
    pub const fn audit(self) -> (&'static str, &'static str) {
        match self {
            Self::Ask => ("message", "ask"),
            Self::FollowUp => ("reply", "follow_up"),
        }
    }
}

/// An owner message waiting for its turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub question: String,
    /// Talk to this agent from this turn on (`<Project>:`, a Retry card);
    /// `None`: whoever the thread talks to now.
    #[serde(default)]
    pub agent: Option<(ConversationAgent, String)>,
    pub user: String,
    pub kind: TurnKind,
    pub queued_at_unix_ms: u64,
    /// Context for this turn only (an ambiguous `<Project>:`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Part of the text was redacted when it was saved: after a restart the
    /// redacted text is what is sent, and the owner is told.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
}

/// A turn as delivered, for watching its answer (also after a restart).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivered {
    pub command_id: String,
    pub target: AgentTarget,
    /// "the Superintendent" / "the X orchestrator" (already cleaned).
    pub to: String,
    #[serde(default)]
    pub answer_key: Option<String>,
    #[serde(default)]
    pub progress_ts: Option<String>,
    pub user: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Whose conversations these are: kept in the file, checked on start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub team_id: String,
    pub channel_id: String,
    pub user_id: String,
}

/// A thread whose next message waits for the agent (on a prompt, or its
/// terminal open in Yard) instead of being refused.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    since_unix_ms: u64,
    /// Not tried again before this (0: at the next pump).
    retry_at_unix_ms: u64,
    progress_ts: Option<String>,
}

/// A message waiting for its agent, tried again now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retried {
    pub since_unix_ms: u64,
    /// The progress message that says it waits (updated, not reposted).
    pub progress_ts: Option<String>,
}

/// A turn in flight at a restart, to watch again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewatch {
    pub root: String,
    pub token: u64,
    pub since_unix_ms: u64,
    pub question: String,
    pub delivered: Delivered,
}

/// What [`Conversations::resume`] found after a restart.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Resumed {
    /// Roots with messages waiting (pump them).
    pub waiting: Vec<String>,
    /// `(root, how many)` queued messages too old to send now.
    pub dropped: Vec<(String, usize)>,
    /// Turns in flight at the restart.
    pub rewatch: Vec<Rewatch>,
}

#[derive(Debug, Clone)]
struct Conversation {
    agent: ConversationAgent,
    to: String,
    /// What Yard knew about an adopted thread's root (sent with every turn).
    context: Option<String>,
    turns: Vec<Turn>,
    /// The token of the turn being watched, and when it began.
    in_flight: Option<(u64, u64)>,
    /// How the turn in flight was delivered (once sent).
    delivered: Option<Delivered>,
    /// The next queued message waits for the agent.
    waiting: Option<Waiting>,
    queue: VecDeque<Queued>,
    /// The owner was told this thread's messages are queued (once until
    /// the queue empties).
    queue_noticed: bool,
    last_used_unix_ms: u64,
}

impl Conversation {
    fn new(agent: ConversationAgent, to: &str, context: Option<String>, now_unix_ms: u64) -> Self {
        Self {
            agent,
            to: to.to_owned(),
            context,
            turns: Vec::new(),
            in_flight: None,
            delivered: None,
            waiting: None,
            queue: VecDeque::new(),
            queue_noticed: false,
            last_used_unix_ms: now_unix_ms,
        }
    }
}

/// A turn that may now be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begun {
    pub token: u64,
    pub agent: ConversationAgent,
    pub to: String,
    /// Earlier turns, oldest first (at most [`KEEP_TURNS`]).
    pub history: Vec<Turn>,
    /// What Yard knew about an adopted thread's root.
    pub context: Option<String>,
}

/// The next queued message, begun as a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Next {
    pub queued: Queued,
    pub begun: Begun,
    /// Set when this turn switched the thread to another agent.
    pub switched_to: Option<String>,
    /// Set when this message waited for its agent and is tried again.
    pub retried: Option<Retried>,
}

/// Why a turn was not begun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotBegun {
    /// No such conversation (never started, expired, evicted).
    Unknown,
    /// The previous turn is still being watched.
    Busy,
}

/// A message was queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    /// Nothing ahead of it: it is the next turn now.
    Ready,
    /// A turn is in flight or messages are ahead; `notify` the first time.
    Waiting { notify: bool },
}

/// Why a message was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotQueued {
    Unknown,
    /// [`MAX_QUEUED`] messages already wait.
    Full,
}

/// Every open conversation, by thread root ts.
#[derive(Debug)]
pub struct Conversations {
    open: HashMap<String, Conversation>,
    next_token: u64,
    busy_for_ms: u64,
    /// `slack-conversations.json` (`None`: in memory only).
    file: Option<PathBuf>,
    /// Whose conversations these are (from the file until [`Self::bind`]).
    owner: Option<Owner>,
    /// [`Self::bind`] ran: the conversations belong to the current owner.
    bound: bool,
    /// Writes, newest wins (off the conversations lock).
    writer: Arc<Writer>,
    saves: u64,
}

/// Serializes the file writes; a write older than the last one written is
/// skipped.
#[derive(Debug, Default)]
struct Writer {
    written: Mutex<u64>,
}

impl Writer {
    fn write(&self, path: &Path, seq: u64, bytes: &[u8]) {
        let mut written = self
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seq <= *written {
            return;
        }
        match held::write_bytes(path, bytes) {
            Ok(()) => *written = seq,
            Err(error) => warn!(
                path = %path.display(),
                %error,
                "Could not save Slack conversations; threads may need their context again after a restart"
            ),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    owner: Option<Owner>,
    conversations: Vec<SavedConversation>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedConversation {
    root: String,
    agent: ConversationAgent,
    to: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    turns: Vec<SavedTurn>,
    #[serde(default)]
    queue: Vec<Queued>,
    last_used_unix_ms: u64,
    /// The turn in flight when this was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_flight: Option<SavedInFlight>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedInFlight {
    since_unix_ms: u64,
    delivered: Delivered,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedTurn {
    command_id: Option<String>,
    question: String,
    answer: Option<String>,
}

/// `text` as it may be written to disk: every line redacted.
#[must_use]
pub fn redacted(text: &str) -> String {
    text.lines()
        .map(redact_command)
        .collect::<Vec<_>>()
        .join("\n")
}

impl Default for Conversations {
    fn default() -> Self {
        Self::new(DEFAULT_BUSY_FOR)
    }
}

/// [`Conversations::new`]'s default busy bound.
pub const DEFAULT_BUSY_FOR: Duration = Duration::from_secs(10 * 60);

impl Conversations {
    /// A turn holds its conversation at most `busy_for`; then the next
    /// turn may begin (the old one stays in the history, unanswered until
    /// its late answer arrives). In memory only.
    #[must_use]
    pub fn new(busy_for: Duration) -> Self {
        Self {
            open: HashMap::new(),
            next_token: 0,
            busy_for_ms: u64::try_from(busy_for.as_millis()).unwrap_or(u64::MAX),
            file: None,
            owner: None,
            bound: false,
            writer: Arc::default(),
            saves: 0,
        }
    }

    /// [`Self::new`], kept in `file`: what it holds now is loaded (a
    /// missing, foreign or unreadable file starts empty), and every change
    /// is written back.
    #[must_use]
    pub fn load(file: Option<PathBuf>, busy_for: Duration, now_unix_ms: u64) -> Self {
        let mut conversations = Self::new(busy_for);
        let Some(path) = file else {
            return conversations;
        };
        let saved = held::read_capped::<Saved>(&path, MAX_FILE_BYTES).unwrap_or_else(|reason| {
            if path.exists() {
                warn!(path = %path.display(), %reason, "Ignoring the Slack conversations file");
            }
            Saved::default()
        });
        conversations.owner = saved.owner;
        for saved in saved.conversations {
            let mut conversation = Conversation::new(
                saved.agent,
                &saved.to,
                saved.context,
                saved.last_used_unix_ms,
            );
            // A turn never sent was never asked: dropped.
            conversation.turns = saved
                .turns
                .into_iter()
                .filter(|turn| turn.command_id.is_some())
                .map(|turn| Turn {
                    token: 0,
                    command_id: turn.command_id,
                    question: cap(&turn.question, TURN_CHARS),
                    answer: turn.answer.map(|answer| cap(&answer, TURN_CHARS)),
                })
                .collect();
            let excess = conversation.turns.len().saturating_sub(KEEP_TURNS);
            conversation.turns.drain(..excess);
            conversation.queue = saved.queue.into_iter().take(MAX_QUEUED).collect();
            // The turn in flight: held (and watched again) as before.
            if let Some(in_flight) = saved.in_flight
                && let Some(turn) = conversation.turns.iter_mut().find(|turn| {
                    turn.answer.is_none()
                        && turn.command_id.as_deref() == Some(&in_flight.delivered.command_id)
                })
            {
                conversations.next_token += 1;
                turn.token = conversations.next_token;
                conversation.in_flight = Some((turn.token, in_flight.since_unix_ms));
                conversation.delivered = Some(in_flight.delivered);
            }
            conversations.open.insert(saved.root, conversation);
        }
        conversations.sweep(now_unix_ms);
        while conversations.open.len() > MAX_CONVERSATIONS {
            conversations.evict_oldest();
        }
        conversations.file = Some(path);
        conversations
    }

    /// Write every conversation (redacted) to the file, when there is one:
    /// encoded here, written off the lock (in a blocking task when a
    /// runtime runs, else now). Past [`MAX_FILE_BYTES`] the least recently
    /// used conversations are left out.
    fn save(&mut self) {
        let Some(path) = self.file.clone() else {
            return;
        };
        let mut conversations = self
            .open
            .iter()
            .map(|(root, conversation)| saved_conversation(root, conversation))
            .collect::<Vec<_>>();
        // Newest first, so the oldest are dropped when the file is too big.
        conversations.sort_by(|a, b| {
            b.last_used_unix_ms
                .cmp(&a.last_used_unix_ms)
                .then_with(|| a.root.cmp(&b.root))
        });
        let mut saved = Saved {
            owner: self.owner.clone(),
            conversations,
        };
        let bytes = loop {
            match held::encode(&saved) {
                Ok(bytes)
                    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= MAX_FILE_BYTES
                        || saved.conversations.is_empty() =>
                {
                    break bytes;
                }
                Ok(_) => {
                    saved.conversations.pop();
                }
                Err(error) => {
                    warn!(%error, "Could not encode Slack conversations");
                    return;
                }
            }
        };
        self.saves += 1;
        let (seq, writer) = (self.saves, Arc::clone(&self.writer));
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(move || writer.write(&path, seq, &bytes));
            }
            Err(_) => writer.write(&path, seq, &bytes),
        }
    }

    /// The conversations belong to `owner` (the current workspace, DM
    /// channel and owner): a file written for anyone else (or by a Yard
    /// that did not record it) is dropped. `true` when some were dropped.
    pub fn bind(&mut self, owner: Owner) -> bool {
        if self.bound && self.owner.as_ref() == Some(&owner) {
            return false;
        }
        let foreign = self.owner.as_ref() != Some(&owner);
        let dropped = foreign && !self.open.is_empty();
        if foreign {
            if dropped {
                warn!("Dropping Slack conversations saved for another workspace, channel or owner");
            }
            self.open.clear();
            self.owner = Some(owner);
            self.save();
        }
        self.bound = true;
        dropped
    }

    /// Whether [`Self::bind`] ran.
    #[must_use]
    pub const fn is_bound(&self) -> bool {
        self.bound
    }

    /// The token of `conversation`'s turn in flight, unless it is stale.
    fn holding(&self, conversation: &Conversation, now_unix_ms: u64) -> Option<u64> {
        conversation
            .in_flight
            .filter(|(_, since)| now_unix_ms.saturating_sub(*since) < self.busy_for_ms)
            .map(|(token, _)| token)
    }

    fn sweep(&mut self, now_unix_ms: u64) {
        let ttl = u64::try_from(IDLE_TTL.as_millis()).unwrap_or(u64::MAX);
        self.open.retain(|_, conversation| {
            now_unix_ms.saturating_sub(conversation.last_used_unix_ms) < ttl
        });
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .open
            .iter()
            .min_by_key(|(_, conversation)| conversation.last_used_unix_ms)
            .map(|(root, _)| root.clone());
        if let Some(oldest) = oldest {
            self.open.remove(&oldest);
        }
    }

    fn insert(&mut self, root: &str, conversation: Conversation, now_unix_ms: u64) {
        self.sweep(now_unix_ms);
        if !self.open.contains_key(root) && self.open.len() >= MAX_CONVERSATIONS {
            self.evict_oldest();
        }
        self.open.insert(root.to_owned(), conversation);
        self.save();
    }

    /// Start (or restart) the conversation rooted at `root`.
    pub fn start(&mut self, root: &str, agent: ConversationAgent, to: &str, now_unix_ms: u64) {
        self.insert(
            root,
            Conversation::new(agent, to, None, now_unix_ms),
            now_unix_ms,
        );
    }

    /// Adopt the thread rooted at `root` (one Yard does not know) as a
    /// conversation; `context` is what Yard knows about its root. An open
    /// conversation there is kept as it is.
    pub fn adopt(
        &mut self,
        root: &str,
        agent: ConversationAgent,
        to: &str,
        context: Option<String>,
        now_unix_ms: u64,
    ) {
        if self.contains(root, now_unix_ms) {
            return;
        }
        let context = context.map(|context| cap(context.trim(), TURN_CHARS));
        self.insert(
            root,
            Conversation::new(agent, to, context, now_unix_ms),
            now_unix_ms,
        );
    }

    /// Whether `root` is an open conversation now.
    pub fn contains(&mut self, root: &str, now_unix_ms: u64) -> bool {
        self.sweep(now_unix_ms);
        self.open.contains_key(root)
    }

    /// Who `root` talks to now.
    pub fn agent(&mut self, root: &str, now_unix_ms: u64) -> Option<(ConversationAgent, String)> {
        self.sweep(now_unix_ms);
        self.open
            .get(root)
            .map(|conversation| (conversation.agent.clone(), conversation.to.clone()))
    }

    /// Whether `root` has sent a turn yet (a Retry then is a follow-up).
    #[must_use]
    pub fn has_turns(&self, root: &str) -> bool {
        self.open
            .get(root)
            .is_some_and(|conversation| !conversation.turns.is_empty())
    }

    /// Whether a turn of `root` is in flight (and not yet stale).
    #[must_use]
    pub fn busy(&self, root: &str, now_unix_ms: u64) -> bool {
        self.open
            .get(root)
            .is_some_and(|conversation| self.holding(conversation, now_unix_ms).is_some())
    }

    /// Talk to another agent from now on; earlier turns are dropped.
    pub fn retarget(&mut self, root: &str, agent: ConversationAgent, to: &str, now_unix_ms: u64) {
        if let Some(conversation) = self.open.get_mut(root) {
            conversation.agent = agent;
            to.clone_into(&mut conversation.to);
            conversation.turns.clear();
            conversation.last_used_unix_ms = now_unix_ms;
            self.save();
        }
    }

    /// Begin a turn with `question`, unless one is still in flight (and
    /// not yet stale: a stale one is superseded).
    ///
    /// # Errors
    ///
    /// [`NotBegun`]: no such conversation, or its previous turn is in flight.
    pub fn begin(
        &mut self,
        root: &str,
        question: &str,
        now_unix_ms: u64,
    ) -> Result<Begun, NotBegun> {
        self.sweep(now_unix_ms);
        let conversation = self.open.get(root).ok_or(NotBegun::Unknown)?;
        if self.holding(conversation, now_unix_ms).is_some() {
            return Err(NotBegun::Busy);
        }
        self.next_token += 1;
        let token = self.next_token;
        let conversation = self.open.get_mut(root).ok_or(NotBegun::Unknown)?;
        let begun = begin_turn(conversation, token, question, now_unix_ms);
        self.save();
        Ok(begun)
    }

    /// Queue `queued` behind the turn in flight and any earlier messages.
    ///
    /// # Errors
    ///
    /// [`NotQueued`]: no such conversation, or [`MAX_QUEUED`] already wait
    /// (counting a turn in flight that is not sent yet, which may be
    /// requeued). The newest message is the one refused, so the caller can
    /// tell the owner and audit it.
    pub fn enqueue(
        &mut self,
        root: &str,
        queued: Queued,
        now_unix_ms: u64,
    ) -> Result<Enqueued, NotQueued> {
        self.sweep(now_unix_ms);
        let conversation = self.open.get(root).ok_or(NotQueued::Unknown)?;
        if waiting_count(conversation) >= MAX_QUEUED {
            return Err(NotQueued::Full);
        }
        let busy = self.holding(conversation, now_unix_ms).is_some();
        let conversation = self.open.get_mut(root).ok_or(NotQueued::Unknown)?;
        conversation.queue.push_back(queued);
        conversation.last_used_unix_ms = now_unix_ms;
        let enqueued = if busy || conversation.queue.len() > 1 {
            let notify = !conversation.queue_noticed;
            conversation.queue_noticed = true;
            Enqueued::Waiting { notify }
        } else {
            Enqueued::Ready
        };
        self.save();
        Ok(enqueued)
    }

    /// The oldest queued message of `root`, begun as its next turn, unless
    /// a turn is still in flight (or nothing waits). A message naming
    /// another agent switches the thread to it first.
    pub fn next(&mut self, root: &str, now_unix_ms: u64) -> Option<Next> {
        self.sweep(now_unix_ms);
        let conversation = self.open.get(root)?;
        if conversation.queue.is_empty()
            || self.holding(conversation, now_unix_ms).is_some()
            || conversation
                .waiting
                .as_ref()
                .is_some_and(|waiting| waiting.retry_at_unix_ms > now_unix_ms)
        {
            return None;
        }
        self.next_token += 1;
        let token = self.next_token;
        let conversation = self.open.get_mut(root)?;
        let queued = conversation.queue.pop_front()?;
        let retried = conversation.waiting.as_ref().map(|waiting| Retried {
            since_unix_ms: waiting.since_unix_ms,
            progress_ts: waiting.progress_ts.clone(),
        });
        if conversation.queue.is_empty() {
            conversation.queue_noticed = false;
        }
        let mut switched_to = None;
        if let Some((agent, to)) = &queued.agent
            && *agent != conversation.agent
        {
            conversation.agent = agent.clone();
            to.clone_into(&mut conversation.to);
            conversation.turns.clear();
            switched_to = Some(to.clone());
        }
        let begun = begin_turn(conversation, token, &queued.question, now_unix_ms);
        self.save();
        Some(Next {
            queued,
            begun,
            switched_to,
            retried,
        })
    }

    /// The turn `token` of `root` was not sent because its agent can't take
    /// it yet (on a prompt, terminal open in Yard): `queued` goes back to
    /// the front of the queue, tried again at `retry_at_unix_ms` (or when
    /// [`Self::wake`] runs). `progress_ts` is the message that says so.
    /// `false` when the turn is no longer this thread's turn in flight.
    pub fn requeue(
        &mut self,
        root: &str,
        token: u64,
        queued: Queued,
        progress_ts: Option<String>,
        retry_at_unix_ms: u64,
        now_unix_ms: u64,
    ) -> bool {
        let Some(conversation) = self.open.get_mut(root) else {
            return false;
        };
        if conversation.in_flight.map(|(held, _)| held) != Some(token) {
            return false;
        }
        conversation.in_flight = None;
        conversation.delivered = None;
        conversation
            .turns
            .retain(|turn| turn.token != token || turn.command_id.is_some());
        // Never truncate: `enqueue` reserved a slot for this unsent turn, so
        // the queue stays within MAX_QUEUED, and every message in it was
        // accepted (and acknowledged) already.
        conversation.queue.push_front(queued);
        debug_assert!(conversation.queue.len() <= MAX_QUEUED);
        let since_unix_ms = conversation
            .waiting
            .as_ref()
            .map_or(now_unix_ms, |waiting| waiting.since_unix_ms);
        let progress_ts = progress_ts.or_else(|| {
            conversation
                .waiting
                .as_ref()
                .and_then(|waiting| waiting.progress_ts.clone())
        });
        conversation.waiting = Some(Waiting {
            since_unix_ms,
            retry_at_unix_ms,
            progress_ts,
        });
        self.save();
        true
    }

    /// Something changed (a prompt was answered): every waiting thread may
    /// try its message again now. Returns those roots.
    pub fn wake(&mut self) -> Vec<String> {
        let mut roots = self
            .open
            .iter_mut()
            .filter_map(|(root, conversation)| {
                let waiting = conversation.waiting.as_mut()?;
                waiting.retry_at_unix_ms = 0;
                Some(root.clone())
            })
            .collect::<Vec<_>>();
        roots.sort();
        roots
    }

    /// After a restart: queued messages older than `within` are dropped
    /// (`(root, how many)`, so the owner can be told), the roots that still
    /// have messages waiting are returned to be sent, and the turns in
    /// flight at the restart to be watched again.
    pub fn resume(&mut self, now_unix_ms: u64, within: Duration) -> Resumed {
        self.sweep(now_unix_ms);
        let within = u64::try_from(within.as_millis()).unwrap_or(u64::MAX);
        let busy_for_ms = self.busy_for_ms;
        let mut resumed = Resumed::default();
        for (root, conversation) in &mut self.open {
            let before = conversation.queue.len();
            conversation
                .queue
                .retain(|queued| now_unix_ms.saturating_sub(queued.queued_at_unix_ms) < within);
            if conversation.queue.len() < before {
                resumed
                    .dropped
                    .push((root.clone(), before - conversation.queue.len()));
            }
            if conversation.queue.is_empty() {
                conversation.queue_noticed = false;
            } else {
                resumed.waiting.push(root.clone());
            }
            if let (Some((token, since)), Some(delivered)) =
                (conversation.in_flight, &conversation.delivered)
                && now_unix_ms.saturating_sub(since) < busy_for_ms
            {
                let question = conversation
                    .turns
                    .iter()
                    .find(|turn| turn.token == token)
                    .map(|turn| turn.question.clone())
                    .unwrap_or_default();
                resumed.rewatch.push(Rewatch {
                    root: root.clone(),
                    token,
                    since_unix_ms: since,
                    question,
                    delivered: delivered.clone(),
                });
            }
        }
        if !resumed.dropped.is_empty() {
            self.save();
        }
        resumed.waiting.sort();
        resumed.dropped.sort();
        resumed.rewatch.sort_by(|a, b| a.root.cmp(&b.root));
        resumed
    }

    /// How long until the turn in flight of `root` no longer holds it.
    #[must_use]
    pub fn held_for(&self, root: &str, now_unix_ms: u64) -> Option<Duration> {
        let conversation = self.open.get(root)?;
        let (_, since) = conversation.in_flight?;
        let left = (since + self.busy_for_ms).checked_sub(now_unix_ms)?;
        Some(Duration::from_millis(left))
    }

    /// The turn was delivered (as `delivered.command_id`).
    pub fn sent(&mut self, root: &str, token: u64, delivered: Delivered) {
        let command_id = delivered.command_id.clone();
        let Some(turn) = self.turn_mut(root, token) else {
            return;
        };
        turn.command_id = Some(command_id);
        if let Some(conversation) = self.open.get_mut(root) {
            conversation.waiting = None;
            if conversation.in_flight.map(|(held, _)| held) == Some(token) {
                conversation.delivered = Some(delivered);
            }
        }
        self.save();
    }

    /// The answer for `command_id` was relayed (also late); a turn still in
    /// flight ends now, so the owner can follow up at once.
    pub fn answered(&mut self, command_id: &str, answer: &str) {
        for conversation in self.open.values_mut() {
            let Some(turn) = conversation
                .turns
                .iter_mut()
                .find(|turn| turn.command_id.as_deref() == Some(command_id))
            else {
                continue;
            };
            turn.answer = Some(cap(answer.trim(), TURN_CHARS));
            if conversation.in_flight.map(|(token, _)| token) == Some(turn.token) {
                conversation.in_flight = None;
                conversation.delivered = None;
            }
            self.save();
            return;
        }
    }

    /// The turn's watch ended (answered, failed, timed out, or never sent):
    /// the next turn may start. A turn that was never sent is dropped.
    pub fn finish(&mut self, root: &str, token: u64) {
        let Some(conversation) = self.open.get_mut(root) else {
            return;
        };
        if conversation.in_flight.map(|(held, _)| held) == Some(token) {
            conversation.in_flight = None;
            conversation.delivered = None;
            conversation.waiting = None;
        }
        conversation
            .turns
            .retain(|turn| turn.token != token || turn.command_id.is_some());
        self.save();
    }

    fn turn_mut(&mut self, root: &str, token: u64) -> Option<&mut Turn> {
        self.open
            .get_mut(root)?
            .turns
            .iter_mut()
            .find(|turn| turn.token == token)
    }
}

/// `conversation` as written to the file: every text redacted (a queued
/// message changed by that is marked, so the owner is told after a
/// restart).
fn saved_conversation(root: &str, conversation: &Conversation) -> SavedConversation {
    let in_flight = match (conversation.in_flight, &conversation.delivered) {
        (Some((_, since_unix_ms)), Some(delivered)) => Some(SavedInFlight {
            since_unix_ms,
            delivered: delivered.clone(),
        }),
        _ => None,
    };
    SavedConversation {
        root: root.to_owned(),
        agent: conversation.agent.clone(),
        to: conversation.to.clone(),
        context: conversation.context.as_deref().map(redacted),
        turns: conversation
            .turns
            .iter()
            .map(|turn| SavedTurn {
                command_id: turn.command_id.clone(),
                question: redacted(&turn.question),
                answer: turn.answer.as_deref().map(redacted),
            })
            .collect(),
        queue: conversation
            .queue
            .iter()
            .map(|queued| {
                let question = redacted(&queued.question);
                Queued {
                    redacted: queued.redacted || question != queued.question,
                    question,
                    note: queued.note.as_deref().map(redacted),
                    ..queued.clone()
                }
            })
            .collect(),
        last_used_unix_ms: conversation.last_used_unix_ms,
        in_flight,
    }
}

/// Messages waiting in `conversation`: its queue, plus a turn in flight
/// that has not been sent yet (it goes back to the queue if its agent
/// can't take it).
fn waiting_count(conversation: &Conversation) -> usize {
    let unsent = conversation.in_flight.is_some() && conversation.delivered.is_none();
    conversation.queue.len() + usize::from(unsent)
}

/// Mark `token` in flight in `conversation` and add its turn.
fn begin_turn(
    conversation: &mut Conversation,
    token: u64,
    question: &str,
    now_unix_ms: u64,
) -> Begun {
    conversation.last_used_unix_ms = now_unix_ms;
    conversation.in_flight = Some((token, now_unix_ms));
    conversation.delivered = None;
    let history = conversation.turns.clone();
    conversation.turns.push(Turn {
        token,
        command_id: None,
        question: cap(question.trim(), TURN_CHARS),
        answer: None,
    });
    let excess = conversation.turns.len().saturating_sub(KEEP_TURNS);
    conversation.turns.drain(..excess);
    Begun {
        token,
        agent: conversation.agent.clone(),
        to: conversation.to.clone(),
        history,
        context: conversation.context.clone(),
    }
}

#[cfg(test)]
#[path = "conversations_tests.rs"]
mod tests;

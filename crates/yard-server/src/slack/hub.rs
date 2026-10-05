//! Inbound Slack (phase 2): the Socket Mode loop, the owner filter, and
//! the answer flow, run inside the notifier.
//!
//! Runs only with `YARD_SLACK_NOTIFICATIONS=on`, `YARD_SLACK_INBOUND=on`
//! and `YARD_SLACK_APP_SECRET_ID`. Two supervised tasks: the socket reader
//! (connect, ack, queue) and the worker (filter, answer, reply), so a slow
//! answer never delays an ack. The bot session (`auth.test` +
//! `conversations.open`) supplies team, enterprise, DM channel and bot ids
//! for the filter; nothing is accepted before it exists. Owner messages are
//! a reply to a question card, a status command ([`commands`]), or a
//! question for an orchestrator whose answer is watched for in the
//! background ([`relay`]).

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::{
    NotifierParts, SlackNotifier,
    actions::{
        self, ActionRegistry, AgentConsole, AgentTarget, CardSpec, ConsoleError, NavAction,
        NavError, Outcome, PendingAction, Refusal, TerminalIdentity,
    },
    audit::{AuditLog, AuditRecord},
    blocks,
    cards::{
        BUTTON_ACTION_PREFIX, CardContext, answered_card, not_sent_card, plain, prompt_card,
        refusal_text,
    },
    client::SlackError,
    commands::{self, Command, ProjectName},
    controls::{self, Applied, Control},
    detector::{AttentionEvent, AttentionKind, EventSource},
    inbound::{Envelope, Inbound, InboundContext, InboundFilter, OwnerAction, OwnerMessage},
    lock,
    message::OutgoingMessage,
    outbox::Batch,
    policy::DigestKind,
    presence::ViewTarget,
    prompt::{OptionRole, ParsedScreen, parse_screen},
    relay::{self, RelayTiming},
    secret::{AppToken, SecretError, SecretSource},
    socket::{self, SessionEnd, SessionStats, SocketEvent, UrlPolicy},
    unix_ms, views,
};
use crate::config::{SlackInbound, SlackSettings};

mod asking;

/// Envelopes waiting for the worker; more are dropped (already acked).
const QUEUE_CAPACITY: usize = 64;
const WAIT_FOR_BOT_SESSION: Duration = Duration::from_secs(5);
const WAITING_FOR_BOT: &str = "Waiting for the bot connection";
/// Reply to an owner DM that arrived too late (or too early) to act on.
const LATE_MESSAGE: &str =
    "Yard was offline or reconnecting when you sent this, so nothing was done. Send it again.";
/// A plain reply in a thread that is not a question card's thread.
const THREAD_REPLY_NOT_SENT: &str = "Nothing was sent: only a question card's thread takes a reply. Use the card's buttons, start with `<Project>:` to ask that project's orchestrator, or ask the Superintendent at the top level of this DM.";
const WORKER_PANIC_BACKOFF: Duration = Duration::from_secs(5);
/// How long after sending keys the pane is re-read for the next prompt.
const SETTLE_AFTER_SEND: Duration = Duration::from_millis(1_500);

/// Test seams for inbound.
#[derive(Debug, Clone)]
pub(crate) struct InboundParts {
    pub audit_file: Option<PathBuf>,
    pub url_policy: UrlPolicy,
    pub idle_timeout: Duration,
    pub relay: RelayTiming,
    /// After keys are sent, the pane is re-read this much later.
    pub settle: Duration,
    /// Where "Open in Yard" buttons point ([`blocks::ui_url`]).
    pub ui_url: String,
}

impl Default for InboundParts {
    fn default() -> Self {
        Self {
            audit_file: None,
            url_policy: UrlPolicy::Slack,
            idle_timeout: socket::IDLE_TIMEOUT,
            relay: RelayTiming::default(),
            settle: SETTLE_AFTER_SEND,
            ui_url: blocks::DEFAULT_UI_URL.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundStatusKind {
    Off,
    Misconfigured,
    Connecting,
    Connected,
    Error,
}

/// `inbound` in `GET /api/v1/integrations/slack`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboundStatusView {
    pub status: InboundStatusKind,
    pub last_error: Option<String>,
}

impl InboundStatusView {
    #[must_use]
    pub const fn off() -> Self {
        Self {
            status: InboundStatusKind::Off,
            last_error: None,
        }
    }
}

pub(crate) struct InboundState {
    /// `None` unless inbound is enabled and the app secret is configured.
    app_secrets: Option<SecretSource>,
    misconfigured: Option<String>,
    app_token: tokio::sync::Mutex<Option<AppToken>>,
    console: OnceLock<Arc<dyn AgentConsole>>,
    registry: Mutex<ActionRegistry>,
    filter: Mutex<InboundFilter>,
    audit: AuditLog,
    url_policy: UrlPolicy,
    idle_timeout: Duration,
    status: Mutex<InboundStatusView>,
    relay: RelayTiming,
    settle: Duration,
    pub(crate) ui_url: String,
    /// Answers being watched ([`relay::MAX_WATCHERS`] at most).
    watchers: Arc<AtomicUsize>,
    /// Answers still watched after their window ([`relay::MAX_LATE_WATCHERS`]).
    late_watchers: Arc<AtomicUsize>,
    /// Command ids whose answer was relayed (newest last, bounded).
    relayed: Mutex<std::collections::VecDeque<String>>,
    /// Questions still watched (window or late window) by command id, so
    /// shutdown can say Yard restarted instead of leaving "Waiting…".
    asking: Mutex<std::collections::HashMap<String, asking::Asked>>,
    /// Set on shutdown: the socket is closed cleanly and not reopened.
    closing: tokio::sync::watch::Sender<bool>,
    /// A Socket Mode connection is open now.
    socket_open: tokio::sync::watch::Sender<bool>,
}

/// One watched answer; frees its slot when dropped (also on panic).
struct WatchSlot(Arc<AtomicUsize>);

impl WatchSlot {
    fn take(watchers: &Arc<AtomicUsize>, most: usize) -> Option<Self> {
        watchers
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |watching| {
                (watching < most).then_some(watching + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(watchers)))
    }
}

impl Drop for WatchSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl InboundState {
    pub(crate) fn new(settings: Option<&SlackSettings>, parts: &NotifierParts) -> Self {
        let (app_secrets, misconfigured) = match settings
            .map(|settings| (settings, &settings.inbound))
        {
            Some((settings, SlackInbound::Enabled { app_secret_id })) => {
                let mut source = SecretSource::from_settings(settings).for_secret(app_secret_id);
                source.aws_binary.clone_from(&parts.aws_binary);
                (Some(source), None)
            }
            Some((_, SlackInbound::Misconfigured(reason))) => (None, Some(reason.clone())),
            _ => (None, None),
        };
        let status = match (&app_secrets, &misconfigured) {
            (Some(_), _) => InboundStatusView {
                status: InboundStatusKind::Connecting,
                last_error: None,
            },
            (None, Some(reason)) => InboundStatusView {
                status: InboundStatusKind::Misconfigured,
                last_error: Some(reason.clone()),
            },
            (None, None) => InboundStatusView::off(),
        };
        Self {
            app_secrets,
            misconfigured,
            app_token: tokio::sync::Mutex::new(None),
            console: OnceLock::new(),
            registry: Mutex::default(),
            filter: Mutex::default(),
            audit: AuditLog::new(parts.inbound.audit_file.clone()),
            url_policy: parts.inbound.url_policy,
            idle_timeout: parts.inbound.idle_timeout,
            status: Mutex::new(status),
            relay: parts.inbound.relay,
            settle: parts.inbound.settle,
            ui_url: parts.inbound.ui_url.clone(),
            watchers: Arc::default(),
            late_watchers: Arc::default(),
            relayed: Mutex::default(),
            asking: Mutex::default(),
            closing: tokio::sync::watch::Sender::new(false),
            socket_open: tokio::sync::watch::Sender::new(false),
        }
    }
}

/// The agent a notification is about, for its prompt card or "Answer".
pub(crate) fn event_target(event: &AttentionEvent) -> Option<AgentTarget> {
    Some(match event.view_target.as_ref()? {
        ViewTarget::Assignment(assignment_id) => AgentTarget::Assignment {
            project_id: event.project_id.clone()?,
            assignment_id: assignment_id.clone(),
        },
        ViewTarget::ProjectOrchestrator(project_id) => AgentTarget::ProjectOrchestrator {
            project_id: project_id.clone(),
        },
        ViewTarget::YardOrchestrator => AgentTarget::YardOrchestrator,
        ViewTarget::CoordinationNode(node_id) => AgentTarget::CoordinationNode {
            node_id: node_id.clone(),
        },
    })
}

fn target_key(target: &AgentTarget) -> String {
    match target {
        AgentTarget::Assignment {
            project_id,
            assignment_id,
        } => format!("assignment:{project_id}/{assignment_id}"),
        AgentTarget::ProjectOrchestrator { project_id } => format!("orchestrator:{project_id}"),
        AgentTarget::YardOrchestrator => "superintendent".to_owned(),
        AgentTarget::CoordinationNode { node_id } => format!("workstream:{node_id}"),
    }
}

const fn role_name(role: OptionRole) -> &'static str {
    match role {
        OptionRole::Allow => "allow",
        OptionRole::Deny => "deny",
        OptionRole::Choice => "choice",
    }
}

fn refusal_name(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::Unknown => "unknown",
        Refusal::Expired => "expired",
        Refusal::AlreadyUsed => "already_used",
        Refusal::Gone(_) => "gone",
        Refusal::TerminalChanged => "terminal_changed",
        Refusal::NoLongerBlocked => "no_longer_blocked",
        Refusal::PromptChanged => "prompt_changed",
        Refusal::NotOffered => "not_offered",
        Refusal::Busy(_) => "busy",
        Refusal::Unavailable(_) => "unavailable",
        Refusal::Failed(_) => "failed",
    }
}

impl super::Delivery {
    /// Respect Slack's per-channel pace between our posts.
    async fn pace(&mut self) {
        if let Some(last) = self.last_post_at {
            let wait = super::CHANNEL_MIN_INTERVAL.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        self.last_post_at = Some(Instant::now());
    }

    /// Post in the owner DM: in `thread_ts` quietly, or top level. Returns
    /// the posted message's ts.
    pub(super) async fn post_inbound(
        &mut self,
        message: &OutgoingMessage,
        thread_ts: Option<&str>,
    ) -> Result<String, SlackError> {
        self.pace().await;
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| SlackError::Transport("not connected".to_owned()))?;
        let posted = match thread_ts {
            Some(thread_ts) => {
                self.api
                    .post_thread_reply(&session.token, &session.channel, message, thread_ts)
                    .await?
            }
            None => {
                self.api
                    .post_message(&session.token, &session.channel, message, None)
                    .await?
            }
        };
        Ok(posted.ts)
    }

    async fn update_inbound(
        &mut self,
        ts: &str,
        message: &OutgoingMessage,
    ) -> Result<(), SlackError> {
        self.pace().await;
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| SlackError::Transport("not connected".to_owned()))?;
        self.api
            .update_message(&session.token, &session.channel, ts, message)
            .await
    }
}

impl SlackNotifier {
    /// Give inbound Slack Yard's guarded agent paths (set once, when the
    /// HTTP services are built).
    pub fn attach_console(&self, console: Arc<dyn AgentConsole>) {
        let _ = self.inner.inbound.console.set(console);
    }

    pub(crate) fn console(&self) -> Option<Arc<dyn AgentConsole>> {
        self.inner.inbound.console.get().cloned()
    }

    /// Occupy every watch slot (as if that many answers were watched).
    #[cfg(test)]
    pub(crate) fn fill_watchers(&self) {
        self.inner
            .inbound
            .watchers
            .store(relay::MAX_WATCHERS, Ordering::SeqCst);
    }

    pub(crate) fn inbound_enabled(&self) -> bool {
        self.settings().is_some() && self.inner.inbound.app_secrets.is_some()
    }

    pub(crate) fn inbound_status(&self) -> InboundStatusView {
        lock(&self.inner.inbound.status).clone()
    }

    fn set_inbound_status(&self, status: InboundStatusKind, error: Option<String>) {
        let mut current = lock(&self.inner.inbound.status);
        current.status = status;
        current.last_error = error;
    }

    /// The owner-only filter context from the bot session (connecting it
    /// if needed); `None` while it is not connected.
    async fn inbound_context(&self) -> Option<InboundContext> {
        let mut delivery = self.inner.delivery.lock().await;
        match delivery.ensure_session(Instant::now()).await {
            Ok(true) => self.record_connected(&delivery),
            Ok(false) => {}
            Err(error) => {
                self.record_connect_error(&error);
                return None;
            }
        }
        let session = delivery.session.as_ref()?;
        Some(InboundContext {
            team_id: session.identity.team_id.clone(),
            enterprise_id: session.identity.enterprise_id.clone(),
            owner_user_id: delivery.owner_user_id.clone(),
            dm_channel: session.channel.clone(),
            bot_user_id: session.identity.bot_user_id.clone(),
            bot_id: session.identity.bot_id.clone(),
            app_id: None,
        })
    }

    /// The app-level token, read from its secret once and kept in memory.
    async fn app_token(&self) -> Result<AppToken, SecretError> {
        let mut cached = self.inner.inbound.app_token.lock().await;
        if let Some(token) = cached.as_ref() {
            return Ok(token.clone());
        }
        let source = self
            .inner
            .inbound
            .app_secrets
            .as_ref()
            .ok_or(SecretError::NotAnAppToken)?;
        let token = source.fetch_app_token().await?;
        *cached = Some(token.clone());
        Ok(token)
    }

    async fn forget_app_token(&self) {
        self.inner.inbound.app_token.lock().await.take();
    }

    async fn post(&self, message: &OutgoingMessage, thread_ts: Option<&str>) -> Option<String> {
        let mut delivery = self.inner.delivery.lock().await;
        match delivery.post_inbound(message, thread_ts).await {
            Ok(ts) => Some(ts),
            Err(error) => {
                self.log_failure(
                    "inbound reply",
                    &format!("Posting a Slack reply failed: {error}"),
                );
                None
            }
        }
    }

    async fn update(&self, ts: &str, message: &OutgoingMessage) {
        let mut delivery = self.inner.delivery.lock().await;
        if let Err(error) = delivery.update_inbound(ts, message).await {
            debug!(%error, "Updating a Slack prompt card failed");
        }
    }

    fn audit(
        &self,
        kind: &'static str,
        action: &str,
        target: Option<&AgentTarget>,
        outcome: &str,
        user: &str,
    ) {
        self.inner.inbound.audit.record(&AuditRecord {
            at_unix_ms: unix_ms(),
            slack_user: user.to_owned(),
            kind,
            action: action.to_owned(),
            target: target.map(target_key),
            outcome: outcome.to_owned(),
        });
    }
}

impl SlackNotifier {
    /// Post a card for what `target` shows now (buttons for a recognized
    /// prompt, a reply thread for a question, else a redacted excerpt) in
    /// `thread_ts` or top level. Used for a changed prompt and by
    /// notifications / the `blocked` command.
    ///
    /// # Errors
    ///
    /// Returns a short reason when there is no console, the agent can't be
    /// read, or Slack rejects the post.
    pub(crate) async fn post_prompt_card(
        &self,
        target: &AgentTarget,
        title: &str,
        thread_ts: Option<&str>,
    ) -> Result<(), String> {
        let console = self
            .inner
            .inbound
            .console
            .get()
            .cloned()
            .ok_or("inbound Slack is not ready")?;
        let snapshot = console
            .snapshot(target)
            .await
            .map_err(|error| error.to_string())?;
        if !snapshot.blocked {
            // Screen content goes to Slack only for a blocked prompt: never
            // an excerpt of live output.
            let text = format!("{title} is no longer blocked. Nothing to answer here.");
            return self
                .post(&plain(&text), thread_ts)
                .await
                .map(|_| ())
                .ok_or_else(|| "Slack rejected the post".to_owned());
        }
        let screen = console
            .screen(target)
            .await
            .map_err(|error| error.to_string())?;
        let screen = parse_screen(&screen);
        self.post_card(target, &snapshot.identity, title, &screen, thread_ts)
            .await
            .ok_or_else(|| "Slack rejected the post".to_owned())
    }

    /// An agent that finished its turn by asking a question (Herdr reports
    /// a question at the input box as `done`/`idle`, not `blocked`): post a
    /// question card so the owner can reply. Anything else on its screen
    /// (a summary, a menu, live output) is never posted.
    ///
    /// # Errors
    ///
    /// Returns a short reason when there is no console, the agent can't be
    /// read, or Slack rejects the post.
    pub(crate) async fn post_question_if_asked(
        &self,
        target: &AgentTarget,
        title: &str,
    ) -> Result<(), String> {
        let console = self
            .inner
            .inbound
            .console
            .get()
            .cloned()
            .ok_or("inbound Slack is not ready")?;
        let snapshot = console
            .snapshot(target)
            .await
            .map_err(|error| error.to_string())?;
        if snapshot.blocked {
            return Ok(());
        }
        let screen = console
            .screen(target)
            .await
            .map_err(|error| error.to_string())?;
        let screen = parse_screen(&screen);
        if !matches!(screen, ParsedScreen::Question { .. }) {
            return Ok(());
        }
        self.post_card(target, &snapshot.identity, title, &screen, None)
            .await
            .ok_or_else(|| "Slack rejected the post".to_owned())
    }

    async fn post_card(
        &self,
        target: &AgentTarget,
        identity: &TerminalIdentity,
        title: &str,
        screen: &ParsedScreen,
        thread_ts: Option<&str>,
    ) -> Option<()> {
        let ids = match screen {
            ParsedScreen::Choice(choice) => {
                let spec = CardSpec {
                    target,
                    identity,
                    fingerprint: &choice.fingerprint,
                    title,
                    options: choice
                        .options
                        .iter()
                        .map(|option| (option.index, option.role, option.label.clone()))
                        .collect(),
                };
                match lock(&self.inner.inbound.registry).issue(&spec, Instant::now()) {
                    Ok(ids) => ids,
                    Err(error) => {
                        warn!(%error, "No random action ids; posting the prompt without buttons");
                        Vec::new()
                    }
                }
            }
            ParsedScreen::Question { .. } | ParsedScreen::Unparsed { .. } => Vec::new(),
        };
        // A question card is always its own thread root, so a reply there
        // can only mean this agent (several cards in one thread would share
        // a root and a reply would reach whichever registered last).
        let thread_ts = match screen {
            ParsedScreen::Question { .. } => None,
            _ => thread_ts,
        };
        let posted = self
            .post(
                &prompt_card(
                    title,
                    screen,
                    &ids,
                    &CardContext {
                        ui_url: &self.inner.inbound.ui_url,
                        pane: Some(&identity.pane_id),
                        at_unix_ms: unix_ms(),
                    },
                ),
                thread_ts,
            )
            .await?;
        if let ParsedScreen::Question { fingerprint, .. } = screen {
            lock(&self.inner.inbound.registry).expect_reply(
                &posted,
                target,
                identity,
                fingerprint,
                title,
                Instant::now(),
            );
        }
        Some(())
    }

    async fn handle_action(&self, console: &dyn AgentConsole, action: OwnerAction, user: &str) {
        let thread = action
            .thread_ts
            .clone()
            .unwrap_or_else(|| action.message_ts.clone());
        let nav = lock(&self.inner.inbound.registry).take_nav(&action.action, Instant::now());
        if let Some(nav) = nav {
            self.handle_nav(nav, &action, user).await;
            return;
        }
        let taken = lock(&self.inner.inbound.registry).take(&action.action, Instant::now());
        let pending: PendingAction = match taken {
            Ok(pending) => pending,
            Err(error) => {
                let refusal = Refusal::from(error);
                // A navigation button Yard no longer knows (restart, or long
                // forgotten): its card (help, status, a question's timeout)
                // is not a prompt, so it is left as it is.
                if matches!(refusal, Refusal::Unknown)
                    && !action.action_id.starts_with(BUTTON_ACTION_PREFIX)
                {
                    self.audit("button", "nav", None, "refused:unknown", user);
                    let text = "Yard no longer knows that button (it expired or Yard restarted), so nothing was run. Say `blocked` for the current prompts or `help` for fresh buttons.";
                    self.post(&plain(text), Some(&thread)).await;
                    return;
                }
                self.audit(
                    "button",
                    "unknown",
                    None,
                    &format!("refused:{}", refusal_name(&refusal)),
                    user,
                );
                // Expired or unknown buttons can never work: remove them.
                // (An already-used card was collapsed by the first click.)
                if matches!(refusal, Refusal::Expired | Refusal::Unknown) {
                    self.update(&action.message_ts, &not_sent_card("This prompt", &refusal))
                        .await;
                }
                self.post(&plain(&refusal_text("That agent", &refusal)), Some(&thread))
                    .await;
                return;
            }
        };
        let outcome = actions::unblock(console, &pending).await;
        let role = role_name(pending.role);
        match outcome {
            Outcome::Sent => {
                self.audit("button", role, Some(&pending.target), "sent", user);
                let answer = match pending.role {
                    OptionRole::Allow => "allowed once".to_owned(),
                    OptionRole::Deny => "denied".to_owned(),
                    OptionRole::Choice => pending.label.clone(),
                };
                self.update(
                    &action.message_ts,
                    &answered_card(&pending.title, &answer, unix_ms()),
                )
                .await;
                let reply = format!(
                    "Sent to {}: {}.",
                    pending.title,
                    super::message::clean_prompt_line(&answer)
                );
                self.post(&plain(&reply), Some(&thread)).await;
                let hub = self.clone();
                tokio::spawn(async move { hub.check_after_send(pending, thread).await });
            }
            Outcome::Refused { refusal, current } => {
                let outcome = format!("refused:{}", refusal_name(&refusal));
                self.audit("button", role, Some(&pending.target), &outcome, user);
                self.update(&action.message_ts, &not_sent_card(&pending.title, &refusal))
                    .await;
                self.post(
                    &plain(&refusal_text(&pending.title, &refusal)),
                    Some(&thread),
                )
                .await;
                if matches!(refusal, Refusal::PromptChanged)
                    && let Some(current) = current
                {
                    self.post_card(
                        &pending.target,
                        &pending.identity,
                        &pending.title,
                        &current,
                        Some(&thread),
                    )
                    .await;
                }
            }
        }
    }

    /// After a button was sent: re-read the pane. A multi-step dialog
    /// (another question, a Submit step) gets its next card in the same
    /// thread; an unchanged prompt gets "still shows this prompt". Nothing
    /// is posted once the agent is no longer blocked or its terminal changed.
    async fn check_after_send(&self, pending: PendingAction, thread: String) {
        tokio::time::sleep(self.inner.inbound.settle).await;
        let Some(console) = self.inner.inbound.console.get().cloned() else {
            return;
        };
        let Ok(snapshot) = console.snapshot(&pending.target).await else {
            return;
        };
        if !snapshot.blocked || snapshot.identity != pending.identity {
            return;
        }
        let Ok(screen) = console.screen(&pending.target).await else {
            return;
        };
        let screen = parse_screen(&screen);
        let same = match &screen {
            ParsedScreen::Choice(choice) => choice.fingerprint == pending.fingerprint,
            ParsedScreen::Question { fingerprint, .. } => *fingerprint == pending.fingerprint,
            ParsedScreen::Unparsed { .. } => false,
        };
        if same {
            let text = format!(
                "{} still shows this prompt, so the answer may not have taken effect. Open Yard.",
                pending.title
            );
            self.post(&plain(&text), Some(&thread)).await;
            return;
        }
        let text = format!("{} is waiting on another prompt:", pending.title);
        self.post(&plain(&text), Some(&thread)).await;
        self.post_card(
            &pending.target,
            &snapshot.identity,
            &pending.title,
            &screen,
            Some(&thread),
        )
        .await;
    }

    async fn handle_message(
        &self,
        console: &Arc<dyn AgentConsole>,
        message: OwnerMessage,
        user: &str,
    ) {
        let question = message.thread_ts.as_deref().and_then(|thread| {
            lock(&self.inner.inbound.registry).take_question(thread, Instant::now())
        });
        let Some(question) = question else {
            let closed = message.thread_ts.as_deref().is_some_and(|thread| {
                lock(&self.inner.inbound.registry).question_closed(thread, Instant::now())
            });
            if closed {
                self.audit("reply", "free_text", None, "refused:closed", user);
                let thread = message.thread_ts.as_deref().unwrap_or(&message.ts);
                self.post(
                    &plain("That question was already answered or has expired, so nothing was sent. Say `blocked` for the current prompts, or ask at the top level of this DM."),
                    Some(thread),
                )
                .await;
                return;
            }
            self.handle_command(console, message, user).await;
            return;
        };
        let console = console.as_ref();
        let thread = message
            .thread_ts
            .clone()
            .unwrap_or_else(|| message.ts.clone());
        let actor = format!("slack:owner:{user}");
        let outcome = actions::reply(console, &question, &message.text, &actor).await;
        match outcome {
            Outcome::Sent => {
                self.audit("reply", "free_text", Some(&question.target), "sent", user);
                self.post(
                    &plain(&format!("Sent your reply to {}.", question.title)),
                    Some(&thread),
                )
                .await;
            }
            Outcome::Refused { refusal, current } => {
                let outcome = format!("refused:{}", refusal_name(&refusal));
                self.audit("reply", "free_text", Some(&question.target), &outcome, user);
                self.post(
                    &plain(&refusal_text(&question.title, &refusal)),
                    Some(&thread),
                )
                .await;
                if matches!(refusal, Refusal::PromptChanged)
                    && let Some(current) = current
                {
                    self.post_card(
                        &question.target,
                        &question.identity,
                        &question.title,
                        &current,
                        Some(&thread),
                    )
                    .await;
                }
            }
        }
    }

    /// Messages that are not a reply to a question: status commands
    /// (deterministic, from durable state) and questions for the
    /// Superintendent or a project orchestrator.
    async fn handle_command(
        &self,
        console: &Arc<dyn AgentConsole>,
        message: OwnerMessage,
        user: &str,
    ) {
        let thread = message
            .thread_ts
            .clone()
            .unwrap_or_else(|| message.ts.clone());
        let (records, projects) = match self.durable_state().await {
            Ok(state) => state,
            Err(error) => {
                debug!(%error, "Reading Yard state for a Slack command failed");
                self.audit("message", "command", None, "failed:store", user);
                self.post(
                    &plain("Yard could not read its state just now; try again."),
                    Some(&thread),
                )
                .await;
                return;
            }
        };
        let ui_url = self.inner.inbound.ui_url.clone();
        let mut nav = self.nav_issuer();
        let (action, reply) = match commands::parse(&message.text, &projects) {
            Command::Help => ("help", views::help_card(&mut nav, &ui_url)),
            Command::Status => (
                "status",
                views::status_card(&records, &projects, &mut nav, &ui_url),
            ),
            Command::ProjectStatus(query) => (
                "status_project",
                match commands::find_project(&query, &records, &projects) {
                    Ok(project) => views::project_card(project, &records, &mut nav, &ui_url),
                    Err(_) => plain(&commands::project_status_text(&query, &records, &projects)),
                },
            ),
            Command::Review => ("review", views::review_card(&records, &mut nav, &ui_url)),
            Command::AmbiguousProject(name) => (
                "ask",
                plain(&format!(
                    "Several projects are named \"{name}\"; nothing was sent. Ask in Yard, or rename one."
                )),
            ),
            Command::Blocked => {
                drop(nav);
                self.post_blocked(&records, &thread, user).await;
                return;
            }
            Command::Control(control) => {
                drop(nav);
                self.run_control(control, &thread, Some(user)).await;
                return;
            }
            Command::ControlUsage(text) => {
                drop(nav);
                self.audit("message", "mute", None, "refused:invalid", user);
                self.post(&plain(&text), Some(&thread)).await;
                return;
            }
            Command::Ask {
                target,
                to,
                question,
            } => {
                drop(nav);
                // A reply in a thread (a card, a notification, an answer)
                // without an explicit `<Project>:` is meant for whatever
                // that thread is about, never a new Superintendent question.
                if message.thread_ts.is_some() && target == AgentTarget::YardOrchestrator {
                    self.audit("reply", "free_text", None, "refused:not_a_question", user);
                    self.post(&plain(THREAD_REPLY_NOT_SENT), Some(&thread))
                        .await;
                    return;
                }
                self.ask(console, target, &to, &question, &thread, user)
                    .await;
                return;
            }
        };
        drop(nav);
        self.audit("message", action, None, "answered", user);
        self.post(&reply, Some(&thread)).await;
    }

    /// A quiet-hours control (typed or tapped). `audit_user` is set for a
    /// typed command; a button click is already audited by `handle_nav`.
    async fn run_control(&self, control: Control, thread: &str, audit_user: Option<&str>) {
        // `digest` is answered in this thread now; holding the delivery
        // lock first keeps the tick from posting it elsewhere meanwhile.
        let digest_delivery = if matches!(control, Control::Digest) {
            Some(self.inner.delivery.lock().await)
        } else {
            None
        };
        let applied = controls::apply(&mut lock(&self.inner.policy), control);
        let requested = matches!(applied, Applied::DigestRequested);
        let (outcome, reply) = match applied {
            Applied::Settings(snapshot) => {
                let ui_url = self.inner.inbound.ui_url.clone();
                let mut nav = self.nav_issuer();
                (
                    "answered",
                    Some(controls::settings_card(&snapshot, &mut nav, &ui_url)),
                )
            }
            Applied::Reply { text, outcome } => (outcome, Some(plain(&text))),
            // The digest itself is the reply, posted below in this thread.
            Applied::DigestRequested => ("requested", None),
        };
        if let Some(user) = audit_user {
            self.audit("message", control.name(), None, outcome, user);
        }
        match (requested, digest_delivery) {
            (true, Some(mut delivery)) => {
                self.send_digest(&mut delivery, DigestKind::Requested, Some(thread))
                    .await;
                return;
            }
            // Never post below while holding the delivery lock.
            (_, delivery) => drop(delivery),
        }
        if let Some(reply) = reply {
            self.post(&reply, Some(thread)).await;
        }
    }

    /// Issues navigation ids in the registry (`None` without an RNG).
    pub(super) fn nav_issuer(&self) -> impl FnMut(NavAction) -> Option<String> + '_ {
        self.nav_issuer_for(None)
    }

    /// [`Self::nav_issuer`] whose buttons last `ttl` (`None`: each action's
    /// own lifetime).
    pub(super) fn nav_issuer_for(
        &self,
        ttl: Option<Duration>,
    ) -> impl FnMut(NavAction) -> Option<String> + '_ {
        move |action| {
            let mut registry = lock(&self.inner.inbound.registry);
            let issued = match ttl {
                Some(ttl) => registry.issue_nav_for(action, Instant::now(), ttl),
                None => registry.issue_nav(action, Instant::now()),
            };
            issued
                .map_err(|error| {
                    warn!(%error, "No random action ids; posting the card without that button");
                })
                .ok()
        }
    }

    /// A navigation button (Status, Blocked, Review, Details, Answer, Ask):
    /// already owner-filtered and audited as a click; runs the same
    /// read-only command a typed message would and posts in the thread.
    async fn handle_nav(&self, nav: Result<NavAction, NavError>, action: &OwnerAction, user: &str) {
        let thread = action
            .thread_ts
            .clone()
            .unwrap_or_else(|| action.message_ts.clone());
        let nav = match nav {
            Ok(nav) => nav,
            Err(error) => {
                let (outcome, text) = match error {
                    NavError::Expired => (
                        "refused:expired",
                        "That button expired, so nothing was run. Say `blocked` for the current prompts or `help` for fresh buttons.",
                    ),
                    NavError::AlreadyUsed => (
                        "refused:already_used",
                        "That button was already used. Say `help` for fresh buttons.",
                    ),
                };
                self.audit("button", "nav", None, outcome, user);
                self.post(&plain(text), Some(&thread)).await;
                return;
            }
        };
        let target = match &nav {
            NavAction::Answer { target, .. }
            | NavAction::Retry { target, .. }
            | NavAction::CheckAgain { target, .. } => Some(target.clone()),
            _ => None,
        };
        self.audit("button", nav.name(), target.as_ref(), "answered", user);
        let ui_url = self.inner.inbound.ui_url.clone();
        match nav {
            NavAction::AskHint => {
                self.post(&views::ask_hint(), Some(&thread)).await;
                return;
            }
            NavAction::Answer { target, title } => {
                if let Err(error) = self.post_prompt_card(&target, &title, Some(&thread)).await {
                    debug!(%error, "Reading an agent's prompt for a Slack button failed");
                    self.post(
                        &plain(&format!(
                            "*{title}*: Yard could not read its prompt. Open Yard to answer it."
                        )),
                        Some(&thread),
                    )
                    .await;
                }
                return;
            }
            NavAction::Quiet(control) => {
                self.run_control(control, &thread, None).await;
                return;
            }
            nav @ (NavAction::Retry { .. } | NavAction::CheckAgain { .. }) => {
                self.handle_ask_nav(nav, thread, user).await;
                return;
            }
            NavAction::Status
            | NavAction::Blocked
            | NavAction::Review
            | NavAction::ProjectStatus { .. } => {}
        }
        let (records, projects) = match self.durable_state().await {
            Ok(state) => state,
            Err(error) => {
                debug!(%error, "Reading Yard state for a Slack button failed");
                self.post(
                    &plain("Yard could not read its state just now; try again."),
                    Some(&thread),
                )
                .await;
                return;
            }
        };
        let reply = {
            let mut issue = self.nav_issuer();
            match nav {
                NavAction::Status => views::status_card(&records, &projects, &mut issue, &ui_url),
                NavAction::Review => views::review_card(&records, &mut issue, &ui_url),
                NavAction::ProjectStatus { project_id } => {
                    match projects.iter().find(|project| project.id == project_id) {
                        Some(project) => {
                            views::project_card(project, &records, &mut issue, &ui_url)
                        }
                        None => plain("That project is no longer visible in Yard."),
                    }
                }
                _ => {
                    drop(issue);
                    self.post_blocked(&records, &thread, user).await;
                    return;
                }
            }
        };
        self.post(&reply, Some(&thread)).await;
    }

    /// Visible projects and the attention records, read now.
    async fn durable_state(
        &self,
    ) -> Result<(yard_store::AttentionRecords, Vec<ProjectName>), String> {
        let store = self.inner.store.clone().ok_or("no store")?;
        let records = store
            .attention_records(unix_ms())
            .await
            .map_err(|error| error.to_string())?;
        let projects = store
            .list_projects()
            .await
            .map_err(|error| error.to_string())?
            .projects
            .into_iter()
            .filter(|project| records.visible_project_ids.contains(&project.id))
            .map(|project| ProjectName {
                id: project.id,
                name: project.name,
            })
            .collect();
        Ok((records, projects))
    }

    /// `blocked`: a card with answer buttons per blocked agent.
    async fn post_blocked(&self, records: &yard_store::AttentionRecords, thread: &str, user: &str) {
        let blocked = commands::blocked_agents(records);
        self.audit(
            "message",
            "blocked",
            None,
            &format!("answered:{}", blocked.len()),
            user,
        );
        let heading = {
            let mut nav = self.nav_issuer();
            views::blocked_heading(blocked.len(), &mut nav, &self.inner.inbound.ui_url)
        };
        self.post(&heading, Some(thread)).await;
        if blocked.is_empty() {
            return;
        }
        for (target, title) in blocked.iter().take(commands::MAX_BLOCKED_CARDS) {
            if let Err(error) = self.post_prompt_card(target, title, Some(thread)).await {
                debug!(%error, "Reading a blocked agent's prompt failed");
                self.post(
                    &plain(&format!(
                        "*{title}* is blocked, but Yard could not read its prompt. Open Yard to answer it."
                    )),
                    Some(thread),
                )
                .await;
            }
        }
        if blocked.len() > commands::MAX_BLOCKED_CARDS {
            let more = blocked.len() - commands::MAX_BLOCKED_CARDS;
            self.post(
                &plain(&format!("…and {more} more — open Yard.")),
                Some(thread),
            )
            .await;
        }
    }
}

impl SlackNotifier {
    /// After a notification about blocked agents, post each one's prompt
    /// card in the same thread (inbound only; in the background).
    pub(crate) fn follow_with_prompt_cards(&self, batch: &Batch, root: String) {
        if !self.inbound_enabled() || self.inner.inbound.console.get().is_none() {
            return;
        }
        let cards = batch
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    AttentionKind::Blocked | AttentionKind::ReadyForReview
                ) && matches!(event.source, EventSource::Runtime { .. })
            })
            .filter_map(|event| {
                Some((
                    event_target(event)?,
                    commands::event_title(event),
                    event.kind,
                ))
            })
            .take(commands::MAX_BLOCKED_CARDS)
            .collect::<Vec<_>>();
        if cards.is_empty() {
            return;
        }
        let notifier = self.clone();
        tokio::spawn(async move {
            for (target, title, kind) in cards {
                let posted = if kind == AttentionKind::Blocked {
                    notifier
                        .post_prompt_card(&target, &title, Some(&root))
                        .await
                } else {
                    notifier.post_question_if_asked(&target, &title).await
                };
                if let Err(error) = posted {
                    debug!(%error, "Posting an agent's prompt card failed");
                }
            }
        });
    }
}

impl SlackNotifier {
    /// Filter and answer one queued event.
    pub(crate) async fn handle_socket_event(&self, event: SocketEvent) {
        let envelope: Envelope = match event {
            SocketEvent::Hello {
                num_connections,
                app_id,
            } => {
                lock(&self.inner.inbound.filter).set_app_id(app_id);
                // Surfaced in the status API: Slack splits events between
                // the connections, so another Yard would get some of them.
                // Cleared by the next hello that reports one connection.
                let shared = (num_connections > 1).then(|| shared_warning(num_connections));
                self.set_inbound_status(InboundStatusKind::Connected, shared);
                if num_connections > 1 {
                    warn!(
                        num_connections,
                        "Slack reported more than one Socket Mode connection for this app (may be stale right after a restart; otherwise another Yard gets some events). Run inbound Slack on one Yard only"
                    );
                }
                return;
            }
            SocketEvent::Envelope(envelope) => envelope,
        };
        let Some(context) = self.inbound_context().await else {
            debug!("Slack event dropped: the bot session is not connected");
            return;
        };
        let owner = context.owner_user_id.clone();
        let verdict = {
            let mut filter = lock(&self.inner.inbound.filter);
            filter.set_context(context);
            filter.accept(&envelope, unix_ms())
        };
        let inbound = match verdict {
            Ok(inbound) => inbound,
            Err(reason) => {
                debug!(reason, "Slack event dropped");
                let user = envelope
                    .payload
                    .pointer("/event/user")
                    .or_else(|| envelope.payload.pointer("/user/id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("-");
                if reason != "duplicate" {
                    self.audit("dropped", reason, None, "dropped", user);
                }
                let late = lock(&self.inner.inbound.filter).late_owner_message(
                    &envelope,
                    reason,
                    unix_ms(),
                );
                if let Some(ts) = late {
                    self.post(&plain(LATE_MESSAGE), Some(&ts)).await;
                }
                return;
            }
        };
        let Some(console) = self.inner.inbound.console.get().cloned() else {
            self.audit("dropped", "not_ready", None, "dropped", &owner);
            return;
        };
        match inbound {
            Inbound::Action(action) => self.handle_action(console.as_ref(), action, &owner).await,
            Inbound::Message(message) => {
                self.handle_message(&console, message, &owner).await;
            }
        }
    }

    /// Shutdown: close the Socket Mode connection with a close frame (so
    /// Slack drops it at once and the next Yard's `hello` does not count
    /// it) and stop reconnecting. Waits at most `wait` for the close.
    pub async fn close_inbound(&self, wait: Duration) {
        let _ = tokio::time::timeout(wait, self.end_open_questions()).await;
        self.inner.inbound.closing.send_replace(true);
        let mut open = self.inner.inbound.socket_open.subscribe();
        let _ = tokio::time::timeout(wait, open.wait_for(|open| !*open)).await;
    }

    /// Supervised inbound loop; never returns. Off → pending forever.
    pub(crate) async fn run_inbound(self) {
        if !self.inbound_enabled() {
            if let Some(reason) = &self.inner.inbound.misconfigured {
                warn!(%reason, "Inbound Slack is misconfigured; it stays off (DM notifications continue)");
            }
            std::future::pending::<()>().await;
            return;
        }
        info!("Inbound Slack (Socket Mode) enabled");
        let (queue, mut events) = mpsc::channel(QUEUE_CAPACITY);
        let worker = self.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let notifier = worker.clone();
                if let Err(error) =
                    tokio::spawn(async move { notifier.handle_socket_event(event).await }).await
                {
                    warn!(%error, "Handling a Slack event failed; continuing");
                    tokio::time::sleep(WORKER_PANIC_BACKOFF).await;
                }
            }
        });
        let mut failures = 0_u32;
        let mut last_end: Option<SessionEnd> = None;
        loop {
            let delay = match (&last_end, failures) {
                (None, 0) => Duration::ZERO,
                (end, failures) => socket::retry_delay(end.as_ref(), failures),
            };
            let mut closing = self.inner.inbound.closing.subscribe();
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                _ = closing.wait_for(|closing| *closing) => {}
            }
            if *closing.borrow() {
                self.set_inbound_status(
                    InboundStatusKind::Off,
                    Some(session_end_message(&SessionEnd::Shutdown)),
                );
                std::future::pending::<()>().await;
            }
            let started = Instant::now();
            match self.socket_session(&queue).await {
                Ok(SessionEnd::Shutdown) => {
                    last_end = Some(SessionEnd::Shutdown);
                }
                Ok(end) => {
                    failures = socket::failures_after(failures, &end, started.elapsed());
                    if !end.immediate() {
                        self.set_inbound_status(
                            InboundStatusKind::Error,
                            Some(session_end_message(&end)),
                        );
                    }
                    last_end = Some(end);
                }
                Err((kind, message)) => {
                    // Waiting for the bot session already paused; it is
                    // not a socket failure and must not grow the backoff.
                    if message != WAITING_FOR_BOT {
                        failures = failures.saturating_add(1);
                    }
                    last_end = None;
                    self.set_inbound_status(kind, Some(message.clone()));
                    self.log_failure("inbound", &message);
                }
            }
        }
    }

    /// One connection: bot session, app token, URL, socket, read to end.
    async fn socket_session(
        &self,
        queue: &mpsc::Sender<SocketEvent>,
    ) -> Result<SessionEnd, (InboundStatusKind, String)> {
        if self.inbound_context().await.is_none() {
            tokio::time::sleep(WAIT_FOR_BOT_SESSION).await;
            return Err((InboundStatusKind::Connecting, WAITING_FOR_BOT.to_owned()));
        }
        let token = self.app_token().await.map_err(|error| {
            let kind = if error.is_misconfiguration() {
                InboundStatusKind::Misconfigured
            } else {
                InboundStatusKind::Error
            };
            (kind, format!("Reading the app-level token failed: {error}"))
        })?;
        let api = self.inner.delivery.lock().await.api.clone();
        let url = match api.open_socket_url(&token).await {
            Ok(url) => url,
            Err(error) => {
                if matches!(error, SlackError::InvalidAuth(_)) {
                    self.forget_app_token().await;
                }
                return Err((
                    InboundStatusKind::Error,
                    format!("apps.connections.open: {error}"),
                ));
            }
        };
        let mut connection = socket::connect(&url, self.inner.inbound.url_policy)
            .await
            .map_err(|error| (InboundStatusKind::Error, error.to_string()))?;
        drop(url);
        let mut stats = SessionStats::default();
        self.inner.inbound.socket_open.send_replace(true);
        let mut closing = self.inner.inbound.closing.subscribe();
        let end = socket::run_session(
            &mut connection,
            queue,
            self.inner.inbound.idle_timeout,
            &mut stats,
            &mut closing,
        )
        .await;
        let frame = socket::close_frame(&end);
        let _ = tokio::time::timeout(Duration::from_secs(2), connection.close(frame)).await;
        self.inner.inbound.socket_open.send_replace(false);
        if stats.dropped_queue_full > 0 {
            warn!(
                dropped = stats.dropped_queue_full,
                "Slack events dropped: Yard was busy"
            );
        }
        debug!(acked = stats.acked, ?end, "Socket Mode connection ended");
        Ok(end)
    }
}

/// Status text for a hello with more than one connection. Slack's count
/// can include stale sockets of a Yard that just stopped, so it is worded
/// as possibly stale.
fn shared_warning(num_connections: u64) -> String {
    format!(
        "Slack reported {num_connections} Socket Mode connections for this app at the last connect. This may be stale (a Yard that just stopped can still be counted for a minute) and clears when a later connect reports one. If it persists, another Yard is connected: events are split between them, so run inbound Slack on one Yard only"
    )
}

fn session_end_message(end: &SessionEnd) -> String {
    match end {
        SessionEnd::Disconnect(reason) if reason == "link_disabled" => {
            "Socket Mode is disabled for this Slack app (link_disabled); enable it in the app settings".to_owned()
        }
        SessionEnd::Disconnect(reason) => format!("Slack asked Yard to reconnect ({reason})"),
        SessionEnd::Closed => "The Socket Mode connection closed; reconnecting".to_owned(),
        SessionEnd::Idle => "The Socket Mode connection went quiet; reconnecting".to_owned(),
        SessionEnd::Error(error) => format!("Socket Mode connection lost: {error}; reconnecting"),
        SessionEnd::Shutdown => "Inbound Slack stopped: Yard is shutting down".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicUsize};

    use super::{WatchSlot, relay::MAX_WATCHERS};

    #[test]
    fn at_most_a_few_answers_are_watched_at_once() {
        let watchers = Arc::new(AtomicUsize::new(0));
        let mut slots = (0..MAX_WATCHERS)
            .map(|_| WatchSlot::take(&watchers, MAX_WATCHERS).expect("a free slot"))
            .collect::<Vec<_>>();
        assert!(WatchSlot::take(&watchers, MAX_WATCHERS).is_none(), "full");
        slots.pop();
        let again = WatchSlot::take(&watchers, MAX_WATCHERS).expect("a freed slot");
        drop(again);
        drop(slots);
        assert_eq!(watchers.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

//! Owner questions for the Superintendent or a project orchestrator: the
//! reachability pre-check, one progress message kept up to date, fast
//! failure detection, the watch window, late answers, Retry and Check again.
//!
//! The progress message is posted in the question's thread at once and
//! then only updated (chat.update, at most every [`RelayTiming::progress`]).
//! Only the status report for THIS command id is relayed (see
//! [`relay`]), at most once per command id.
//!
//! Never "not sent" for something that passes: a Superintendent that is
//! gone, not running or shows a lost session is started (the Slack one
//! falls back to the main one when it can't be); an agent on a prompt or
//! with its terminal open in Yard keeps the message first in the thread's
//! queue and gets it as soon as it can take it.

use std::{sync::Arc, time::Instant};

use tracing::debug;

use super::{
    AgentConsole, AgentTarget, ConsoleError, NavAction, QUEUED_NOTICE, Refusal, SlackNotifier,
    WatchSlot, lock, plain, queue_full_text, refusal_name, refusal_text, relay,
    routing::{agent_for, joined},
    views,
};
use crate::slack::{
    actions::{self, AgentHealth},
    ask,
    blocks::Style,
    conversations::{
        ConversationAgent, Delivered, Enqueued, Next, NotQueued, Queued, Retried, Rewatch, Turn,
        TurnKind, redacted,
    },
    message::OutgoingMessage,
    superintendent::{self, SLACK_SUPERINTENDENT_TO},
    unix_ms,
};

/// A cold start of the Slack Superintendent taking longer than this gets a
/// "starting" progress message first.
const STARTING_NOTICE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);
const MAIN_SUPERINTENDENT: &str = "the Superintendent";
/// The whole find / recover / start of the Slack Superintendent; past it
/// this turn falls back (the start itself goes on in the background).
const RESOLVE_TIMEOUT: std::time::Duration =
    superintendent::READY_TIMEOUT.saturating_add(std::time::Duration::from_secs(60));
/// How often a Superintendent being started is re-read until it is up.
const RECOVER_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Consecutive "not running" health reads before the watch gives up (one
/// can be a reconciliation race).
const NOT_RUNNING_READS: u32 = 2;
/// Consecutive failed pane reads before the watch gives up.
const UNREADABLE_READS: u32 = 5;
/// A message waiting for its agent is tried again every this many watch
/// polls ([`relay::RelayTiming::poll`]), and at once when a prompt is
/// answered from Slack.
const WAIT_POLLS: u32 = 4;
/// Said with the prompt card when the agent waits on a prompt.
const BLOCKED_WAIT: &str = "is waiting on a prompt; answer it below";
/// Said when the agent's terminal is open in Yard.
const BUSY_WAIT: &str = "has its terminal open in Yard";

/// One question as sent.
#[derive(Debug, Clone)]
pub(super) struct Asked {
    pub target: AgentTarget,
    /// "the Superintendent" / "the X orchestrator" (already cleaned).
    pub to: String,
    pub question: String,
    pub command_id: String,
    pub thread: String,
    pub user: String,
    /// The progress message (`None` when posting it failed).
    pub progress_ts: Option<String>,
    pub started: Instant,
    /// The Slack Superintendent's answer file key.
    pub answer_key: Option<String>,
    /// Shown with the answer (why the main Superintendent answered).
    pub note: Option<String>,
}

/// Where and how one question goes.
pub(super) struct AskRoute {
    pub target: AgentTarget,
    pub to: String,
    /// The owner's question (kept for Retry).
    pub question: String,
    /// What is sent ([`relay::question_text`] / [`relay::chat_text`]).
    pub prompt: String,
    /// Audit kind and action: `message/ask` or `reply/follow_up`.
    pub audit: (&'static str, &'static str),
    /// Audit outcome once sent: `sent`, or `sent:fallback`.
    pub sent: &'static str,
    pub answer_key: Option<String>,
    pub note: Option<String>,
    /// The Slack Superintendent's pane, held while this turn is answered
    /// (released once answered, or after [`relay::RelayTiming::busy_for`]).
    pub pane: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Start or recover the agent when it is gone, not running or shows a
    /// lost session (a Superintendent: the main one always, the Slack one
    /// when set).
    pub recover: bool,
    /// What the main Superintendent is sent when the Slack Superintendent
    /// can't be started (its route only).
    pub fallback: Option<String>,
}

/// A conversation turn in flight; ends it when dropped (watch ended,
/// refused, panicked), so the next follow-up may be sent. A turn whose
/// agent can't take it yet goes back to the front of the queue instead
/// ([`Self::wait`]).
pub(super) struct TurnGuard {
    notifier: SlackNotifier,
    root: String,
    token: u64,
    /// The message, to queue again while its agent can't take it (`None`
    /// for a turn watched again after a restart).
    queued: Option<Queued>,
    /// Set when this message already waited for its agent.
    retried: Option<Retried>,
    /// Ended by [`Self::wait`]: dropping it does nothing more.
    defused: bool,
}

impl TurnGuard {
    fn sent(&self, delivered: Delivered) {
        lock(&self.notifier.inner.inbound.conversations).sent(&self.root, self.token, delivered);
    }

    /// Queue the message again, first, tried again after `after` (or when
    /// a prompt is answered). `false` (and the turn ends as usual) when it
    /// can't be queued again.
    fn wait(mut self, progress_ts: Option<String>, after: std::time::Duration) -> bool {
        let Some(queued) = self.queued.clone() else {
            return false;
        };
        let now = unix_ms();
        let retry_at = now.saturating_add(u64::try_from(after.as_millis()).unwrap_or(u64::MAX));
        let requeued = lock(&self.notifier.inner.inbound.conversations).requeue(
            &self.root,
            self.token,
            queued,
            progress_ts,
            retry_at,
            now,
        );
        if requeued {
            self.defused = true;
            self.notifier.spawn_pump(&self.root, Some(after));
        }
        requeued
    }

    /// Keep the thread (and `pane`) held for `busy_for`, in the background:
    /// a turn sent but not watched still answers for a while.
    fn hold(self, pane: Option<tokio::sync::OwnedMutexGuard<()>>, busy_for: std::time::Duration) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            tokio::time::sleep(busy_for).await;
            drop(pane);
            drop(self);
        });
    }
}

impl Drop for TurnGuard {
    /// The turn ended: the next queued message of the thread is sent.
    fn drop(&mut self) {
        if self.defused {
            return;
        }
        lock(&self.notifier.inner.inbound.conversations).finish(&self.root, self.token);
        self.notifier.spawn_pump(&self.root, None);
    }
}

/// The relayed answer as kept for the next turn's context: redacted.
fn remembered(body: &str) -> String {
    redacted(body)
}

/// [`SlackNotifier::pump`] as a boxed future, so a turn can schedule the
/// next one without its future type containing itself.
fn pump_later(
    notifier: SlackNotifier,
    console: Arc<dyn AgentConsole>,
    root: String,
    after: Option<std::time::Duration>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        if let Some(after) = after {
            tokio::time::sleep(after).await;
        }
        notifier.pump(&console, &root).await;
    })
}

/// What [`SlackNotifier::reachable_now`] found.
enum Reach {
    Ready(actions::AgentSnapshot),
    /// A Superintendent that is gone, not running or disconnected (why):
    /// start it.
    Start(String),
    /// Not reachable now; the turn ends with a card.
    Refused(Unreached),
}

/// Why the agent can't be asked now.
enum Unreached {
    /// Running but busy (terminal open in Yard, lease to recover): the
    /// message waits for it.
    Wait { outcome: String, reason: String },
    /// Gone or not running (and not started from Slack): a card says why.
    Down { outcome: String, reason: String },
    /// Refused as the UI would (summary worker, a failed read).
    NotSent { outcome: String, reason: String },
    /// A Superintendent that could not be started.
    StartFailed(String),
}

/// How a watch ended.
enum Ended {
    Done,
    TimedOut,
}

/// A short reason for an error card.
fn refusal_reason(refusal: &Refusal) -> String {
    match refusal {
        Refusal::Busy(_) => "its terminal is open in Yard; answer it there".to_owned(),
        Refusal::Gone(reason) => format!("it is no longer running ({reason})"),
        Refusal::TerminalChanged => "its terminal changed".to_owned(),
        Refusal::Failed(_) => "Yard could not deliver the question".to_owned(),
        Refusal::Unavailable(reason) => reason.trim_end_matches('.').to_owned(),
        _ => "the question could not be delivered".to_owned(),
    }
}

/// A refusal the agent gets over by itself (terminal open in Yard, a
/// lease being recovered): the message waits instead of being refused.
fn transient(refusal: &Refusal) -> Option<String> {
    match refusal {
        Refusal::Busy(_) => Some(BUSY_WAIT.to_owned()),
        Refusal::Unavailable(reason) => Some(format!(
            "can't be typed into yet ({})",
            reason.trim_end_matches('.')
        )),
        _ => None,
    }
}

impl SlackNotifier {
    /// Post `message` as the progress message, or update it in place.
    async fn show(&self, ts: Option<&str>, message: &OutgoingMessage, thread: &str) {
        match ts {
            Some(ts) => self.update(ts, message).await,
            None => {
                self.post(message, Some(thread)).await;
            }
        }
    }

    /// Update the question's progress message; when there is none yet
    /// (posting it failed), post it and keep its ts so later updates edit
    /// that one message instead of adding more.
    async fn show_progress(&self, asked: &mut Asked, message: &OutgoingMessage) {
        if let Some(ts) = &asked.progress_ts {
            self.update(ts, message).await;
        } else {
            asked.progress_ts = self.post(message, Some(&asked.thread)).await;
            self.track(asked);
        }
    }

    /// Remember (or refresh) a watched question for [`Self::end_open_questions`].
    fn track(&self, asked: &Asked) {
        lock(&self.inner.inbound.asking).insert(asked.command_id.clone(), asked.clone());
    }

    /// Shutdown: every watched question's progress message says Yard
    /// restarted (its watch ends with this process; the next one watches
    /// a conversation's turn again).
    pub(super) async fn end_open_questions(&self) {
        let open = std::mem::take(&mut *lock(&self.inner.inbound.asking));
        for asked in open.into_values() {
            let Some(ts) = &asked.progress_ts else {
                continue;
            };
            self.audit(
                "answer",
                "relay",
                Some(&asked.target),
                "ended:restart",
                &asked.user,
            );
            let card = ask::restarted_card(&asked.to, &self.inner.inbound.ui_url);
            self.update(ts, &card).await;
        }
    }

    /// One navigation button (`None` without an id).
    fn ask_button(&self, action: NavAction, text: &str, style: Style) -> Option<serde_json::Value> {
        let mut issue = self.nav_issuer();
        views::nav_button(&mut issue, action, text, style)
    }

    fn retry_button(
        &self,
        target: &AgentTarget,
        to: &str,
        question: &str,
    ) -> Option<serde_json::Value> {
        self.ask_button(
            NavAction::Retry {
                target: target.clone(),
                to: to.to_owned(),
                question: question.to_owned(),
            },
            "Retry",
            Style::Primary,
        )
    }

    fn check_button(&self, asked: &Asked) -> Option<serde_json::Value> {
        self.ask_button(
            NavAction::CheckAgain {
                target: asked.target.clone(),
                to: asked.to.clone(),
                command_id: asked.command_id.clone(),
                progress_ts: asked.progress_ts.clone(),
                asked_at: asked.started,
                answer_key: asked.answer_key.clone(),
                note: asked.note.clone(),
            },
            "Check again",
            Style::Primary,
        )
    }

    /// The agent's snapshot when it runs in its pane (a Superintendent that
    /// is down is started first, saying so; `target` becomes the agent to
    /// ask). Else why not; nothing is shown.
    #[allow(clippy::too_many_arguments)]
    async fn reachable(
        &self,
        console: &dyn AgentConsole,
        target: &mut AgentTarget,
        to: &str,
        (progress, thread): (Option<&str>, &str),
        user: &str,
        audit: (&'static str, &'static str),
        recover: bool,
    ) -> Result<actions::AgentSnapshot, Unreached> {
        let target_now = target.clone();
        match self.reachable_now(console, &target_now, recover).await {
            Reach::Ready(snapshot) => Ok(snapshot),
            Reach::Refused(unreached) => Err(unreached),
            Reach::Start(why) => {
                self.start_superintendent(
                    console,
                    target,
                    (to, &why),
                    (progress, thread),
                    user,
                    audit,
                )
                .await
            }
        }
    }

    /// The agent's snapshot when it can be asked now: sendable, running
    /// and its terminal not showing a lost session.
    async fn ready_now(
        &self,
        console: &dyn AgentConsole,
        target: &AgentTarget,
    ) -> Option<actions::AgentSnapshot> {
        console.sendable(target).await.ok()?;
        let snapshot = console.snapshot(target).await.ok()?;
        if matches!(snapshot.health, AgentHealth::NotRunning(_)) {
            return None;
        }
        let lost = console
            .screen(target)
            .await
            .ok()
            .as_deref()
            .and_then(relay::lost_before_asking)
            .is_some();
        (!lost).then_some(snapshot)
    }

    /// Start (or recover) the Superintendent at `target`, saying so, and
    /// wait until it is up; `target` becomes the agent to ask (a new Slack
    /// Superintendent node, maybe). One start at a time: a message that
    /// waited for another one's start finds it up and is sent.
    async fn start_superintendent(
        &self,
        console: &dyn AgentConsole,
        target: &mut AgentTarget,
        (to, why): (&str, &str),
        (progress, thread): (Option<&str>, &str),
        user: &str,
        audit: (&'static str, &'static str),
    ) -> Result<actions::AgentSnapshot, Unreached> {
        let ui_url = self.inner.inbound.ui_url.clone();
        self.audit(audit.0, audit.1, Some(target), "starting", user);
        let card = ask::recovering_card(to, unix_ms(), &ui_url);
        self.show(progress, &card, thread).await;
        let _starting = self.inner.inbound.superintendent_start.lock().await;
        // Started by another message meanwhile (the Slack one may now be
        // another node: re-resolved by recovering).
        if let Some(snapshot) = self.ready_now(console, target).await {
            return Ok(snapshot);
        }
        let actor = format!("slack:owner:{user}");
        let started = tokio::time::timeout(
            RESOLVE_TIMEOUT,
            console.recover_superintendent(target, &actor),
        )
        .await
        .unwrap_or_else(|_| {
            Err(ConsoleError::Unavailable(
                "it did not start in time".to_owned(),
            ))
        });
        let failure = match started {
            Ok(now) => {
                *target = now;
                None
            }
            Err(error) => Some(error.to_string()),
        };
        let deadline = Instant::now() + superintendent::READY_TIMEOUT;
        while failure.is_none() {
            if let Some(snapshot) = self.ready_now(console, target).await {
                return Ok(snapshot);
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(RECOVER_POLL).await;
        }
        let reason = failure.unwrap_or_else(|| "it did not come up in time".to_owned());
        self.audit(audit.0, audit.1, Some(target), "failed:start", user);
        Err(Unreached::StartFailed(format!(
            "{why}; Yard could not start it: {}",
            reason.trim_end_matches('.')
        )))
    }

    /// [`Self::reachable`] without starting anything: ready, why not, or a
    /// Superintendent to start (`recover`: gone, not running, or its TUI
    /// showing a lost session).
    async fn reachable_now(
        &self,
        console: &dyn AgentConsole,
        target: &AgentTarget,
        recover: bool,
    ) -> Reach {
        // Refused as the UI would (summary worker, lease needs recovery)
        // before anything is read or sent.
        let checked = match console.sendable(target).await {
            Ok(()) => console.snapshot(target).await,
            Err(error) => Err(error),
        };
        let (outcome, reason) = match checked {
            Ok(snapshot) => match &snapshot.health {
                AgentHealth::NotRunning(reason) => ("not_running", reason.clone()),
                // Running, but its TUI may still show a lost session (the
                // process lives on after the app-server is gone).
                _ => match console
                    .screen(target)
                    .await
                    .ok()
                    .as_deref()
                    .and_then(relay::lost_before_asking)
                {
                    Some(line) => ("disconnected", format!("its terminal shows \"{line}\"")),
                    None => return Reach::Ready(snapshot),
                },
            },
            Err(ConsoleError::Gone(reason) | ConsoleError::IdentityChanged(reason)) => {
                ("gone", reason)
            }
            Err(error) => {
                let refusal = actions::console_refusal(error);
                let outcome = refusal_name(&refusal).to_owned();
                return Reach::Refused(match transient(&refusal) {
                    Some(reason) => Unreached::Wait { outcome, reason },
                    None => Unreached::NotSent {
                        outcome,
                        reason: refusal_reason(&refusal),
                    },
                });
            }
        };
        if recover {
            return Reach::Start(reason);
        }
        Reach::Refused(Unreached::Down {
            outcome: outcome.to_owned(),
            reason,
        })
    }

    /// Queue one owner message as the next turn of the conversation at
    /// `root` (it must exist) and send it when nothing is ahead of it.
    /// Never refused: while a turn is in flight the owner is told once that
    /// it is queued; only past [`crate::slack::conversations::MAX_QUEUED`]
    /// waiting messages is it not queued (and they are told so).
    pub(super) async fn converse(
        &self,
        console: &Arc<dyn AgentConsole>,
        root: &str,
        queued: Queued,
    ) {
        let (user, audit) = (queued.user.clone(), queued.kind.audit());
        let enqueued = {
            let mut conversations = lock(&self.inner.inbound.conversations);
            let now = unix_ms();
            match conversations.enqueue(root, queued.clone(), now) {
                Err(NotQueued::Unknown) => {
                    // Evicted just now: adopt it again.
                    let (agent, to) = agent_for(AgentTarget::YardOrchestrator, "");
                    conversations.adopt(root, agent, &to, None, now);
                    conversations.enqueue(root, queued, now)
                }
                other => other,
            }
        };
        match enqueued {
            Ok(Enqueued::Ready) => {}
            Ok(Enqueued::Waiting { notify }) => {
                self.audit(audit.0, audit.1, None, "queued", &user);
                if notify {
                    self.post(&plain(QUEUED_NOTICE), Some(root)).await;
                }
            }
            Err(_) => {
                self.audit(audit.0, audit.1, None, "refused:queue_full", &user);
                self.post(&plain(&queue_full_text()), Some(root)).await;
                return;
            }
        }
        self.pump(console, root).await;
    }

    /// Send the next queued message of `root`, unless a turn is in flight
    /// (its end pumps again) or it waits for its agent (tried again later).
    pub(super) async fn pump(&self, console: &Arc<dyn AgentConsole>, root: &str) {
        let next = lock(&self.inner.inbound.conversations).next(root, unix_ms());
        let Some(next) = next else {
            return;
        };
        if let Some(to) = &next.switched_to {
            let back = if matches!(next.begun.agent, ConversationAgent::SlackSuperintendent) {
                String::new()
            } else {
                " Start a message with `Superintendent:` to switch back.".to_owned()
            };
            let text = format!("Now talking to {to} in this thread.{back}");
            self.post(&plain(&text), Some(root)).await;
        }
        if next.queued.redacted && next.retried.is_none() {
            self.post(
                &plain(
                    "Part of this message was redacted when Yard saved it across a restart, so it was sent as saved. Send it again if the redacted part mattered.",
                ),
                Some(root),
            )
            .await;
        }
        self.run_turn(console, root, next).await;
    }

    /// [`Self::pump`] in its own task (now, or `after` a while).
    pub(super) fn spawn_pump(&self, root: &str, after: Option<std::time::Duration>) {
        let Some(console) = self.inner.inbound.console.get().cloned() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(pump_later(self.clone(), console, root.to_owned(), after));
    }

    /// One begun turn: the Slack Superintendent is resolved (and maybe
    /// started) off the event loop, anything else is asked at once.
    async fn run_turn(&self, console: &Arc<dyn AgentConsole>, root: &str, next: Next) {
        let Next {
            queued,
            begun,
            retried,
            ..
        } = next;
        let (question, user, audit) = (
            queued.question.clone(),
            queued.user.clone(),
            queued.kind.audit(),
        );
        let redacted_note = queued.redacted.then(|| {
            "Parts of the owner's message were redacted when Yard saved it across a restart; ask the owner if they matter.".to_owned()
        });
        let context = joined(&[begun.context.clone(), queued.note.clone(), redacted_note]);
        let progress_ts = retried
            .as_ref()
            .and_then(|retried| retried.progress_ts.clone());
        let guard = TurnGuard {
            notifier: self.clone(),
            root: root.to_owned(),
            token: begun.token,
            queued: Some(queued),
            retried,
            defused: false,
        };
        // A turn unanswered past its busy bound no longer holds the thread:
        // the next queued message goes then, even while this one is
        // still watched.
        self.spawn_pump(
            root,
            Some(self.inner.inbound.relay.busy_for() + std::time::Duration::from_millis(50)),
        );
        match begun.agent {
            ConversationAgent::Fixed(target) => {
                let recover = target == AgentTarget::YardOrchestrator;
                let route = AskRoute {
                    target,
                    to: begun.to,
                    question: question.clone(),
                    prompt: relay::chat_text(&begun.history, &question, None, context.as_deref()),
                    audit,
                    sent: "sent",
                    answer_key: None,
                    note: None,
                    pane: None,
                    recover,
                    fallback: None,
                };
                self.ask_with(console, route, root, &user, progress_ts, guard)
                    .await;
            }
            ConversationAgent::SlackSuperintendent => {
                let (notifier, console) = (self.clone(), Arc::clone(console));
                let root = root.to_owned();
                tokio::spawn(async move {
                    notifier
                        .ask_slack_superintendent(
                            &console,
                            &begun.history,
                            (&question, context.as_deref()),
                            (&root, progress_ts),
                            &user,
                            audit,
                            guard,
                        )
                        .await;
                });
            }
        }
    }

    /// Resolve the Slack Superintendent (saying so when a cold start takes
    /// a while), then ask it; when it can't start, the main Superintendent
    /// answers this turn with a note.
    #[allow(clippy::too_many_arguments)]
    async fn ask_slack_superintendent(
        &self,
        console: &Arc<dyn AgentConsole>,
        history: &[Turn],
        (question, context): (&str, Option<&str>),
        (root, mut progress_ts): (&str, Option<String>),
        user: &str,
        audit: (&'static str, &'static str),
        guard: TurnGuard,
    ) {
        // Off this task, so a hung start can't hold the turn: past
        // RESOLVE_TIMEOUT this turn falls back while the start goes on.
        let resolving = {
            let console = Arc::clone(console);
            tokio::spawn(async move { console.slack_superintendent().await })
        };
        let resolving = async move {
            match tokio::time::timeout(RESOLVE_TIMEOUT, resolving).await {
                Ok(Ok(resolved)) => resolved,
                Ok(Err(_)) => Err(ConsoleError::Unavailable("its start failed".to_owned())),
                Err(_) => Err(ConsoleError::Unavailable(
                    "it did not start in time".to_owned(),
                )),
            }
        };
        tokio::pin!(resolving);
        let resolved = if let Ok(resolved) =
            tokio::time::timeout(STARTING_NOTICE_AFTER, &mut resolving).await
        {
            resolved
        } else {
            let card = ask::starting_card(unix_ms(), &self.inner.inbound.ui_url);
            match progress_ts.as_deref() {
                Some(ts) => self.update(ts, &card).await,
                None => progress_ts = self.post(&card, Some(root)).await,
            }
            resolving.await
        };
        let main_prompt = relay::chat_text(history, question, None, context);
        let resolved = match resolved {
            Ok(Some(target)) => {
                match self.slack_superintendent_pane(root, &mut progress_ts).await {
                    Some(pane) => Ok((target, pane)),
                    None => Err(Some(
                        "it was busy with other Slack questions for too long".to_owned(),
                    )),
                }
            }
            Ok(None) => Err(None),
            Err(error) => {
                debug!(%error, "The Slack Superintendent could not take the question");
                Err(Some(error.to_string()))
            }
        };
        let route = match resolved {
            Ok((target, pane)) => {
                let answer_key = superintendent::new_answer_key();
                let file = superintendent::answer_file(&answer_key);
                AskRoute {
                    target,
                    to: SLACK_SUPERINTENDENT_TO.to_owned(),
                    question: question.to_owned(),
                    prompt: relay::chat_text(history, question, Some(&file), context),
                    audit,
                    sent: "sent",
                    answer_key: Some(answer_key),
                    note: None,
                    pane: Some(pane),
                    recover: true,
                    fallback: Some(main_prompt),
                }
            }
            Err(reason) => {
                let note = reason.as_deref().map(fallback_note);
                AskRoute {
                    target: AgentTarget::YardOrchestrator,
                    to: MAIN_SUPERINTENDENT.to_owned(),
                    question: question.to_owned(),
                    prompt: main_prompt,
                    audit,
                    sent: if note.is_some() {
                        "sent:fallback"
                    } else {
                        "sent"
                    },
                    answer_key: None,
                    note,
                    pane: None,
                    recover: true,
                    fallback: None,
                }
            }
        };
        self.ask_with(console, route, root, user, progress_ts, guard)
            .await;
    }

    /// The Slack Superintendent's pane for this turn: at once when free,
    /// else after the turns ahead of it (a "queued" progress message
    /// meanwhile). Each turn holds it at most
    /// [`relay::RelayTiming::busy_for`] once sent; `None` past that plus the
    /// start bounds (something hung): the main Superintendent answers.
    async fn slack_superintendent_pane(
        &self,
        root: &str,
        progress_ts: &mut Option<String>,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let pane = Arc::clone(&self.inner.inbound.slack_superintendent_turn);
        if let Ok(held) = Arc::clone(&pane).try_lock_owned() {
            return Some(held);
        }
        let card = ask::queued_card(unix_ms(), &self.inner.inbound.ui_url);
        match progress_ts.as_deref() {
            Some(ts) => self.update(ts, &card).await,
            None => *progress_ts = self.post(&card, Some(root)).await,
        }
        let most =
            self.inner.inbound.relay.busy_for() + RESOLVE_TIMEOUT + superintendent::READY_TIMEOUT;
        tokio::time::timeout(most, pane.lock_owned()).await.ok()
    }

    /// Acknowledge at once (or reuse `progress_ts`), check the agent is
    /// reachable and not on a prompt, send `route.prompt` (marked as from
    /// Slack) and watch for the answer. `turn` ends when the watch window
    /// does (or at once when nothing was sent); while the agent can't take
    /// the message yet it goes back to the front of the queue instead.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn ask_with(
        &self,
        console: &Arc<dyn AgentConsole>,
        route: AskRoute,
        thread: &str,
        user: &str,
        progress_ts: Option<String>,
        turn: TurnGuard,
    ) {
        let ui_url = self.inner.inbound.ui_url.clone();
        let AskRoute {
            mut target,
            mut to,
            question,
            mut prompt,
            audit,
            mut sent,
            mut answer_key,
            mut note,
            mut pane,
            recover,
            mut fallback,
        } = route;
        let mut recover = recover || target == AgentTarget::YardOrchestrator;
        // A message that waited already shows "Waiting…": left as it is
        // until it is sent.
        let progress_ts = match progress_ts {
            Some(ts) if turn.retried.is_some() => Some(ts),
            Some(ts) => {
                self.update(&ts, &ask::asking_card(&to, unix_ms(), &ui_url))
                    .await;
                Some(ts)
            }
            None => {
                self.post(&ask::asking_card(&to, unix_ms(), &ui_url), Some(thread))
                    .await
            }
        };
        let progress = progress_ts.as_deref();
        let snapshot = loop {
            let reached = self
                .reachable(
                    console.as_ref(),
                    &mut target,
                    &to,
                    (progress, thread),
                    user,
                    audit,
                    recover,
                )
                .await;
            match reached {
                Ok(snapshot) => break snapshot,
                // The Slack Superintendent could not be started: the main
                // one answers this turn, with a note.
                Err(Unreached::StartFailed(reason)) if fallback.is_some() => {
                    pane = None;
                    target = AgentTarget::YardOrchestrator;
                    MAIN_SUPERINTENDENT.clone_into(&mut to);
                    prompt = fallback.take().unwrap_or_default();
                    answer_key = None;
                    note = Some(fallback_note(&reason));
                    sent = "sent:fallback";
                    recover = true;
                }
                Err(unreached) => {
                    let card = match unreached {
                        Unreached::Wait { outcome, reason } => {
                            drop(pane);
                            self.wait_for_agent(
                                turn,
                                (&target, &to, &question),
                                (&outcome, &reason),
                                (progress, thread),
                                (user, audit),
                            )
                            .await;
                            return;
                        }
                        Unreached::Down { outcome, reason } => {
                            self.audit(
                                audit.0,
                                audit.1,
                                Some(&target),
                                &format!("refused:{outcome}"),
                                user,
                            );
                            let retry = self.retry_button(&target, &to, &question);
                            ask::unreachable_card(&to, &reason, retry, &ui_url)
                        }
                        Unreached::NotSent { outcome, reason } => {
                            self.audit(
                                audit.0,
                                audit.1,
                                Some(&target),
                                &format!("refused:{outcome}"),
                                user,
                            );
                            let retry = self.retry_button(&target, &to, &question);
                            ask::not_sent_card(&to, &reason, retry, &ui_url)
                        }
                        Unreached::StartFailed(reason) => {
                            let retry = self.retry_button(&target, &to, &question);
                            ask::unreachable_card(&to, &reason, retry, &ui_url)
                        }
                    };
                    self.show(progress, &card, thread).await;
                    return;
                }
            }
        };
        if snapshot.blocked {
            drop(pane);
            self.wait_for_agent(
                turn,
                (&target, &to, &question),
                ("blocked", BLOCKED_WAIT),
                (progress, thread),
                (user, audit),
            )
            .await;
            return;
        }
        let actor = format!("slack:owner:{user}");
        let sent_prompt = console
            .send_prompt(&target, &snapshot.identity, &prompt, &actor)
            .await;
        let command_id = match sent_prompt {
            Ok(command_id) => command_id,
            Err(error) => {
                let refusal = actions::console_refusal(error);
                if let Some(reason) = transient(&refusal) {
                    drop(pane);
                    self.wait_for_agent(
                        turn,
                        (&target, &to, &question),
                        (refusal_name(&refusal), &reason),
                        (progress, thread),
                        (user, audit),
                    )
                    .await;
                    return;
                }
                let outcome = format!("refused:{}", refusal_name(&refusal));
                self.audit(audit.0, audit.1, Some(&target), &outcome, user);
                let retry = self.retry_button(&target, &to, &question);
                let card = ask::failed_card(
                    &to,
                    &refusal_reason(&refusal),
                    std::time::Duration::ZERO,
                    retry,
                    &ui_url,
                );
                self.show(progress, &card, thread).await;
                return;
            }
        };
        turn.sent(Delivered {
            command_id: command_id.clone(),
            target: target.clone(),
            to: to.clone(),
            answer_key: answer_key.clone(),
            progress_ts: progress_ts.clone(),
            user: user.to_owned(),
            note: note.clone(),
        });
        self.audit(audit.0, audit.1, Some(&target), sent, user);
        if turn.retried.is_some()
            && let Some(ts) = progress
        {
            self.update(ts, &ask::asking_card(&to, unix_ms(), &ui_url))
                .await;
        }
        let asked = Asked {
            target,
            to,
            question,
            command_id,
            thread: thread.to_owned(),
            user: user.to_owned(),
            progress_ts,
            started: Instant::now(),
            answer_key,
            note,
        };
        self.watch_turn(console, asked, pane, turn).await;
    }

    /// The agent can't take the message yet (on a prompt, terminal open in
    /// Yard): it stays first in the thread's queue and is tried again
    /// soon, and at once when a prompt is answered from Slack. Said once
    /// (with the prompt card when it is on one). Past the watch window
    /// ([`relay::RelayTiming::follow`]) of waiting it ends with the
    /// not-sent card and Retry.
    async fn wait_for_agent(
        &self,
        turn: TurnGuard,
        (target, to, question): (&AgentTarget, &str, &str),
        (outcome, reason): (&str, &str),
        (progress, thread): (Option<&str>, &str),
        (user, audit): (&str, (&'static str, &'static str)),
    ) {
        let timing = self.inner.inbound.relay;
        let ui_url = self.inner.inbound.ui_url.clone();
        let first = turn.retried.is_none();
        let waited_ms = turn
            .retried
            .as_ref()
            .map_or(0, |retried| unix_ms().saturating_sub(retried.since_unix_ms));
        let window = u64::try_from(timing.follow.as_millis()).unwrap_or(u64::MAX);
        let after = timing.poll.saturating_mul(WAIT_POLLS);
        if waited_ms < window {
            let waiting = ask::waiting_card(to, reason, &ui_url);
            // Shown (and audited) once; later tries leave it as it is.
            let progress_ts = if first {
                match progress {
                    Some(ts) => {
                        self.update(ts, &waiting).await;
                        Some(ts.to_owned())
                    }
                    None => self.post(&waiting, Some(thread)).await,
                }
            } else {
                progress.map(str::to_owned)
            };
            if turn.wait(progress_ts, after) {
                if first {
                    self.audit(
                        audit.0,
                        audit.1,
                        Some(target),
                        &format!("waiting:{outcome}"),
                        user,
                    );
                    if outcome == "blocked"
                        && let Err(error) = self
                            .post_prompt_card(target, &ask::title(to), Some(thread))
                            .await
                    {
                        debug!(%error, "Reading the orchestrator's prompt failed");
                    }
                }
                return;
            }
        }
        // Waited a whole window (or the thread moved on): not sent.
        self.audit(
            audit.0,
            audit.1,
            Some(target),
            &format!("refused:{outcome}"),
            user,
        );
        let retry = self.retry_button(target, to, question);
        let reason = format!("it still {}", reason.split(';').next().unwrap_or(reason));
        let card = ask::not_sent_card(to, &reason, retry, &ui_url);
        self.show(progress, &card, thread).await;
    }

    /// Watch a sent turn's answer in its own task (`turn` ends with the
    /// window). Without a free watch slot the turn (and `pane`) still hold
    /// the thread for [`relay::RelayTiming::busy_for`], so the next message
    /// is not typed into an agent still answering.
    async fn watch_turn(
        &self,
        console: &Arc<dyn AgentConsole>,
        asked: Asked,
        mut pane: Option<tokio::sync::OwnedMutexGuard<()>>,
        turn: TurnGuard,
    ) {
        let ui_url = self.inner.inbound.ui_url.clone();
        let Some(slot) = WatchSlot::take(&self.inner.inbound.watchers, relay::MAX_WATCHERS) else {
            let card = ask::not_watched_card(&asked.to, self.check_button(&asked), &ui_url);
            self.show(asked.progress_ts.as_deref(), &card, &asked.thread)
                .await;
            let left = self
                .inner
                .inbound
                .relay
                .busy_for()
                .saturating_sub(asked.started.elapsed());
            turn.hold(pane, left);
            return;
        };
        let (notifier, console) = (self.clone(), Arc::clone(console));
        tokio::spawn(async move {
            let mut asked = asked;
            notifier.track(&asked);
            let ended = notifier
                .watch_answer(console.as_ref(), &mut asked, &mut pane)
                .await;
            drop(pane);
            drop(slot);
            if matches!(ended, Ended::TimedOut) {
                let late = WatchSlot::take(
                    &notifier.inner.inbound.late_watchers,
                    relay::MAX_LATE_WATCHERS,
                );
                notifier.timed_out(&mut asked, late.is_some()).await;
                // The window ended: the next follow-up may be sent (a late
                // answer is still relayed and remembered).
                drop(turn);
                if let Some(late) = late {
                    notifier.watch_late(console.as_ref(), &asked).await;
                    drop(late);
                }
            } else {
                drop(turn);
            }
            lock(&notifier.inner.inbound.asking).remove(&asked.command_id);
        });
    }

    /// After a restart: watch the answer of a turn sent before it, as the
    /// old process would have (its thread waits for it as before).
    pub(super) async fn rewatch(&self, console: Arc<dyn AgentConsole>, rewatch: Rewatch) {
        let Rewatch {
            root,
            token,
            since_unix_ms,
            question,
            delivered,
        } = rewatch;
        let turn = TurnGuard {
            notifier: self.clone(),
            root: root.clone(),
            token,
            queued: None,
            retried: None,
            defused: false,
        };
        let elapsed = std::time::Duration::from_millis(unix_ms().saturating_sub(since_unix_ms));
        let asked = Asked {
            target: delivered.target,
            to: delivered.to,
            question,
            command_id: delivered.command_id,
            thread: root,
            user: delivered.user,
            progress_ts: delivered.progress_ts,
            started: Instant::now()
                .checked_sub(elapsed)
                .unwrap_or_else(Instant::now),
            answer_key: delivered.answer_key,
            note: delivered.note,
        };
        self.audit(
            "answer",
            "relay",
            Some(&asked.target),
            "rewatch",
            &asked.user,
        );
        self.watch_turn(&console, asked, None, turn).await;
    }
}

/// Why the main Superintendent answered instead of the Slack one.
fn fallback_note(reason: &str) -> String {
    format!(
        "The Slack Superintendent could not take the question ({}), so the main Superintendent answered.",
        reason.trim_end_matches('.')
    )
}

impl SlackNotifier {
    /// Re-read the pane until the report for this command id appears (then
    /// relay it), the agent can't answer (error card with Retry), or the
    /// window ends ([`Ended::TimedOut`]; the caller says so).
    ///
    /// `pane` (the Slack Superintendent's) is released after
    /// [`relay::RelayTiming::busy_for`] even without an answer, so the
    /// next Slack question is not held for the whole window.
    async fn watch_answer(
        &self,
        console: &dyn AgentConsole,
        asked: &mut Asked,
        pane: &mut Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Ended {
        let timing = self.inner.inbound.relay;
        let ui_url = self.inner.inbound.ui_url.clone();
        let mut last_update = Instant::now();
        let (mut unreadable, mut not_running) = (0_u32, 0_u32);
        let mut state = "thinking".to_owned();
        loop {
            tokio::time::sleep(if asked.started.elapsed() < timing.wait {
                timing.poll
            } else {
                timing.follow_poll
            })
            .await;
            if pane.is_some() && asked.started.elapsed() >= timing.busy_for() {
                *pane = None;
            }
            match console.watch(&asked.target, &asked.command_id).await {
                Ok(relay::Watched {
                    report: Some(report),
                    ..
                }) => {
                    if self.relay_answer(console, asked, &report, "relayed").await {
                        return Ended::Done;
                    }
                    unreadable = 0;
                }
                Ok(relay::Watched {
                    session_lost: Some(line),
                    ..
                }) => {
                    let reason = format!("its terminal shows \"{line}\"");
                    self.ask_failed(asked, &reason, "lost:disconnected").await;
                    return Ended::Done;
                }
                Ok(_) => unreadable = 0,
                Err(ConsoleError::Gone(reason) | ConsoleError::IdentityChanged(reason)) => {
                    let reason = format!("its terminal changed or is gone ({reason})");
                    self.ask_failed(asked, &reason, "lost:terminal_changed")
                        .await;
                    return Ended::Done;
                }
                Err(error) => {
                    unreadable = unreadable.saturating_add(1);
                    debug!(%error, unreadable, "Reading an orchestrator's answer failed");
                    if unreadable >= UNREADABLE_READS {
                        self.ask_failed(asked, "Yard can't read its terminal", "lost:unreadable")
                            .await;
                        return Ended::Done;
                    }
                }
            }
            match console.snapshot(&asked.target).await {
                Ok(snapshot) => match snapshot.health {
                    AgentHealth::NotRunning(reason) => {
                        not_running += 1;
                        if not_running >= NOT_RUNNING_READS {
                            self.ask_failed(asked, &reason, "lost:not_running").await;
                            return Ended::Done;
                        }
                    }
                    health => {
                        not_running = 0;
                        state = if snapshot.blocked {
                            "waiting on a prompt (say `blocked` to answer it)".to_owned()
                        } else {
                            health.word().to_owned()
                        };
                    }
                },
                Err(ConsoleError::Gone(reason) | ConsoleError::IdentityChanged(reason)) => {
                    let reason = format!("its terminal changed or is gone ({reason})");
                    self.ask_failed(asked, &reason, "lost:terminal_changed")
                        .await;
                    return Ended::Done;
                }
                Err(error) => debug!(%error, "Reading an orchestrator's state failed"),
            }
            if asked.started.elapsed() >= timing.follow {
                return Ended::TimedOut;
            }
            if last_update.elapsed() >= timing.progress {
                last_update = Instant::now();
                let card = ask::progress_card(
                    &asked.to,
                    asked.started.elapsed(),
                    &state,
                    timing.follow,
                    &ui_url,
                );
                self.show_progress(asked, &card).await;
            }
        }
    }

    /// The window ended: say so, with "Check again".
    async fn timed_out(&self, asked: &mut Asked, watching_late: bool) {
        let timing = self.inner.inbound.relay;
        self.audit(
            "answer",
            "relay",
            Some(&asked.target),
            "timed_out",
            &asked.user,
        );
        let card = ask::timed_out_card(
            &asked.to,
            timing.follow,
            watching_late.then_some(timing.late),
            self.check_button(asked),
            &self.inner.inbound.ui_url,
        );
        self.show_progress(asked, &card).await;
    }

    /// After the window: re-read slowly so a late answer (same command
    /// id) is still relayed. It ends quietly ("Check again" and Yard
    /// remain) when the terminal is gone or changed, or after
    /// [`UNREADABLE_READS`] failed reads in a row; one failed read or a
    /// loss line (the report may still follow) does not end it.
    async fn watch_late(&self, console: &dyn AgentConsole, asked: &Asked) {
        let timing = self.inner.inbound.relay;
        let ended = Instant::now();
        let mut unreadable = 0_u32;
        while ended.elapsed() < timing.late {
            tokio::time::sleep(timing.late_poll).await;
            match console.watch(&asked.target, &asked.command_id).await {
                Ok(relay::Watched {
                    report: Some(report),
                    ..
                }) => {
                    if self
                        .relay_answer(console, asked, &report, "relayed_late")
                        .await
                    {
                        return;
                    }
                    unreadable = 0;
                }
                Ok(_) => unreadable = 0,
                Err(ConsoleError::Gone(_) | ConsoleError::IdentityChanged(_)) => return,
                Err(error) => {
                    unreadable = unreadable.saturating_add(1);
                    debug!(%error, unreadable, "Reading a late answer failed");
                    if unreadable >= UNREADABLE_READS {
                        return;
                    }
                }
            }
            if self.was_relayed(&asked.command_id) {
                return;
            }
        }
    }

    fn was_relayed(&self, command_id: &str) -> bool {
        lock(&self.inner.inbound.relayed)
            .iter()
            .any(|id| id == command_id)
    }

    /// Post the answer once per command id and mark the progress message
    /// answered. `true` once it is posted (now or earlier); `false` when
    /// posting failed: the id is released so the next read (or Check
    /// again) posts it, and the progress message is left as it was.
    async fn relay_answer(
        &self,
        console: &dyn AgentConsole,
        asked: &Asked,
        report: &yard_domain::OrchestratorStatusReport,
        outcome: &str,
    ) -> bool {
        {
            let mut relayed = lock(&self.inner.inbound.relayed);
            if relayed.iter().any(|id| *id == asked.command_id) {
                return true;
            }
            if relayed.len() >= relay::MAX_RELAYED {
                relayed.pop_front();
            }
            relayed.push_back(asked.command_id.clone());
        }
        // The Slack Superintendent's answer file, else the report's `last`.
        let file = match &asked.answer_key {
            Some(key) => console.answer_text(&asked.target, key).await,
            None => None,
        };
        let body = file.as_deref().unwrap_or(&report.last);
        let message = ask::answer_with(&asked.to, report, body, asked.note.as_deref());
        let posted = self.post(&message, Some(&asked.thread)).await;
        if posted.is_none() {
            lock(&self.inner.inbound.relayed).retain(|id| *id != asked.command_id);
            self.audit(
                "answer",
                "relay",
                Some(&asked.target),
                "post_failed",
                &asked.user,
            );
            return false;
        }
        if let Some(key) = &asked.answer_key {
            console.forget_answer(&asked.target, key).await;
        }
        lock(&self.inner.inbound.conversations).answered(&asked.command_id, &remembered(body));
        self.audit("answer", "relay", Some(&asked.target), outcome, &asked.user);
        let done = ask::answered_progress(&asked.to, asked.started.elapsed());
        if let Some(ts) = &asked.progress_ts {
            self.update(ts, &done).await;
        }
        true
    }

    /// The agent can't answer: the progress message becomes an error card.
    async fn ask_failed(&self, asked: &mut Asked, reason: &str, outcome: &str) {
        self.audit("answer", "relay", Some(&asked.target), outcome, &asked.user);
        let retry = self.retry_button(&asked.target, &asked.to, &asked.question);
        let card = ask::failed_card(
            &asked.to,
            reason,
            asked.started.elapsed(),
            retry,
            &self.inner.inbound.ui_url,
        );
        self.show_progress(asked, &card).await;
    }

    /// "Check again": re-read the latest report for that command id now.
    pub(super) async fn check_again(&self, console: &dyn AgentConsole, asked: Asked) {
        if self.was_relayed(&asked.command_id) {
            self.audit(
                "answer",
                "check_again",
                Some(&asked.target),
                "already_relayed",
                &asked.user,
            );
            let text = format!("{} already answered; see above.", ask::title(&asked.to));
            self.post(&plain(&text), Some(&asked.thread)).await;
            return;
        }
        match console.watch(&asked.target, &asked.command_id).await {
            Ok(relay::Watched {
                report: Some(report),
                ..
            }) => {
                self.relay_answer(console, &asked, &report, "relayed_on_check")
                    .await;
            }
            Ok(_) => {
                self.audit(
                    "answer",
                    "check_again",
                    Some(&asked.target),
                    "not_yet",
                    &asked.user,
                );
                let card = ask::still_nothing(
                    &asked.to,
                    self.check_button(&asked),
                    &self.inner.inbound.ui_url,
                );
                self.post(&card, Some(&asked.thread)).await;
            }
            Err(error) => {
                let refusal = actions::console_refusal(error);
                let outcome = format!("refused:{}", refusal_name(&refusal));
                self.audit(
                    "answer",
                    "check_again",
                    Some(&asked.target),
                    &outcome,
                    &asked.user,
                );
                let text = refusal_text(&ask::title(&asked.to), &refusal);
                self.post(&plain(&text), Some(&asked.thread)).await;
            }
        }
    }

    /// "Retry" (send the same question again), "Check again" and "Send it
    /// now" (a message that arrived while Yard was offline).
    pub(super) async fn handle_ask_nav(&self, nav: NavAction, thread: String, user: &str) {
        let Some(console) = self.inner.inbound.console.get().cloned() else {
            return;
        };
        match nav {
            NavAction::Retry {
                target,
                to,
                question,
            } => {
                // Retry is the thread's next turn (with its context,
                // re-resolving its agent). A thread that talks to someone
                // else now switches back to the card's agent; a thread
                // Yard does not know is adopted.
                let now = unix_ms();
                let (agent, to) = agent_for(target.clone(), &to);
                let (current, earlier) = {
                    let mut conversations = lock(&self.inner.inbound.conversations);
                    let current = conversations.agent(&thread, now);
                    if current.is_none() {
                        conversations.adopt(&thread, agent.clone(), &to, None, now);
                    }
                    (current, conversations.has_turns(&thread))
                };
                // Audited as what is retried: the first question or a
                // follow-up.
                let kind = if earlier {
                    TurnKind::FollowUp
                } else {
                    TurnKind::Ask
                };
                let wanted = match current {
                    Some((now_agent, _)) if retry_fits(&now_agent, &target) => None,
                    Some(_) => Some((agent, to)),
                    None => None,
                };
                let queued = Queued {
                    question,
                    agent: wanted,
                    user: user.to_owned(),
                    kind,
                    queued_at_unix_ms: now,
                    note: None,
                    redacted: false,
                };
                self.converse(&console, &thread, queued).await;
            }
            NavAction::CheckAgain {
                target,
                to,
                command_id,
                progress_ts,
                asked_at,
                answer_key,
                note,
            } => {
                let asked = Asked {
                    target,
                    to,
                    question: String::new(),
                    command_id,
                    thread,
                    user: user.to_owned(),
                    progress_ts,
                    started: asked_at,
                    answer_key,
                    note,
                };
                self.check_again(console.as_ref(), asked).await;
            }
            NavAction::SendLate {
                text,
                ts,
                thread_ts,
            } => {
                let message = crate::slack::inbound::OwnerMessage {
                    text,
                    ts,
                    thread_ts,
                };
                self.send_late(message, user).await;
            }
            _ => {}
        }
    }
}

/// Whether a Retry card for `target` belongs to a conversation with
/// `agent` (the Slack Superintendent's cards name its node, or the main
/// Superintendent when it answered instead).
fn retry_fits(agent: &ConversationAgent, target: &AgentTarget) -> bool {
    match agent {
        ConversationAgent::SlackSuperintendent => matches!(
            target,
            AgentTarget::YardOrchestrator | AgentTarget::CoordinationNode { .. }
        ),
        ConversationAgent::Fixed(fixed) => fixed == target,
    }
}

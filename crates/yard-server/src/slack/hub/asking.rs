//! Owner questions for the Superintendent or a project orchestrator: the
//! reachability pre-check, one progress message kept up to date, fast
//! failure detection, the watch window, late answers, Retry and Check again.
//!
//! The progress message is posted in the question's thread at once and
//! then only updated (chat.update, at most every [`RelayTiming::progress`]).
//! Only the status report for THIS command id is relayed (see
//! [`relay`]), at most once per command id.

use std::{sync::Arc, time::Instant};

use tracing::debug;

use super::{
    AgentConsole, AgentTarget, ConsoleError, NavAction, Refusal, SlackNotifier, WatchSlot, lock,
    plain, refusal_name, refusal_text, relay, views,
};
use crate::slack::{
    actions::{self, AgentHealth},
    ask,
    blocks::Style,
    message::OutgoingMessage,
    unix_ms,
};

/// Consecutive "not running" health reads before the watch gives up (one
/// can be a reconciliation race).
const NOT_RUNNING_READS: u32 = 2;
/// Consecutive failed pane reads before the watch gives up.
const UNREADABLE_READS: u32 = 5;

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
}

/// How a watch ended.
enum Ended {
    Done,
    TimedOut,
}

/// Why a question to an agent on a prompt was not sent.
const BLOCKED_REASON: &str = "it is waiting on a prompt; answer it below first";

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
    /// restarted (its watch ends with this process).
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
            },
            "Check again",
            Style::Primary,
        )
    }

    /// The agent's snapshot when it runs in its pane; else the progress
    /// message says why (with Retry) and `None`.
    #[allow(clippy::too_many_arguments)]
    async fn reachable(
        &self,
        console: &dyn AgentConsole,
        target: &AgentTarget,
        to: &str,
        question: &str,
        progress: Option<&str>,
        thread: &str,
        user: &str,
    ) -> Option<actions::AgentSnapshot> {
        // Refused as the UI would (summary worker, lease needs recovery)
        // before anything is read or sent.
        let checked = match console.sendable(target).await {
            Ok(()) => console.snapshot(target).await,
            Err(error) => Err(error),
        };
        let reason = match checked {
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
                    None => return Some(snapshot),
                },
            },
            Err(ConsoleError::Gone(reason) | ConsoleError::IdentityChanged(reason)) => {
                ("gone", reason)
            }
            Err(error) => {
                let refusal = actions::console_refusal(error);
                let outcome = format!("refused:{}", refusal_name(&refusal));
                self.audit("message", "ask", Some(target), &outcome, user);
                let retry = self.retry_button(target, to, question);
                let card = ask::not_sent_card(
                    to,
                    &refusal_reason(&refusal),
                    retry,
                    &self.inner.inbound.ui_url,
                );
                self.show(progress, &card, thread).await;
                return None;
            }
        };
        let (outcome, reason) = reason;
        self.audit(
            "message",
            "ask",
            Some(target),
            &format!("refused:{outcome}"),
            user,
        );
        let retry = self.retry_button(target, to, question);
        let card = ask::unreachable_card(to, &reason, retry, &self.inner.inbound.ui_url);
        self.show(progress, &card, thread).await;
        None
    }

    /// A free-form question: acknowledge at once, check the agent is
    /// reachable and not on a prompt, send it (marked as from Slack) and
    /// watch for the answer.
    pub(super) async fn ask(
        &self,
        console: &Arc<dyn AgentConsole>,
        target: AgentTarget,
        to: &str,
        question: &str,
        thread: &str,
        user: &str,
    ) {
        let ui_url = self.inner.inbound.ui_url.clone();
        let progress_ts = self
            .post(&ask::asking_card(to, unix_ms(), &ui_url), Some(thread))
            .await;
        let progress = progress_ts.as_deref();
        let Some(snapshot) = self
            .reachable(
                console.as_ref(),
                &target,
                to,
                question,
                progress,
                thread,
                user,
            )
            .await
        else {
            return;
        };
        if snapshot.blocked {
            self.audit("message", "ask", Some(&target), "refused:blocked", user);
            let retry = self.retry_button(&target, to, question);
            let card = ask::not_sent_card(to, BLOCKED_REASON, retry, &ui_url);
            self.show(progress, &card, thread).await;
            if let Err(error) = self
                .post_prompt_card(&target, &ask::title(to), Some(thread))
                .await
            {
                debug!(%error, "Reading the orchestrator's prompt failed");
            }
            return;
        }
        let actor = format!("slack:owner:{user}");
        let sent = console
            .send_prompt(
                &target,
                &snapshot.identity,
                &relay::question_text(question),
                &actor,
            )
            .await;
        let command_id = match sent {
            Ok(command_id) => command_id,
            Err(error) => {
                let refusal = actions::console_refusal(error);
                let outcome = format!("refused:{}", refusal_name(&refusal));
                self.audit("message", "ask", Some(&target), &outcome, user);
                let retry = self.retry_button(&target, to, question);
                let card = ask::failed_card(
                    to,
                    &refusal_reason(&refusal),
                    std::time::Duration::ZERO,
                    retry,
                    &ui_url,
                );
                self.show(progress, &card, thread).await;
                return;
            }
        };
        self.audit("message", "ask", Some(&target), "sent", user);
        let asked = Asked {
            target,
            to: to.to_owned(),
            question: question.to_owned(),
            command_id,
            thread: thread.to_owned(),
            user: user.to_owned(),
            progress_ts,
            started: Instant::now(),
        };
        let Some(slot) = WatchSlot::take(&self.inner.inbound.watchers, relay::MAX_WATCHERS) else {
            let card = ask::not_watched_card(to, self.check_button(&asked), &ui_url);
            self.show(asked.progress_ts.as_deref(), &card, thread).await;
            return;
        };
        let (notifier, console) = (self.clone(), Arc::clone(console));
        tokio::spawn(async move {
            let mut asked = asked;
            notifier.track(&asked);
            let ended = notifier.watch_answer(console.as_ref(), &mut asked).await;
            drop(slot);
            if matches!(ended, Ended::TimedOut) {
                let late = WatchSlot::take(
                    &notifier.inner.inbound.late_watchers,
                    relay::MAX_LATE_WATCHERS,
                );
                notifier.timed_out(&mut asked, late.is_some()).await;
                if let Some(late) = late {
                    notifier.watch_late(console.as_ref(), &asked).await;
                    drop(late);
                }
            }
            lock(&notifier.inner.inbound.asking).remove(&asked.command_id);
        });
    }
}

impl SlackNotifier {
    /// Re-read the pane until the report for this command id appears (then
    /// relay it), the agent can't answer (error card with Retry), or the
    /// window ends ([`Ended::TimedOut`]; the caller says so).
    async fn watch_answer(&self, console: &dyn AgentConsole, asked: &mut Asked) -> Ended {
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
            match console.watch(&asked.target, &asked.command_id).await {
                Ok(relay::Watched {
                    report: Some(report),
                    ..
                }) => {
                    if self.relay_answer(asked, &report, "relayed").await {
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
                    if self.relay_answer(asked, &report, "relayed_late").await {
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
        let posted = self
            .post(&ask::answer_message(&asked.to, report), Some(&asked.thread))
            .await;
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
                self.relay_answer(&asked, &report, "relayed_on_check").await;
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

    /// "Retry" (send the same question again) and "Check again".
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
                self.ask(&console, target, &to, &question, &thread, user)
                    .await;
            }
            NavAction::CheckAgain {
                target,
                to,
                command_id,
                progress_ts,
                asked_at,
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
                };
                self.check_again(console.as_ref(), asked).await;
            }
            _ => {}
        }
    }
}

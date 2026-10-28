//! Where an owner message goes: never refused. A top-level message starts
//! a conversation; a reply in a known thread is its next turn; a reply in
//! any other thread (a card, a notification, a conversation from before a
//! restart Yard no longer has) adopts that thread as a conversation, seeded
//! with what Yard knows about its root. `<Project>:` talks to that
//! project's orchestrator (in a thread: from then on, until
//! `Superintendent:` switches it back); anything else goes to the Slack
//! Superintendent.

use std::sync::Arc;

use super::{
    AgentConsole, AgentTarget, CONTEXT_LOOKUP_WAIT, ConversationAgent, ProjectName, Queued,
    RESUME_QUEUED_WITHIN, SlackNotifier, TurnKind, lock, plain, superintendent, unix_ms,
};
use crate::slack::{inbound::OwnerMessage, outbox::ThreadKey};

/// One owner message to route.
pub(super) struct Route {
    pub target: AgentTarget,
    pub to: String,
    pub question: String,
    /// Extra context for this turn (an ambiguous `<Project>:`); such a
    /// message goes to the Slack Superintendent.
    pub note: Option<String>,
    /// The thread's prompt card was answered or has expired.
    pub closed_card: bool,
    /// `Superintendent:`: a thread talking to a project switches back.
    pub switch_back: bool,
}

/// Who a conversation started with `target` talks to: the Slack
/// Superintendent for anything Yard-level, else that agent.
pub(super) fn agent_for(target: AgentTarget, to: &str) -> (ConversationAgent, String) {
    match target {
        AgentTarget::YardOrchestrator | AgentTarget::CoordinationNode { .. } => (
            ConversationAgent::SlackSuperintendent,
            superintendent::SLACK_SUPERINTENDENT_TO.to_owned(),
        ),
        target => (ConversationAgent::Fixed(target), to.to_owned()),
    }
}

pub(super) fn joined(parts: &[Option<String>]) -> Option<String> {
    let parts = parts.iter().flatten().cloned().collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" "))
}

impl SlackNotifier {
    pub(super) async fn route(
        &self,
        console: &Arc<dyn AgentConsole>,
        message: &OwnerMessage,
        route: Route,
        projects: &[ProjectName],
        user: &str,
    ) {
        let now = unix_ms();
        let Route {
            target,
            to,
            question,
            note,
            closed_card,
            switch_back,
        } = route;
        let queued = |agent, kind| Queued {
            question: question.clone(),
            agent,
            user: user.to_owned(),
            kind,
            queued_at_unix_ms: now,
            note: note.clone(),
            redacted: false,
        };
        let Some(root) = message.thread_ts.as_deref() else {
            let (agent, to) = agent_for(target, &to);
            lock(&self.inner.inbound.conversations).adopt(&message.ts, agent, &to, None, now);
            self.converse(console, &message.ts, queued(None, TurnKind::Ask))
                .await;
            return;
        };
        if lock(&self.inner.inbound.conversations).contains(root, now) {
            // `<Project>:` naming another agent switches the thread to it
            // (at its turn), `Superintendent:` and an ambiguous
            // `<Project>:` to the Slack Superintendent; a plain reply stays
            // with whoever it talks to.
            let wanted = if target != AgentTarget::YardOrchestrator {
                Some((ConversationAgent::Fixed(target), to.clone()))
            } else if switch_back || note.is_some() {
                Some(agent_for(AgentTarget::YardOrchestrator, ""))
            } else {
                None
            };
            self.converse(console, root, queued(wanted, TurnKind::FollowUp))
                .await;
            return;
        }
        let context = self.thread_context(root, projects, closed_card).await;
        let (agent, to) = agent_for(target.clone(), &to);
        lock(&self.inner.inbound.conversations).adopt(root, agent, &to, context, now);
        self.audit("reply", "free_text", Some(&target), "adopted_thread", user);
        self.converse(console, root, queued(None, TurnKind::FollowUp))
            .await;
    }

    /// What Yard knows about the thread rooted at `root` (never waits
    /// long: without it the thread is adopted with a generic note).
    async fn thread_context(
        &self,
        root: &str,
        projects: &[ProjectName],
        closed_card: bool,
    ) -> Option<String> {
        let key = match tokio::time::timeout(CONTEXT_LOOKUP_WAIT, self.inner.delivery.lock()).await
        {
            Ok(delivery) => delivery.session.as_ref().and_then(|session| {
                delivery
                    .threads
                    .key_of(&session.identity.team_id, &session.channel, root)
                    .map(str::to_owned)
            }),
            Err(_) => None,
        };
        let about = match key.as_deref() {
            Some(key) if key == ThreadKey::Yard.storage_key() => Some(
                "It is Yard's notification thread for the Superintendent, workstreams and digests."
                    .to_owned(),
            ),
            Some(key) => key.strip_prefix("project:").map(|project_id| {
                let name = projects
                    .iter()
                    .find(|project| project.id == project_id)
                    .map_or("(a project Yard no longer lists)", |project| {
                        project.name.as_str()
                    });
                format!(
                    "It is Yard's notification thread for the project \"{name}\" (id {project_id}): its agents' blocked, finished and failed notices are posted there."
                )
            }),
            None => None,
        };
        let card = closed_card.then(|| {
            "It held an agent's prompt card that was already answered or has expired, so this message did not go to that agent.".to_owned()
        });
        // A prompt card whose buttons still work: the owner may have meant
        // that agent; only its buttons answer it.
        let open = lock(&self.inner.inbound.registry)
            .open_card(root, std::time::Instant::now())
            .map(|title| {
                format!(
                    "It holds a prompt card for {title}, still open: that prompt is answered only with the card's buttons, so this message did not go to {title}. If the owner seems to mean that prompt, tell them to use its buttons (or say `blocked`)."
                )
            });
        let fallback = (about.is_none() && card.is_none() && open.is_none()).then(|| {
            "Yard has no record of what started it (an older conversation, a status or help card, or a message from before a restart), so there are no earlier turns.".to_owned()
        });
        joined(&[
            Some("The owner replied in an existing Slack thread.".to_owned()),
            about,
            card,
            open,
            fallback,
        ])
    }

    /// Once the socket is up after a restart (and the conversations are
    /// bound to the current owner): turns in flight at the restart are
    /// watched again, queued messages are sent, except old ones (the owner
    /// is told those were not sent).
    pub(super) fn resume_queued(&self) {
        let Some(console) = self.inner.inbound.console.get().cloned() else {
            return;
        };
        if !lock(&self.inner.inbound.conversations).is_bound()
            || self
                .inner
                .inbound
                .resumed
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let now = unix_ms();
        let resumed = lock(&self.inner.inbound.conversations).resume(now, RESUME_QUEUED_WITHIN);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for rewatch in resumed.rewatch {
            let (notifier, console) = (self.clone(), Arc::clone(&console));
            runtime.spawn(async move { notifier.rewatch(console, rewatch).await });
        }
        for root in resumed.waiting {
            // Held by a turn from before the restart: pumped when its watch
            // ends, or once it no longer holds the thread.
            let held = lock(&self.inner.inbound.conversations).held_for(&root, now);
            self.spawn_pump(
                &root,
                held.map(|left| left + std::time::Duration::from_millis(50)),
            );
        }
        if resumed.dropped.is_empty() {
            return;
        }
        let notifier = self.clone();
        runtime.spawn(async move {
            for (root, count) in resumed.dropped {
                let text = format!(
                    "Yard restarted before it could send {count} queued message{} in this thread, and too long ago to send now. Send {} again if it still matters.",
                    if count == 1 { "" } else { "s" },
                    if count == 1 { "it" } else { "them" },
                );
                notifier.post(&plain(&text), Some(&root)).await;
            }
        });
    }
}

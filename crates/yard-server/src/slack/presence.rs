//! Which agents the owner is looking at in a browser right now.
//!
//! Two signals, both provenance rather than authentication:
//! - an open terminal WebSocket (the upgrade already requires a loopback
//!   `Origin`), counted for as long as the socket lives;
//! - a presence heartbeat from an open chat view
//!   (`PUT /api/v1/integrations/slack/presence`, same-origin browser requests
//!   only), counted for [`RECENT_VIEW_TTL`]; the page repeats it every
//!   20 s while the chat is open and the tab is visible.
//!
//! Terminal-output reads do not count: the web's background status poll reads
//! every project orchestrator, which would mark all of them as viewed.
//!
//! A notification whose agent is being viewed is suppressed, like Herdr
//! suppressing toasts for the active tab.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long one chat heartbeat keeps an agent "in view" (three missed
/// 20 s heartbeats end the view).
pub const RECENT_VIEW_TTL: Duration = Duration::from_secs(60);

/// The Yard surface a browser can have open.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum ViewTarget {
    Assignment(String),
    ProjectOrchestrator(String),
    YardOrchestrator,
    CoordinationNode(String),
}

#[derive(Clone, Default)]
pub struct ViewerPresence {
    inner: Arc<Mutex<PresenceState>>,
}

#[derive(Default)]
struct PresenceState {
    open: HashMap<ViewTarget, usize>,
    recent: HashMap<ViewTarget, Instant>,
}

/// Holds one open view until dropped.
pub struct PresenceGuard {
    inner: Arc<Mutex<PresenceState>>,
    target: ViewTarget,
}

impl ViewerPresence {
    /// Count an open terminal for `target` until the guard drops.
    #[must_use]
    pub fn open(&self, target: ViewTarget) -> PresenceGuard {
        *self.lock().open.entry(target.clone()).or_default() += 1;
        PresenceGuard {
            inner: Arc::clone(&self.inner),
            target,
        }
    }

    /// Record a chat heartbeat for `target`.
    pub fn touch(&self, target: ViewTarget, now: Instant) {
        let mut state = self.lock();
        state
            .recent
            .retain(|_, seen| now.saturating_duration_since(*seen) < RECENT_VIEW_TTL);
        state.recent.insert(target, now);
    }

    #[must_use]
    pub fn is_viewing(&self, target: &ViewTarget, now: Instant) -> bool {
        let state = self.lock();
        state.open.get(target).is_some_and(|count| *count > 0)
            || state
                .recent
                .get(target)
                .is_some_and(|seen| now.saturating_duration_since(*seen) < RECENT_VIEW_TTL)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PresenceState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for PresenceGuard {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = state.open.get_mut(&self.target) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.open.remove(&self.target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{RECENT_VIEW_TTL, ViewTarget, ViewerPresence};

    #[test]
    fn open_terminals_count_until_every_guard_drops() {
        let presence = ViewerPresence::default();
        let now = Instant::now();
        let target = ViewTarget::Assignment("a-1".to_owned());
        let first = presence.open(target.clone());
        let second = presence.open(target.clone());
        assert!(presence.is_viewing(&target, now));
        assert!(!presence.is_viewing(&ViewTarget::Assignment("a-2".to_owned()), now));
        drop(first);
        assert!(presence.is_viewing(&target, now));
        drop(second);
        assert!(!presence.is_viewing(&target, now));
    }

    #[test]
    fn recent_reads_expire() {
        let presence = ViewerPresence::default();
        let now = Instant::now();
        presence.touch(ViewTarget::YardOrchestrator, now);
        assert!(presence.is_viewing(&ViewTarget::YardOrchestrator, now));
        assert!(presence.is_viewing(
            &ViewTarget::YardOrchestrator,
            now + RECENT_VIEW_TTL.saturating_sub(Duration::from_millis(1))
        ));
        assert!(!presence.is_viewing(&ViewTarget::YardOrchestrator, now + RECENT_VIEW_TTL));
    }
}

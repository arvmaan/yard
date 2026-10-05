//! In-memory send queue: a global rate limit and burst digests.
//!
//! At most one message every [`GLOBAL_MIN_INTERVAL`] (Slack allows about one
//! per second per channel; Yard is far stricter so a burst cannot flood a
//! phone). Everything that becomes ready inside one interval goes out as a
//! single message: in the project's thread when all of it belongs to one
//! project, otherwise as one portfolio digest. The queue is bounded, events
//! older than [`MAX_EVENT_AGE`] are dropped while Slack is unreachable, and
//! drops are reported as a count instead of a flood.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use super::detector::AttentionEvent;

pub const GLOBAL_MIN_INTERVAL: Duration = Duration::from_secs(10);
pub const MAX_QUEUED: usize = 50;
pub const MAX_EVENT_AGE: Duration = Duration::from_secs(60 * 60);

/// The DM thread a message belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThreadKey {
    Project(String),
    /// Superintendent, workstreams, project-less commands and digests.
    Yard,
}

impl ThreadKey {
    #[must_use]
    pub fn storage_key(&self) -> String {
        match self {
            Self::Project(project_id) => format!("project:{project_id}"),
            Self::Yard => "yard".to_owned(),
        }
    }
}

#[derive(Debug)]
pub struct Batch {
    pub events: Vec<AttentionEvent>,
    /// Events dropped (queue overflow or age) since the last message.
    pub dropped: usize,
    pub thread: ThreadKey,
}

#[derive(Debug)]
pub struct Outbox {
    queue: VecDeque<AttentionEvent>,
    next_send_at: Option<Instant>,
    dropped: usize,
    min_interval: Duration,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::with_interval(GLOBAL_MIN_INTERVAL)
    }
}

impl Outbox {
    #[must_use]
    pub fn with_interval(min_interval: Duration) -> Self {
        Self {
            queue: VecDeque::new(),
            next_send_at: None,
            dropped: 0,
            min_interval,
        }
    }

    pub fn push(&mut self, events: impl IntoIterator<Item = AttentionEvent>) {
        for event in events {
            if self.queue.len() >= MAX_QUEUED {
                self.queue.pop_front();
                self.dropped += 1;
            }
            self.queue.push_back(event);
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.dropped == 0
    }

    /// Take everything sendable now. `keep` re-checks each event (still true,
    /// not being viewed); events that fail it resolved on their own.
    pub fn next_batch(
        &mut self,
        now: Instant,
        mut keep: impl FnMut(&AttentionEvent) -> bool,
    ) -> Option<Batch> {
        if self.next_send_at.is_some_and(|next| now < next) {
            return None;
        }
        let mut events = Vec::with_capacity(self.queue.len());
        for event in self.queue.drain(..) {
            if now.saturating_duration_since(event.ready_at) >= MAX_EVENT_AGE {
                self.dropped += 1;
            } else if keep(&event) {
                events.push(event);
            }
        }
        if events.is_empty() && self.dropped == 0 {
            return None;
        }
        let thread = match events.first().and_then(|event| event.project_id.as_ref()) {
            Some(project_id)
                if self.dropped == 0
                    && events
                        .iter()
                        .all(|event| event.project_id.as_ref() == Some(project_id)) =>
            {
                ThreadKey::Project(project_id.clone())
            }
            _ => ThreadKey::Yard,
        };
        Some(Batch {
            events,
            dropped: std::mem::take(&mut self.dropped),
            thread,
        })
    }

    /// Everything queued, oldest first, and the dropped count (quiet hours
    /// began: the policy holds it instead, so nothing posts a lone "N
    /// dropped" message while quiet).
    pub fn take_all(&mut self) -> (Vec<AttentionEvent>, usize) {
        (
            self.queue.drain(..).collect(),
            std::mem::take(&mut self.dropped),
        )
    }

    /// Whether the global interval allows a message now.
    #[must_use]
    pub fn can_send(&self, now: Instant) -> bool {
        self.next_send_at.is_none_or(|next| now >= next)
    }

    /// Wait until `not_before` (a digest post failed).
    pub const fn hold_off(&mut self, not_before: Instant) {
        self.next_send_at = Some(not_before);
    }

    /// A message went out; hold the next one for the global interval.
    pub fn sent(&mut self, now: Instant) {
        self.next_send_at = Some(now + self.min_interval);
    }

    /// Delivery failed: put the batch back (oldest first) and wait.
    pub fn retry(&mut self, batch: Batch, not_before: Instant) {
        self.dropped += batch.dropped;
        for event in batch.events.into_iter().rev() {
            self.queue.push_front(event);
        }
        while self.queue.len() > MAX_QUEUED {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.next_send_at = Some(not_before);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use yard_domain::ObservedStatus;

    use super::{GLOBAL_MIN_INTERVAL, MAX_EVENT_AGE, MAX_QUEUED, Outbox, ThreadKey};
    use crate::slack::detector::{AttentionEvent, AttentionKind, EventSource, Subject};

    fn event(project: Option<&str>, ready_at: Instant) -> AttentionEvent {
        AttentionEvent {
            kind: AttentionKind::Blocked,
            source: EventSource::Runtime {
                worker_id: format!("w-{}", project.unwrap_or("yard")),
                status: ObservedStatus::Blocked,
            },
            project_id: project.map(str::to_owned),
            project_name: project.map(str::to_owned),
            subject: Subject::ProjectOrchestrator,
            view_target: None,
            observed_at_unix_ms: 0,
            ready_at,
            automatic: false,
        }
    }

    #[test]
    fn one_message_per_interval_and_bursts_become_one_digest() {
        let now = Instant::now();
        let mut outbox = Outbox::default();
        outbox.push([event(Some("p-1"), now)]);
        let first = outbox.next_batch(now, |_| true).unwrap();
        assert_eq!(first.thread, ThreadKey::Project("p-1".to_owned()));
        assert_eq!(first.events.len(), 1);
        outbox.sent(now);

        outbox.push([
            event(Some("p-1"), now),
            event(Some("p-2"), now),
            event(None, now),
        ]);
        assert!(
            outbox
                .next_batch(
                    now + GLOBAL_MIN_INTERVAL.saturating_sub(Duration::from_millis(1)),
                    |_| { true }
                )
                .is_none()
        );
        let digest = outbox
            .next_batch(now + GLOBAL_MIN_INTERVAL, |_| true)
            .unwrap();
        assert_eq!(digest.thread, ThreadKey::Yard);
        assert_eq!(digest.events.len(), 3);
        assert!(outbox.is_empty());
    }

    #[test]
    fn same_project_bursts_stay_in_the_project_thread() {
        let now = Instant::now();
        let mut outbox = Outbox::default();
        outbox.push([event(Some("p-1"), now), event(Some("p-1"), now)]);
        let batch = outbox.next_batch(now, |_| true).unwrap();
        assert_eq!(batch.thread, ThreadKey::Project("p-1".to_owned()));
        assert_eq!(batch.events.len(), 2);
    }

    #[test]
    fn resolved_events_are_dropped_silently_and_stale_ones_are_counted() {
        let now = Instant::now();
        let mut outbox = Outbox::default();
        outbox.push([event(Some("p-1"), now)]);
        assert!(outbox.next_batch(now, |_| false).is_none());

        outbox.push([event(Some("p-1"), now)]);
        let later = now + MAX_EVENT_AGE;
        let batch = outbox.next_batch(later, |_| true).unwrap();
        assert!(batch.events.is_empty());
        assert_eq!(batch.dropped, 1);
        assert_eq!(batch.thread, ThreadKey::Yard);
    }

    #[test]
    fn take_all_hands_over_the_dropped_count_so_nothing_posts_while_quiet() {
        let now = Instant::now();
        let mut outbox = Outbox::default();
        outbox.push((0..MAX_QUEUED + 3).map(|_| event(Some("p-1"), now)));
        let (events, dropped) = outbox.take_all();
        assert_eq!(events.len(), MAX_QUEUED);
        assert_eq!(dropped, 3);
        // No lone "3 older updates were dropped" message during quiet hours.
        assert!(outbox.next_batch(now, |_| true).is_none());
        assert!(outbox.is_empty());
    }

    #[test]
    fn overflow_and_failed_delivery_keep_the_queue_bounded() {
        let now = Instant::now();
        let mut outbox = Outbox::default();
        outbox.push((0..MAX_QUEUED + 5).map(|_| event(Some("p-1"), now)));
        let batch = outbox.next_batch(now, |_| true).unwrap();
        assert_eq!(batch.events.len(), MAX_QUEUED);
        assert_eq!(batch.dropped, 5);

        outbox.retry(batch, now + Duration::from_secs(30));
        assert!(
            outbox
                .next_batch(now + Duration::from_secs(29), |_| true)
                .is_none()
        );
        let retried = outbox
            .next_batch(now + Duration::from_secs(30), |_| true)
            .unwrap();
        assert_eq!(retried.events.len(), MAX_QUEUED);
        assert_eq!(retried.dropped, 5);
    }
}

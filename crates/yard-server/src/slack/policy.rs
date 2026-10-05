//! The quiet policy between detection and posting: it decides, for each
//! proactive notification, whether it goes out now, waits for a digest, or
//! is dropped as noise. Replies to the owner's own messages and clicks never
//! pass through here (they are posted by [`super::hub`] directly).
//!
//! Rules (all times from an injected clock):
//! - Outside working hours or while muted, everything is held in
//!   `slack-held.json`; the next working window opens with ONE "While you
//!   were away" digest.
//! - Inside working hours, only [`InstantKinds`] go out at once (default:
//!   blocked); the rest wait for a digest that runs at most every
//!   `digest_every`, timed from the first new item.
//! - Noise control: no repeat DM for the same worker and kind within
//!   [`NoiseConfig::debounce`] unless the prompt fingerprint changed; a
//!   re-block on the same prompt within [`NoiseConfig::flap_window`] of the
//!   last DM counts once (dropped); an instant DM replaces a held line for
//!   the same worker and kind; at most [`NoiseConfig::hourly_cap`] instant
//!   DMs per rolling hour (overflow waits for the digest); events caused by
//!   Yard's automatic summary prompts never notify.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    detector::{AttentionEvent, EventSource},
    held::{HeldEntry, HeldState, JsonFile, MAX_HELD, Preferences},
    quiet::{InstantKinds, QuietConfig},
};

/// The wall clock, injectable for tests.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(Utc::now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoiseConfig {
    pub debounce: Duration,
    pub flap_window: Duration,
    pub hourly_cap: usize,
}

impl Default for NoiseConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(30 * 60),
            flap_window: Duration::from_secs(2 * 60),
            hourly_cap: 6,
        }
    }
}

impl NoiseConfig {
    /// No debounce, flap window or cap (tests about something else).
    #[must_use]
    pub const fn off() -> Self {
        Self {
            debounce: Duration::ZERO,
            flap_window: Duration::ZERO,
            hourly_cap: usize::MAX,
        }
    }
}

/// What the policy did with one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Post it now.
    Instant,
    /// Kept for a digest.
    Held,
    Dropped(DropReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    Automatic,
    Debounced,
}

/// Which digest is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestKind {
    /// The first one of a working window, for what was held while quiet.
    Away,
    /// In working hours, for what did not go out instantly.
    Periodic,
    /// The owner asked (`digest`); sent even outside working hours.
    Requested,
}

/// `quiet` in `GET /api/v1/integrations/slack` (unix ms).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuietStatusView {
    /// Quiet hours or a mute hold everything right now.
    pub active: bool,
    /// When the current quiet stretch ends.
    pub until: Option<u64>,
    /// When the held notifications are due (none held: `null`).
    pub next_delivery_at: Option<u64>,
    pub held_count: usize,
    pub muted_until: Option<u64>,
}

/// Construction seams (production: env config, system clock, files beside
/// the database).
#[derive(Clone)]
pub struct QuietParts {
    pub config: QuietConfig,
    pub noise: NoiseConfig,
    pub clock: Clock,
    pub held_file: Option<PathBuf>,
    pub preferences_file: Option<PathBuf>,
}

impl QuietParts {
    /// Always deliverable, every kind instant, no noise control, in memory.
    #[must_use]
    pub fn always() -> Self {
        Self {
            config: QuietConfig::always(),
            noise: NoiseConfig::off(),
            clock: system_clock(),
            held_file: None,
            preferences_file: None,
        }
    }
}

pub struct Policy {
    config: QuietConfig,
    noise: NoiseConfig,
    clock: Clock,
    held: JsonFile<HeldState>,
    preferences: JsonFile<Preferences>,
    /// Instant DMs in the last hour.
    instant_log: VecDeque<DateTime<Utc>>,
    /// Last instant DM per key and its prompt fingerprint.
    last_dm: HashMap<String, (DateTime<Utc>, Option<String>)>,
    requested: bool,
}

/// The debounce key: one worker and kind, or one command.
#[must_use]
pub fn event_key(event: &AttentionEvent) -> String {
    match &event.source {
        EventSource::Runtime { worker_id, .. } => format!("worker:{worker_id}:{:?}", event.kind),
        EventSource::Command { command_id, .. } => format!("command:{command_id}"),
    }
}

fn unix_ms(at: DateTime<Utc>) -> u64 {
    u64::try_from(at.timestamp_millis()).unwrap_or_default()
}

fn from_unix_ms(ms: u64) -> DateTime<Utc> {
    i64::try_from(ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .unwrap_or_default()
}

fn within(earlier: DateTime<Utc>, now: DateTime<Utc>, span: Duration) -> bool {
    !span.is_zero()
        && now
            .signed_duration_since(earlier)
            .to_std()
            .is_ok_and(|elapsed| elapsed < span)
}

impl Policy {
    #[must_use]
    pub fn new(parts: QuietParts) -> Self {
        Self {
            config: parts.config,
            noise: parts.noise,
            clock: parts.clock,
            held: JsonFile::load(parts.held_file),
            preferences: JsonFile::load(parts.preferences_file),
            instant_log: VecDeque::new(),
            last_dm: HashMap::new(),
            requested: false,
        }
    }

    #[must_use]
    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    #[must_use]
    pub const fn config(&self) -> &QuietConfig {
        &self.config
    }

    #[must_use]
    pub const fn instant_kinds(&self) -> InstantKinds {
        self.config.instant
    }

    #[must_use]
    pub fn muted_until(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.preferences
            .value
            .muted_until_unix_ms
            .map(from_unix_ms)
            .filter(|until| *until > now)
    }

    /// Quiet hours or a mute: nothing goes out on its own.
    #[must_use]
    pub fn holding(&self, now: DateTime<Utc>) -> bool {
        self.muted_until(now).is_some() || !self.config.in_window(now)
    }

    /// The first moment at or after `at` that is in working hours and not
    /// muted (`None` when no day is a working day).
    #[must_use]
    pub fn deliverable_at(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let after_mute = self.muted_until(at).map_or(at, |until| until.max(at));
        let window = self.config.next_window(after_mute)?;
        Some(window.start.max(after_mute))
    }

    pub fn mute_until(&mut self, until: DateTime<Utc>) {
        self.preferences.value.muted_until_unix_ms = Some(unix_ms(until));
        self.preferences.save();
    }

    pub fn unmute(&mut self) {
        self.preferences.value.muted_until_unix_ms = None;
        self.preferences.save();
    }

    /// The owner asked for the digest now (sent even outside hours).
    pub const fn request_digest(&mut self) {
        self.requested = true;
    }

    #[must_use]
    pub fn held_count(&self) -> usize {
        self.held.value.entries.len()
    }

    /// Decide one event. `fingerprint` identifies the prompt on screen
    /// (when it could be read); `None` counts as unchanged.
    pub fn admit(&mut self, event: &AttentionEvent, fingerprint: Option<&str>) -> Admission {
        let now = self.now();
        if event.automatic {
            return Admission::Dropped(DropReason::Automatic);
        }
        let key = event_key(event);
        if self.holding(now) {
            self.hold(event.clone(), key, now, true);
            return Admission::Held;
        }
        if !self.config.instant.allows(event.kind) {
            self.hold(event.clone(), key, now, false);
            return Admission::Held;
        }
        if let Some((at, previous)) = self.last_dm.get(&key) {
            // A new prompt is a new question; the same state again within
            // the flap window or the debounce was already announced (a
            // flap counts once).
            let changed = fingerprint.is_some() && previous.as_deref() != fingerprint;
            let quiet_for = self.noise.debounce.max(self.noise.flap_window);
            if within(*at, now, quiet_for) && !changed {
                return Admission::Dropped(DropReason::Debounced);
            }
        }
        self.instant_log
            .retain(|at| within(*at, now, Duration::from_secs(60 * 60)));
        if self.instant_log.len() >= self.noise.hourly_cap {
            self.hold(event.clone(), key, now, false);
            return Admission::Held;
        }
        self.instant_log.push_back(now);
        self.unhold(&key);
        self.last_dm
            .insert(key, (now, fingerprint.map(str::to_owned)));
        let debounce = self.noise.debounce.max(self.noise.flap_window);
        self.last_dm.retain(|_, (at, _)| within(*at, now, debounce));
        Admission::Instant
    }

    /// Hold events that were admitted but could not go out before quiet
    /// hours or a mute began (still in the send queue), and count the ones
    /// the queue dropped for the digest.
    pub fn hold_all(&mut self, events: impl IntoIterator<Item = AttentionEvent>, dropped: usize) {
        let now = self.now();
        let away = self.holding(now);
        for event in events {
            let key = event_key(&event);
            self.hold(event, key, now, away);
        }
        if dropped > 0 {
            let state = &mut self.held.value;
            state.dropped = state.dropped.saturating_add(dropped);
            self.held.save();
        }
    }

    /// An instant DM supersedes a held line for the same worker and kind.
    fn unhold(&mut self, key: &str) {
        let entries = &mut self.held.value.entries;
        let before = entries.len();
        entries.retain(|held| held.key != key);
        if entries.len() != before {
            self.held.save();
        }
    }

    fn hold(&mut self, event: AttentionEvent, key: String, now: DateTime<Utc>, away: bool) {
        let state = &mut self.held.value;
        let mut entry = HeldEntry {
            event: super::held::trimmed(event),
            key,
            held_at_unix_ms: unix_ms(now),
            away,
        };
        if let Some(index) = state.entries.iter().position(|held| held.key == entry.key) {
            // One line per worker and kind: keep the newest state, the
            // earliest hold time, and "away" if either was.
            let previous = state.entries.remove(index);
            entry.held_at_unix_ms = previous.held_at_unix_ms;
            entry.away |= previous.away;
        }
        state.entries.push(entry);
        if state.entries.len() > MAX_HELD {
            let excess = state.entries.len() - MAX_HELD;
            state.entries.drain(..excess);
            state.dropped = state.dropped.saturating_add(excess);
        }
        self.held.save();
    }
}

impl Policy {
    /// When the held notifications are due, ignoring quiet hours (`None`:
    /// nothing held).
    fn due_at(&self) -> Option<DateTime<Utc>> {
        let entries = &self.held.value.entries;
        if entries.is_empty() {
            return None;
        }
        if entries.iter().any(|entry| entry.away) {
            return Some(DateTime::<Utc>::MIN_UTC);
        }
        let first = entries.iter().map(|entry| entry.held_at_unix_ms).min()?;
        let every = chrono::Duration::from_std(self.config.digest_every).ok()?;
        Some(from_unix_ms(first) + every)
    }

    /// Which digest should go out now, if any.
    #[must_use]
    pub fn digest_due(&self) -> Option<DigestKind> {
        if self.requested {
            return Some(DigestKind::Requested);
        }
        let now = self.now();
        let due = self.due_at()?;
        if self.holding(now) || now < due {
            return None;
        }
        Some(if self.held.value.entries.iter().any(|entry| entry.away) {
            DigestKind::Away
        } else {
            DigestKind::Periodic
        })
    }

    /// Everything held, for a digest (and clear the request). It stays held
    /// (and on disk) until [`Self::digest_sent`], so a restart or an aborted
    /// post loses nothing; after a failed post call [`Self::digest_failed`].
    pub fn take_digest(&mut self) -> HeldState {
        self.requested = false;
        self.held.value.clone()
    }

    /// The digest for `taken` went out (or every item in it had resolved):
    /// forget those entries, keeping any held or changed since.
    pub fn digest_sent(&mut self, taken: &HeldState) {
        let state = &mut self.held.value;
        state.entries.retain(|entry| !taken.entries.contains(entry));
        state.dropped = state.dropped.saturating_sub(taken.dropped);
        self.held.save();
    }

    /// A digest post failed: its entries are still held; a digest the
    /// owner asked for is asked for again.
    pub const fn digest_failed(&mut self, kind: DigestKind) {
        if matches!(kind, DigestKind::Requested) {
            self.requested = true;
        }
    }

    /// `quiet` for the status endpoint.
    #[must_use]
    pub fn status(&self) -> QuietStatusView {
        let now = self.now();
        let active = self.holding(now);
        let until = if active {
            self.deliverable_at(now).map(unix_ms)
        } else {
            None
        };
        let next_delivery_at = self
            .due_at()
            .and_then(|due| self.deliverable_at(due.max(now)))
            .map(unix_ms);
        QuietStatusView {
            active,
            until,
            next_delivery_at,
            held_count: self.held_count(),
            muted_until: self.muted_until(now).map(unix_ms),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use chrono::{DateTime, Utc};
    use tempfile::TempDir;
    use yard_domain::ObservedStatus;

    use super::{Admission, DigestKind, DropReason, NoiseConfig, Policy, QuietParts};
    use crate::slack::{
        detector::{AttentionEvent, AttentionKind, EventSource, Subject},
        quiet::QuietConfig,
    };

    /// A clock the test moves by hand.
    #[derive(Clone)]
    pub(crate) struct TestClock(pub Arc<Mutex<DateTime<Utc>>>);

    impl TestClock {
        pub(crate) fn at(text: &str) -> Self {
            Self(Arc::new(Mutex::new(
                DateTime::parse_from_rfc3339(text)
                    .unwrap()
                    .with_timezone(&Utc),
            )))
        }

        pub(crate) fn set(&self, text: &str) {
            *self.0.lock().unwrap() = DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc);
        }

        pub(crate) fn advance(&self, by: Duration) {
            let mut now = self.0.lock().unwrap();
            *now += chrono::Duration::from_std(by).unwrap();
        }

        pub(crate) fn clock(&self) -> super::Clock {
            let inner = Arc::clone(&self.0);
            Arc::new(move || *inner.lock().unwrap())
        }
    }

    /// Thursday 2026-10-01 10:00 PDT.
    pub(crate) const WORKDAY_MORNING: &str = "2026-10-01T17:00:00Z";
    /// Thursday 2026-10-01 20:30 PDT.
    pub(crate) const WORKDAY_EVENING: &str = "2026-10-02T03:30:00Z";
    /// Friday 2026-10-02 09:00 PDT.
    pub(crate) const NEXT_MORNING: &str = "2026-10-02T16:00:00Z";

    pub(crate) fn parts(clock: &TestClock, dir: Option<&TempDir>) -> QuietParts {
        QuietParts {
            config: QuietConfig::default(),
            noise: NoiseConfig::default(),
            clock: clock.clock(),
            held_file: dir.map(|dir| dir.path().join("slack-held.json")),
            preferences_file: dir.map(|dir| dir.path().join("slack-preferences.json")),
        }
    }

    pub(crate) fn event(worker: &str, kind: AttentionKind) -> AttentionEvent {
        AttentionEvent {
            kind,
            source: EventSource::Runtime {
                worker_id: worker.to_owned(),
                status: match kind {
                    AttentionKind::Blocked => ObservedStatus::Blocked,
                    _ => ObservedStatus::Done,
                },
            },
            project_id: Some("p-1".to_owned()),
            project_name: Some("Checkout".to_owned()),
            subject: Subject::ProjectOrchestrator,
            view_target: None,
            observed_at_unix_ms: 1,
            ready_at: Instant::now(),
            automatic: false,
        }
    }

    #[test]
    fn in_hours_blocked_is_instant_and_the_rest_is_digested_every_two_hours() {
        let clock = TestClock::at(WORKDAY_MORNING);
        let mut policy = Policy::new(parts(&clock, None));
        assert_eq!(
            policy.admit(&event("w-1", AttentionKind::Blocked), None),
            Admission::Instant
        );
        assert_eq!(
            policy.admit(&event("w-2", AttentionKind::ReadyForReview), None),
            Admission::Held
        );
        clock.advance(Duration::from_secs(30 * 60));
        assert_eq!(
            policy.admit(&event("w-3", AttentionKind::CommandFailed), None),
            Admission::Held
        );
        assert_eq!(policy.digest_due(), None);
        // Timed from the first new item, not the latest.
        clock.advance(Duration::from_secs(89 * 60));
        assert_eq!(policy.digest_due(), None);
        clock.advance(Duration::from_secs(60));
        assert_eq!(policy.digest_due(), Some(DigestKind::Periodic));
        let taken = policy.take_digest();
        assert_eq!(taken.entries.len(), 2);
        policy.digest_sent(&taken);
        assert_eq!(policy.digest_due(), None);

        // The next digest waits two hours from its own first item.
        clock.advance(Duration::from_secs(5 * 60));
        policy.admit(&event("w-2", AttentionKind::ReadyForReview), None);
        clock.advance(Duration::from_secs(119 * 60));
        assert_eq!(policy.digest_due(), None);
        clock.advance(Duration::from_secs(60));
        assert_eq!(policy.digest_due(), Some(DigestKind::Periodic));
    }

    #[test]
    fn a_digest_due_after_six_waits_for_the_morning() {
        let clock = TestClock::at("2026-10-02T00:00:00Z"); // Thu 17:00 PDT
        let mut policy = Policy::new(parts(&clock, None));
        policy.admit(&event("w-2", AttentionKind::ReadyForReview), None);
        clock.set("2026-10-02T02:30:00Z"); // 19:30 PDT, two and a half hours later
        assert_eq!(policy.digest_due(), None);
        let status = policy.status();
        assert!(status.active);
        assert_eq!(
            status.next_delivery_at,
            Some(
                u64::try_from(
                    DateTime::parse_from_rfc3339(NEXT_MORNING)
                        .unwrap()
                        .timestamp_millis()
                )
                .unwrap()
            )
        );
        clock.set(NEXT_MORNING);
        assert_eq!(policy.digest_due(), Some(DigestKind::Periodic));
    }

    #[test]
    fn quiet_hours_hold_everything_across_a_restart_and_the_morning_gets_one_digest() {
        let temp = TempDir::new().unwrap();
        let clock = TestClock::at(WORKDAY_EVENING);
        let mut policy = Policy::new(parts(&clock, Some(&temp)));
        assert_eq!(
            policy.admit(&event("w-1", AttentionKind::Blocked), None),
            Admission::Held
        );
        policy.admit(&event("w-2", AttentionKind::ReadyForReview), None);
        // The same worker and kind again is one line, not two.
        policy.admit(&event("w-2", AttentionKind::ReadyForReview), None);
        assert_eq!(policy.held_count(), 2);
        assert_eq!(policy.digest_due(), None);
        let status = policy.status();
        assert!(status.active);
        assert_eq!(status.held_count, 2);
        assert_eq!(status.until, status.next_delivery_at);
        drop(policy);

        let file = std::fs::read_to_string(temp.path().join("slack-held.json")).unwrap();
        assert!(file.contains("\"version\": 1"), "{file}");
        let reloaded = Policy::new(parts(&clock, Some(&temp)));
        assert_eq!(reloaded.held_count(), 2);
        assert_eq!(reloaded.digest_due(), None);
        let mut reloaded = reloaded;
        clock.set("2026-10-02T15:59:00Z");
        assert_eq!(reloaded.digest_due(), None);
        clock.set(NEXT_MORNING);
        assert_eq!(reloaded.digest_due(), Some(DigestKind::Away));
        let taken = reloaded.take_digest();
        assert_eq!(taken.entries.len(), 2);
        assert!(taken.entries.iter().all(|entry| entry.away));
        // A post that fails (or a quit mid-post) loses nothing: still held,
        // in memory and on disk.
        reloaded.digest_failed(DigestKind::Away);
        assert_eq!(reloaded.digest_due(), Some(DigestKind::Away));
        assert_eq!(Policy::new(parts(&clock, Some(&temp))).held_count(), 2);

        // Sent: once. Something held during the post stays for the next one.
        let taken = reloaded.take_digest();
        reloaded.admit(&event("w-3", AttentionKind::ReadyForReview), None);
        reloaded.digest_sent(&taken);
        assert_eq!(reloaded.held_count(), 1);
        assert_eq!(reloaded.digest_due(), None);
        assert_eq!(Policy::new(parts(&clock, Some(&temp))).held_count(), 1);
    }

    #[test]
    fn weekend_holds_until_monday_nine() {
        let clock = TestClock::at("2026-10-03T19:00:00Z"); // Saturday noon PDT
        let mut policy = Policy::new(parts(&clock, None));
        assert_eq!(
            policy.admit(&event("w-1", AttentionKind::Blocked), None),
            Admission::Held
        );
        clock.set("2026-10-04T23:00:00Z"); // Sunday
        assert_eq!(policy.digest_due(), None);
        clock.set("2026-10-05T16:00:00Z"); // Monday 09:00 PDT
        assert_eq!(policy.digest_due(), Some(DigestKind::Away));
    }

    #[test]
    fn debounce_flapping_and_the_hourly_cap() {
        let clock = TestClock::at(WORKDAY_MORNING);
        let mut policy = Policy::new(parts(&clock, None));
        let blocked = || event("w-1", AttentionKind::Blocked);
        assert_eq!(policy.admit(&blocked(), Some("a")), Admission::Instant);
        // blocked → working → blocked on the same prompt a minute later:
        // counts once (no DM, and no digest line later either).
        clock.advance(Duration::from_secs(60));
        assert_eq!(
            policy.admit(&blocked(), Some("a")),
            Admission::Dropped(DropReason::Debounced)
        );
        assert_eq!(policy.held_count(), 0);
        // A new question a minute after the last DM goes out now, not in
        // two hours.
        assert_eq!(policy.admit(&blocked(), Some("b")), Admission::Instant);
        clock.advance(Duration::from_secs(60));
        assert_eq!(
            policy.admit(&blocked(), Some("b")),
            Admission::Dropped(DropReason::Debounced)
        );
        // Same prompt within 30 minutes: no repeat DM.
        clock.advance(Duration::from_secs(10 * 60));
        assert_eq!(
            policy.admit(&blocked(), Some("b")),
            Admission::Dropped(DropReason::Debounced)
        );
        assert_eq!(
            policy.admit(&blocked(), None),
            Admission::Dropped(DropReason::Debounced)
        );
        // A different prompt is a new DM.
        assert_eq!(policy.admit(&blocked(), Some("c")), Admission::Instant);
        // After 30 minutes the same prompt may DM again.
        clock.advance(Duration::from_secs(31 * 60));
        assert_eq!(policy.admit(&blocked(), Some("c")), Admission::Instant);

        // At most six instant DMs per rolling hour; overflow is digested.
        let clock = TestClock::at(WORKDAY_MORNING);
        let mut policy = Policy::new(parts(&clock, None));
        for worker in 0..6 {
            assert_eq!(
                policy.admit(&event(&format!("w-{worker}"), AttentionKind::Blocked), None),
                Admission::Instant
            );
            clock.advance(Duration::from_secs(60));
        }
        assert_eq!(
            policy.admit(&event("w-6", AttentionKind::Blocked), None),
            Admission::Held
        );
        // The first DM leaves the window an hour after it went out.
        clock.advance(Duration::from_secs(54 * 60));
        assert_eq!(
            policy.admit(&event("w-7", AttentionKind::Blocked), None),
            Admission::Instant
        );
    }

    #[test]
    fn automatic_summary_events_never_notify() {
        let clock = TestClock::at(WORKDAY_MORNING);
        let mut policy = Policy::new(parts(&clock, None));
        let mut automatic = event("w-1", AttentionKind::Blocked);
        automatic.automatic = true;
        assert_eq!(
            policy.admit(&automatic.clone(), None),
            Admission::Dropped(DropReason::Automatic)
        );
        clock.set(WORKDAY_EVENING);
        assert_eq!(
            policy.admit(&automatic, None),
            Admission::Dropped(DropReason::Automatic)
        );
        assert_eq!(policy.held_count(), 0);
    }

    #[test]
    fn a_mute_holds_like_quiet_hours_and_survives_a_restart() {
        let temp = TempDir::new().unwrap();
        let clock = TestClock::at(WORKDAY_MORNING);
        let mut policy = Policy::new(parts(&clock, Some(&temp)));
        let until = clock.clock()() + chrono::Duration::hours(1);
        policy.mute_until(until);
        assert_eq!(
            policy.admit(&event("w-1", AttentionKind::Blocked), None),
            Admission::Held
        );
        let status = policy.status();
        assert!(status.active);
        let until_ms = u64::try_from(until.timestamp_millis()).unwrap();
        assert_eq!(status.muted_until, Some(until_ms));
        assert_eq!(status.until, Some(until_ms));
        drop(policy);

        let mut policy = Policy::new(parts(&clock, Some(&temp)));
        assert!(policy.muted_until(clock.clock()()).is_some());
        assert_eq!(policy.digest_due(), None);
        // The mute ends inside working hours: the digest goes out.
        clock.advance(Duration::from_secs(60 * 60));
        assert_eq!(policy.digest_due(), Some(DigestKind::Away));
        assert_eq!(policy.status().muted_until, None);

        policy.mute_until(clock.clock()() + chrono::Duration::hours(3));
        policy.unmute();
        assert!(!policy.status().active);
        // Asked for explicitly: due even outside hours, even with nothing held.
        clock.set(WORKDAY_EVENING);
        let _ = policy.take_digest();
        assert_eq!(policy.digest_due(), None);
        policy.request_digest();
        assert_eq!(policy.digest_due(), Some(DigestKind::Requested));
        let _ = policy.take_digest();
        assert_eq!(policy.digest_due(), None);
    }
    #[test]
    fn an_instant_dm_replaces_a_held_line_for_the_same_worker() {
        let clock = TestClock::at("2026-10-01T15:50:00Z"); // Thu 08:50 PDT
        let mut policy = Policy::new(parts(&clock, None));
        assert_eq!(
            policy.admit(&event("w-1", AttentionKind::Blocked), None),
            Admission::Held
        );
        policy.admit(&event("w-2", AttentionKind::ReadyForReview), None);
        // Answered in Yard, blocked again after nine: DM'd once, not again
        // in the morning digest.
        clock.set("2026-10-01T16:01:00Z");
        assert_eq!(
            policy.admit(&event("w-1", AttentionKind::Blocked), None),
            Admission::Instant
        );
        let taken = policy.take_digest();
        assert_eq!(taken.entries.len(), 1);
        assert_eq!(taken.entries[0].key, "worker:w-2:ReadyForReview");
    }

    #[test]
    fn a_failed_requested_digest_is_asked_for_again() {
        let clock = TestClock::at(WORKDAY_EVENING);
        let mut policy = Policy::new(parts(&clock, None));
        policy.admit(&event("w-1", AttentionKind::Blocked), None);
        policy.request_digest();
        assert_eq!(policy.digest_due(), Some(DigestKind::Requested));
        let _ = policy.take_digest();
        assert_eq!(policy.digest_due(), None);
        policy.digest_failed(DigestKind::Requested);
        assert_eq!(policy.digest_due(), Some(DigestKind::Requested));
        let taken = policy.take_digest();
        policy.digest_sent(&taken);
        assert_eq!(policy.digest_due(), None);
        assert_eq!(policy.held_count(), 0);
    }

    #[test]
    fn queue_drops_handed_over_at_quiet_time_go_into_the_digest() {
        let clock = TestClock::at(WORKDAY_EVENING);
        let mut policy = Policy::new(parts(&clock, None));
        policy.hold_all([event("w-1", AttentionKind::Blocked)], 3);
        let taken = policy.take_digest();
        assert_eq!(taken.entries.len(), 1);
        assert_eq!(taken.dropped, 3);
        policy.digest_sent(&taken);
        assert_eq!(policy.take_digest().dropped, 0);
    }

    #[test]
    fn held_events_keep_only_a_redacted_title_and_the_file_stays_small() {
        let clock = TestClock::at(WORKDAY_EVENING);
        let dir = TempDir::new().unwrap();
        let mut policy = Policy::new(parts(&clock, Some(&dir)));
        let objective = format!(
            "Fix the <login> & password=hunter2 flow\n{}",
            "the full task text ".repeat(500)
        );
        for index in 0..crate::slack::held::MAX_HELD {
            let mut event = event(&format!("w-{index}"), AttentionKind::ReadyForReview);
            event.subject = Subject::Worker {
                profile_name: Some("alpha".to_owned()),
                objective: Some(objective.clone()),
            };
            assert_eq!(policy.admit(&event, None), Admission::Held);
        }
        let path = dir.path().join("slack-held.json");
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("hunter2"), "secret written to disk");
        assert!(!file.contains("full task text"), "only the first line");
        assert!(file.contains("Fix the <login> & password=[redacted] flow"));
        assert!(std::fs::metadata(&path).unwrap().len() < 1024 * 1024);

        // A restart reads it back (not "too large") and the card escapes once.
        let mut policy = Policy::new(parts(&clock, Some(&dir)));
        assert_eq!(policy.held_count(), crate::slack::held::MAX_HELD);
        let taken = policy.take_digest();
        assert_eq!(
            crate::slack::message::subject_title(&taken.entries[0].event.subject).as_deref(),
            Some("Fix the &lt;login&gt; &amp; password=[redacted] flow")
        );
    }
}

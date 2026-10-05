//! When Yard may DM the owner: working hours in the owner's time zone, which
//! kinds go out instantly, and how often the digest runs.
//!
//! Defaults: weekdays 09:00–18:00 `America/Los_Angeles`, instant DMs only for
//! blocked agents, a digest at most every 120 minutes. Overrides:
//! `YARD_SLACK_TIMEZONE` (IANA name), `YARD_SLACK_WORK_HOURS` (`09:00-18:00`),
//! `YARD_SLACK_WORK_DAYS` (`mon-fri`, `mon,wed,fri`, `all`),
//! `YARD_SLACK_INSTANT` (`blocked` | `blocked,failed` | `none`) and
//! `YARD_SLACK_DIGEST_EVERY_MINS`. An invalid value is reported once (the
//! caller logs it) and the default is used. Time-zone math goes through
//! `chrono-tz`, so DST days are exact.

use std::time::Duration;

use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use super::detector::AttentionKind;

pub const TIMEZONE_VAR: &str = "YARD_SLACK_TIMEZONE";
pub const WORK_HOURS_VAR: &str = "YARD_SLACK_WORK_HOURS";
pub const WORK_DAYS_VAR: &str = "YARD_SLACK_WORK_DAYS";
pub const INSTANT_VAR: &str = "YARD_SLACK_INSTANT";
pub const DIGEST_EVERY_VAR: &str = "YARD_SLACK_DIGEST_EVERY_MINS";

pub const DEFAULT_TIMEZONE: Tz = chrono_tz::America::Los_Angeles;
const DEFAULT_START_MIN: u32 = 9 * 60;
const DEFAULT_END_MIN: u32 = 18 * 60;
const MINUTES_PER_DAY: u32 = 24 * 60;
pub const DEFAULT_DIGEST_EVERY: Duration = Duration::from_secs(120 * 60);
const MAX_DIGEST_EVERY_MINS: u64 = 24 * 60;
const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Which kinds may DM instantly inside working hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstantKinds {
    pub blocked: bool,
    pub failed: bool,
    pub review: bool,
}

impl InstantKinds {
    #[must_use]
    pub const fn allows(self, kind: AttentionKind) -> bool {
        match kind {
            AttentionKind::Blocked => self.blocked,
            AttentionKind::CommandFailed | AttentionKind::CommandAmbiguous => self.failed,
            AttentionKind::ReadyForReview => self.review,
        }
    }

    /// For the settings card: `blocked`, `blocked, failed` or `none`.
    #[must_use]
    pub fn label(self) -> String {
        let names = [
            (self.blocked, "blocked"),
            (self.failed, "failed"),
            (self.review, "review"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    }
}

/// The owner's delivery schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietConfig {
    pub timezone: Tz,
    /// Minutes after local midnight; `end_min` may be 1440 (midnight).
    pub start_min: u32,
    pub end_min: u32,
    /// Monday first.
    pub days: [bool; 7],
    pub instant: InstantKinds,
    pub digest_every: Duration,
}

impl Default for QuietConfig {
    fn default() -> Self {
        Self {
            timezone: DEFAULT_TIMEZONE,
            start_min: DEFAULT_START_MIN,
            end_min: DEFAULT_END_MIN,
            days: [true, true, true, true, true, false, false],
            instant: InstantKinds {
                blocked: true,
                failed: false,
                review: false,
            },
            digest_every: DEFAULT_DIGEST_EVERY,
        }
    }
}

/// A working window in UTC: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl QuietConfig {
    /// Every minute of every day, every kind instant (tests and rehearsals
    /// that are about something else).
    #[must_use]
    pub const fn always() -> Self {
        Self {
            timezone: DEFAULT_TIMEZONE,
            start_min: 0,
            end_min: MINUTES_PER_DAY,
            days: [true; 7],
            instant: InstantKinds {
                blocked: true,
                failed: true,
                review: true,
            },
            digest_every: DEFAULT_DIGEST_EVERY,
        }
    }

    /// The window containing `now`, if `now` is inside working hours.
    #[must_use]
    pub fn window_at(&self, now: DateTime<Utc>) -> Option<Window> {
        self.windows_from(now)
            .find(|window| window.end > now)
            .filter(|window| window.start <= now)
    }

    #[must_use]
    pub fn in_window(&self, now: DateTime<Utc>) -> bool {
        self.window_at(now).is_some()
    }

    /// The first window that has not ended by `now` (the current one when
    /// inside working hours). `None` only when no day is a working day.
    #[must_use]
    pub fn next_window(&self, now: DateTime<Utc>) -> Option<Window> {
        self.windows_from(now).find(|window| window.end > now)
    }

    /// Windows starting on the local day before `now` (an evening window
    /// that ends after midnight UTC) through the next eight days.
    fn windows_from(&self, now: DateTime<Utc>) -> impl Iterator<Item = Window> + '_ {
        let today = now.with_timezone(&self.timezone).date_naive();
        let first = today.checked_sub_days(Days::new(1)).unwrap_or(today);
        (0..10u64)
            .filter_map(move |offset| first.checked_add_days(Days::new(offset)))
            .filter(|date| self.days[date.weekday().num_days_from_monday() as usize])
            .filter_map(|date| {
                let start = self.local(date, self.start_min)?;
                let end = self.local(date, self.end_min)?;
                (end > start).then_some(Window { start, end })
            })
    }

    /// `date` + `minutes` in the zone. A time skipped by spring-forward
    /// moves to the first valid instant after the gap; a repeated time
    /// (fall-back) takes the earlier one.
    pub(crate) fn local(&self, date: NaiveDate, minutes: u32) -> Option<DateTime<Utc>> {
        let naive = NaiveDateTime::from(date)
            .checked_add_signed(chrono::Duration::minutes(i64::from(minutes)))?;
        for step in 0..=180 {
            let probe = naive.checked_add_signed(chrono::Duration::minutes(step))?;
            match self.timezone.from_local_datetime(&probe) {
                LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => {
                    return Some(at.with_timezone(&Utc));
                }
                LocalResult::None => {}
            }
        }
        None
    }
}

impl QuietConfig {
    /// Read the overrides through `lookup` (the process environment in
    /// production). Returns the config and one warning per invalid value.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> (Self, Vec<String>) {
        let mut config = Self::default();
        let mut warnings = Vec::new();
        let read = |name: &str| lookup(name).map(|value| value.trim().to_owned());
        let mut check = |name: &str, ok: bool, expected: &str, value: &str| {
            if !ok {
                warnings.push(format!(
                    "{name}={value:?} is not {expected}; using the default"
                ));
            }
            ok
        };
        if let Some(value) = read(TIMEZONE_VAR).filter(|value| !value.is_empty()) {
            let parsed = value.parse::<Tz>().ok();
            if check(TIMEZONE_VAR, parsed.is_some(), "an IANA time zone", &value) {
                config.timezone = parsed.unwrap_or(DEFAULT_TIMEZONE);
            }
        }
        if let Some(value) = read(WORK_HOURS_VAR).filter(|value| !value.is_empty()) {
            let parsed = parse_hours(&value);
            if check(WORK_HOURS_VAR, parsed.is_some(), "HH:MM-HH:MM", &value)
                && let Some((start, end)) = parsed
            {
                config.start_min = start;
                config.end_min = end;
            }
        }
        if let Some(value) = read(WORK_DAYS_VAR).filter(|value| !value.is_empty()) {
            let parsed = parse_days(&value);
            if check(
                WORK_DAYS_VAR,
                parsed.is_some(),
                "a day list such as mon-fri",
                &value,
            ) && let Some(days) = parsed
            {
                config.days = days;
            }
        }
        if let Some(value) = read(INSTANT_VAR).filter(|value| !value.is_empty()) {
            let parsed = parse_instant(&value);
            if check(
                INSTANT_VAR,
                parsed.is_some(),
                "blocked, blocked,failed or none",
                &value,
            ) && let Some(instant) = parsed
            {
                config.instant = instant;
            }
        }
        if let Some(value) = read(DIGEST_EVERY_VAR).filter(|value| !value.is_empty()) {
            let parsed = value
                .parse::<u64>()
                .ok()
                .filter(|minutes| (1..=MAX_DIGEST_EVERY_MINS).contains(minutes));
            if check(
                DIGEST_EVERY_VAR,
                parsed.is_some(),
                "minutes from 1 to 1440",
                &value,
            ) && let Some(minutes) = parsed
            {
                config.digest_every = Duration::from_secs(minutes * 60);
            }
        }
        (config, warnings)
    }

    /// `09:00-18:00` for the settings card.
    #[must_use]
    pub fn hours_label(&self) -> String {
        let clock = |minutes: u32| format!("{:02}:{:02}", minutes / 60, minutes % 60);
        format!("{}-{}", clock(self.start_min), clock(self.end_min))
    }

    /// `mon-fri`, `all` or a list, for the settings card.
    #[must_use]
    pub fn days_label(&self) -> String {
        match self.days {
            [true, true, true, true, true, false, false] => "mon-fri".to_owned(),
            [true, true, true, true, true, true, true] => "all".to_owned(),
            days => DAY_NAMES
                .iter()
                .zip(days)
                .filter(|(_, on)| *on)
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// `HH:MM-HH:MM` with start before end (`24:00` allowed as the end).
fn parse_hours(value: &str) -> Option<(u32, u32)> {
    let (start, end) = value.split_once('-')?;
    let minutes = |part: &str| -> Option<u32> {
        let (hours, minutes) = part.trim().split_once(':')?;
        let hours = hours.parse::<u32>().ok()?;
        let minutes = minutes
            .parse::<u32>()
            .ok()
            .filter(|minutes| *minutes < 60)?;
        let total = hours * 60 + minutes;
        (total <= MINUTES_PER_DAY).then_some(total)
    };
    let (start, end) = (minutes(start)?, minutes(end)?);
    (start < end && start < MINUTES_PER_DAY).then_some((start, end))
}

/// `mon-fri`, `mon,wed,fri`, `sat-sun`, `all`; at least one day.
fn parse_days(value: &str) -> Option<[bool; 7]> {
    let lower = value.to_ascii_lowercase();
    if lower == "all" {
        return Some([true; 7]);
    }
    let index = |name: &str| DAY_NAMES.iter().position(|day| *day == name.trim());
    let mut days = [false; 7];
    for part in lower.split(',') {
        match part.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (index(from)?, index(to)?);
                if from > to {
                    return None;
                }
                days[from..=to].iter_mut().for_each(|day| *day = true);
            }
            None => days[index(part)?] = true,
        }
    }
    days.contains(&true).then_some(days)
}

/// `blocked`, `blocked,failed`, `none` (and `failed` / `review` in lists).
fn parse_instant(value: &str) -> Option<InstantKinds> {
    let lower = value.to_ascii_lowercase();
    let mut kinds = InstantKinds {
        blocked: false,
        failed: false,
        review: false,
    };
    if lower == "none" {
        return Some(kinds);
    }
    for part in lower.split(',') {
        match part.trim() {
            "blocked" => kinds.blocked = true,
            "failed" => kinds.failed = true,
            "review" => kinds.review = true,
            _ => return None,
        }
    }
    Some(kinds)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{DEFAULT_TIMEZONE, InstantKinds, QuietConfig};
    use crate::slack::detector::AttentionKind;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn la(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        DEFAULT_TIMEZONE
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn weekday_edges_are_inclusive_at_nine_and_exclusive_at_six() {
        let config = QuietConfig::default();
        // Thursday 2026-10-01, PDT (UTC-7).
        assert!(!config.in_window(utc("2026-10-01T15:59:00Z")));
        assert!(config.in_window(utc("2026-10-01T16:00:00Z")));
        assert!(config.in_window(utc("2026-10-02T00:59:00Z")));
        assert!(!config.in_window(utc("2026-10-02T01:00:00Z")));
        let window = config.window_at(la(2026, 10, 1, 12, 0)).unwrap();
        assert_eq!(window.start, utc("2026-10-01T16:00:00Z"));
        assert_eq!(window.end, utc("2026-10-02T01:00:00Z"));
        // 8–10 pm Pacific (03–05 UTC), where the spam came from, is quiet.
        assert!(!config.in_window(utc("2026-10-02T03:30:00Z")));
    }

    #[test]
    fn weekends_are_quiet_and_monday_nine_is_next() {
        let config = QuietConfig::default();
        let saturday_noon = la(2026, 10, 3, 12, 0);
        assert!(!config.in_window(saturday_noon));
        let next = config.next_window(saturday_noon).unwrap();
        assert_eq!(next.start, utc("2026-10-05T16:00:00Z"));
        // Friday after six also waits for Monday.
        let friday_evening = la(2026, 10, 2, 18, 0);
        assert_eq!(
            config.next_window(friday_evening).unwrap().start,
            utc("2026-10-05T16:00:00Z")
        );
        // Inside the window, the next window is the current one.
        let inside = la(2026, 10, 2, 10, 0);
        assert_eq!(
            config.next_window(inside).unwrap(),
            config.window_at(inside).unwrap()
        );
    }

    #[test]
    fn spring_forward_and_fall_back_move_nine_am_by_an_hour_in_utc() {
        let config = QuietConfig::default();
        // Friday 2026-03-06 is PST (UTC-8); DST starts Sunday 03-08.
        assert!(config.in_window(utc("2026-03-06T17:00:00Z")));
        assert!(!config.in_window(utc("2026-03-06T16:59:00Z")));
        let after_friday = utc("2026-03-07T02:00:00Z");
        assert_eq!(
            config.next_window(after_friday).unwrap().start,
            utc("2026-03-09T16:00:00Z")
        );
        // Friday 2026-10-30 is PDT; DST ends Sunday 11-01.
        let after_friday = la(2026, 10, 30, 18, 0);
        assert_eq!(after_friday, utc("2026-10-31T01:00:00Z"));
        let monday = config.next_window(after_friday).unwrap();
        assert_eq!(monday.start, utc("2026-11-02T17:00:00Z"));
        assert_eq!(monday.end, utc("2026-11-03T02:00:00Z"));

        // On the transition days themselves (every day a working day).
        let mut every_day = QuietConfig {
            days: [true; 7],
            ..QuietConfig::default()
        };
        let spring = every_day.next_window(utc("2026-03-08T12:00:00Z")).unwrap();
        assert_eq!(spring.start, utc("2026-03-08T16:00:00Z"));
        let fall = every_day.next_window(utc("2026-11-01T12:00:00Z")).unwrap();
        assert_eq!(fall.start, utc("2026-11-01T17:00:00Z"));
        // A start inside the skipped hour moves to 03:00 PDT; a start in the
        // repeated hour takes the first (PDT) occurrence.
        every_day.start_min = 2 * 60 + 30;
        let gap = every_day.next_window(utc("2026-03-08T09:00:00Z")).unwrap();
        assert_eq!(gap.start, utc("2026-03-08T10:00:00Z"));
        every_day.start_min = 60 + 30;
        let repeated = every_day.next_window(utc("2026-11-01T07:00:00Z")).unwrap();
        assert_eq!(repeated.start, utc("2026-11-01T08:30:00Z"));
    }

    #[test]
    fn env_overrides_parse_and_invalid_values_fall_back_with_a_warning() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let (config, warnings) = QuietConfig::from_lookup(env(&[]));
        assert_eq!(config, QuietConfig::default());
        assert!(warnings.is_empty());

        let (config, warnings) = QuietConfig::from_lookup(env(&[
            ("YARD_SLACK_TIMEZONE", "Europe/Berlin"),
            ("YARD_SLACK_WORK_HOURS", "08:30-17:00"),
            ("YARD_SLACK_WORK_DAYS", "mon,wed,fri"),
            ("YARD_SLACK_INSTANT", "blocked,failed"),
            ("YARD_SLACK_DIGEST_EVERY_MINS", "45"),
        ]));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.timezone, chrono_tz::Europe::Berlin);
        assert_eq!(config.hours_label(), "08:30-17:00");
        assert_eq!(config.days_label(), "mon,wed,fri");
        assert!(config.instant.allows(AttentionKind::CommandFailed));
        assert!(!config.instant.allows(AttentionKind::ReadyForReview));
        assert_eq!(config.digest_every.as_secs(), 45 * 60);

        let (config, warnings) = QuietConfig::from_lookup(env(&[
            ("YARD_SLACK_TIMEZONE", "Mars/Olympus"),
            ("YARD_SLACK_WORK_HOURS", "18:00-09:00"),
            ("YARD_SLACK_WORK_DAYS", "fri-mon"),
            ("YARD_SLACK_INSTANT", "everything"),
            ("YARD_SLACK_DIGEST_EVERY_MINS", "0"),
        ]));
        assert_eq!(warnings.len(), 5, "{warnings:?}");
        assert!(warnings[0].contains("YARD_SLACK_TIMEZONE"));
        assert_eq!(config, QuietConfig::default());
        // The fallback zone is still DST-correct.
        assert!(config.in_window(utc("2026-10-01T16:00:00Z")));

        let (config, _) = QuietConfig::from_lookup(env(&[("YARD_SLACK_INSTANT", "none")]));
        assert_eq!(
            config.instant,
            InstantKinds {
                blocked: false,
                failed: false,
                review: false
            }
        );
        assert_eq!(config.instant.label(), "none");
    }
}

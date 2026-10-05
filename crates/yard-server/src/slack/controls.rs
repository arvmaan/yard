//! Owner controls for quiet hours (owner DM commands and buttons):
//! `settings` / `quiet` (the schedule card), `mute <duration>`,
//! `mute until <time>`, `unmute` and `digest`.
//!
//! Parsing is pure and times resolve against the configured time zone
//! (DST-correct through [`QuietConfig::local`]). The commands run through
//! the same owner-only, audited inbound path as `status`; the card's buttons
//! are opaque, single-use navigation ids ([`super::actions::NavAction`]).

use std::time::Duration;

use chrono::{DateTime, Datelike, Days, Utc, Weekday};

use super::{
    actions::{ACTION_TTL, NavAction},
    blocks::{self, Style},
    message::OutgoingMessage,
    policy::{Policy, QuietStatusView},
    quiet::QuietConfig,
    views::{Nav, nav_button},
};

/// The longest mute (a typo like `mute 300d` must not silence Yard for good).
pub const MAX_MUTE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Which local day a `mute until` ends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UntilDay {
    /// The next time the clock shows that time (today, else tomorrow).
    Next,
    Today,
    Tomorrow,
    /// The next such weekday whose time is still ahead.
    On(Weekday),
}

/// How long a mute lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteSpec {
    For(Duration),
    /// Local time; `minutes` `None` means the start of working hours.
    Until {
        day: UntilDay,
        minutes: Option<u32>,
    },
}

/// What a control does (typed or tapped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Settings,
    Mute(MuteSpec),
    Unmute,
    Digest,
}

pub const MUTE_USAGE: &str = "Try `mute 30m`, `mute 2h`, `mute 1d`, `mute until 9am`, `mute until tomorrow` or `mute until monday`.";

/// `30m`, `2h`, `1d`, `90 min`, `2 hours` (any length; the caller caps it).
fn parse_duration(text: &str) -> Option<Duration> {
    let compact = text.replace(' ', "");
    let split = compact.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = compact.split_at(split);
    let number = number.parse::<u64>().ok().filter(|number| *number > 0)?;
    let seconds = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
        "d" | "day" | "days" => 24 * 60 * 60,
        _ => return None,
    };
    Some(Duration::from_secs(number.checked_mul(seconds)?))
}

/// `9am`, `9:30am`, `9 pm`, `17:00`, `noon`, `midnight` → minutes after
/// local midnight. A bare number is refused (`9` could mean anything).
fn parse_clock(text: &str) -> Option<u32> {
    let compact = text.replace(' ', "");
    match compact.as_str() {
        "noon" => return Some(12 * 60),
        "midnight" => return Some(0),
        _ => {}
    }
    let (digits, meridiem) = if let Some(rest) = compact.strip_suffix("am") {
        (rest, Some(false))
    } else if let Some(rest) = compact.strip_suffix("pm") {
        (rest, Some(true))
    } else {
        (compact.as_str(), None)
    };
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) if minutes.len() == 2 => (hours, minutes.parse::<u32>().ok()?),
        None if meridiem.is_some() => (digits, 0),
        _ => return None,
    };
    if hours.is_empty() || hours.len() > 2 || minutes >= 60 {
        return None;
    }
    let hours = hours.parse::<u32>().ok()?;
    let hours = match meridiem {
        Some(pm) if (1..=12).contains(&hours) => hours % 12 + if pm { 12 } else { 0 },
        None if hours < 24 => hours,
        _ => return None,
    };
    Some(hours * 60 + minutes)
}

fn parse_day(word: &str) -> Option<UntilDay> {
    Some(match word {
        "today" => UntilDay::Today,
        "tomorrow" | "tmrw" => UntilDay::Tomorrow,
        "mon" | "monday" => UntilDay::On(Weekday::Mon),
        "tue" | "tues" | "tuesday" => UntilDay::On(Weekday::Tue),
        "wed" | "wednesday" => UntilDay::On(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => UntilDay::On(Weekday::Thu),
        "fri" | "friday" => UntilDay::On(Weekday::Fri),
        "sat" | "saturday" => UntilDay::On(Weekday::Sat),
        "sun" | "sunday" => UntilDay::On(Weekday::Sun),
        _ => return None,
    })
}

/// What follows `mute ` (already lower-cased): a duration, or `until` and
/// an optional day plus an optional time (`until tomorrow 9am`,
/// `until monday`, `until 5:30pm`).
///
/// # Errors
/// Owner-facing usage text when the phrase cannot be read or the mute
/// would last longer than [`MAX_MUTE`].
pub fn parse_mute(args: &str) -> Result<MuteSpec, String> {
    let args = args.trim();
    let Some(rest) = args
        .strip_prefix("until")
        .or_else(|| args.strip_prefix("till"))
    else {
        return match parse_duration(args) {
            Some(duration) if duration <= MAX_MUTE => Ok(MuteSpec::For(duration)),
            Some(_) => Err(format!("A mute lasts at most 14 days. {MUTE_USAGE}")),
            None => Err(MUTE_USAGE.to_owned()),
        };
    };
    let words = rest
        .split_whitespace()
        .filter(|word| *word != "at")
        .collect::<Vec<_>>();
    let (day, clock) = match words.split_first() {
        None => return Err(MUTE_USAGE.to_owned()),
        Some((first, rest)) => match parse_day(first) {
            Some(day) => (day, rest.join(" ")),
            None => (UntilDay::Next, words.join(" ")),
        },
    };
    let minutes = if clock.is_empty() {
        if day == UntilDay::Next {
            return Err(MUTE_USAGE.to_owned());
        }
        None
    } else {
        Some(parse_clock(&clock).ok_or_else(|| MUTE_USAGE.to_owned())?)
    };
    Ok(MuteSpec::Until { day, minutes })
}

/// When a mute ends (`now` and the result in UTC; the day and clock are in
/// the configured zone).
///
/// # Errors
/// Owner-facing text when the time has passed (`today`), cannot be found,
/// or is more than [`MAX_MUTE`] away.
pub fn resolve(
    spec: MuteSpec,
    config: &QuietConfig,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, String> {
    let until = match spec {
        MuteSpec::For(duration) => {
            now + chrono::Duration::from_std(duration).map_err(|_| MUTE_USAGE.to_owned())?
        }
        MuteSpec::Until { day, minutes } => {
            let minutes = minutes.unwrap_or(config.start_min);
            let today = now.with_timezone(&config.timezone).date_naive();
            let at = |offset: u64| {
                today
                    .checked_add_days(Days::new(offset))
                    .and_then(|date| Some((date, config.local(date, minutes)?)))
            };
            let found = match day {
                UntilDay::Next => (0..=1).filter_map(at).find(|(_, at)| *at > now),
                UntilDay::Today => at(0).filter(|(_, at)| *at > now),
                UntilDay::Tomorrow => at(1),
                UntilDay::On(weekday) => (0..=7)
                    .filter_map(at)
                    .find(|(date, at)| date.weekday() == weekday && *at > now),
            };
            match found {
                Some((_, at)) => at,
                None if day == UntilDay::Today => {
                    return Err(
                        "That time has already passed today; try `mute until tomorrow`.".to_owned(),
                    );
                }
                None => return Err(MUTE_USAGE.to_owned()),
            }
        }
    };
    let span = until
        .signed_duration_since(now)
        .to_std()
        .unwrap_or_default();
    if span > MAX_MUTE {
        return Err(format!("A mute lasts at most 14 days. {MUTE_USAGE}"));
    }
    Ok(until)
}

/// `Mon Oct 5, 9:00 AM PDT` in the configured zone.
#[must_use]
pub fn local_label(config: &QuietConfig, at: DateTime<Utc>) -> String {
    at.with_timezone(&config.timezone)
        .format("%a %b %-d, %-I:%M %p %Z")
        .to_string()
}

fn ms_label(config: &QuietConfig, ms: u64) -> Option<String> {
    let at = DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)?;
    Some(local_label(config, at))
}

/// What the settings card shows (read under the policy lock, rendered
/// after it is released).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub config: QuietConfig,
    pub status: QuietStatusView,
}

/// The outcome of one control.
#[derive(Debug, Clone)]
pub enum Applied {
    /// Show the settings card.
    Settings(Snapshot),
    /// A confirmation or refusal; `outcome` is the audit outcome.
    Reply { text: String, outcome: &'static str },
    /// The digest goes out on the next tick (it is the reply).
    DigestRequested,
}

impl Control {
    /// The audit action name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Mute(_) => "mute",
            Self::Unmute => "unmute",
            Self::Digest => "digest",
        }
    }
}

/// Run one control against the policy (owner-only; the caller audits).
pub fn apply(policy: &mut Policy, control: Control) -> Applied {
    let now = policy.now();
    match control {
        Control::Settings => Applied::Settings(Snapshot {
            config: policy.config().clone(),
            status: policy.status(),
        }),
        Control::Mute(spec) => match resolve(spec, policy.config(), now) {
            Ok(until) => {
                policy.mute_until(until);
                let label = local_label(policy.config(), until);
                let after = match policy.deliverable_at(until) {
                    Some(at) if at > until => format!(
                        " It ends outside working hours, so the digest comes at {}.",
                        local_label(policy.config(), at)
                    ),
                    _ => String::new(),
                };
                Applied::Reply {
                    text: format!(
                        ":no_bell: Muted until *{label}*. I'll hold every notification and send one digest when the mute ends.{after} Say `unmute` to end it sooner."
                    ),
                    outcome: "answered",
                }
            }
            Err(text) => Applied::Reply {
                text,
                outcome: "refused:invalid",
            },
        },
        Control::Unmute => {
            let was_muted = policy.muted_until(now).is_some();
            policy.unmute();
            let held = policy.held_count();
            let text = if !was_muted {
                "Yard was not muted.".to_owned()
            } else if policy.holding(now) {
                let next = policy
                    .deliverable_at(now)
                    .map(|at| {
                        format!(
                            " Notifications resume at {}.",
                            local_label(policy.config(), at)
                        )
                    })
                    .unwrap_or_default();
                format!(
                    ":bell: Unmuted. It's outside working hours, so I'm still holding notifications.{next}"
                )
            } else if held > 0 {
                ":bell: Unmuted. What I held follows in a digest.".to_owned()
            } else {
                ":bell: Unmuted.".to_owned()
            };
            Applied::Reply {
                text,
                outcome: "answered",
            }
        }
        Control::Digest => {
            policy.request_digest();
            Applied::DigestRequested
        }
    }
}

/// `9am`, `9:30am`, `12pm` (button labels).
fn short_clock(minutes: u32) -> String {
    let (hours, minutes) = (minutes / 60 % 24, minutes % 60);
    let suffix = if hours < 12 { "am" } else { "pm" };
    let hours = match hours % 12 {
        0 => 12,
        hours => hours,
    };
    if minutes == 0 {
        format!("{hours}{suffix}")
    } else {
        format!("{hours}:{minutes:02}{suffix}")
    }
}

/// The buttons of the settings card, in order.
#[must_use]
pub fn card_buttons(config: &QuietConfig) -> Vec<(Control, String, Style)> {
    let start = short_clock(config.start_min);
    vec![
        (
            Control::Mute(MuteSpec::For(Duration::from_secs(60 * 60))),
            "Mute 1h".to_owned(),
            Style::Default,
        ),
        (
            Control::Mute(MuteSpec::Until {
                day: UntilDay::Tomorrow,
                minutes: None,
            }),
            format!("Mute until tomorrow {start}"),
            Style::Default,
        ),
        (
            Control::Mute(MuteSpec::Until {
                day: UntilDay::On(Weekday::Mon),
                minutes: None,
            }),
            "Mute until Monday".to_owned(),
            Style::Default,
        ),
        (Control::Unmute, "Unmute".to_owned(), Style::Default),
        (Control::Digest, "Digest now".to_owned(), Style::Primary),
    ]
}

/// `settings` / `quiet`: the schedule, mute state and next delivery, with
/// buttons.
#[must_use]
pub fn settings_card(snapshot: &Snapshot, nav: &mut Nav<'_>, ui_url: &str) -> OutgoingMessage {
    let Snapshot { config, status } = snapshot;
    let label = |ms: Option<u64>| ms.and_then(|ms| ms_label(config, ms));
    let state = if status.active {
        match label(status.until) {
            Some(until) => {
                format!(":crescent_moon: *Quiet now* — holding notifications until {until}.")
            }
            None => ":crescent_moon: *Quiet now* — holding notifications.".to_owned(),
        }
    } else {
        ":sunny: *Delivering now* (working hours).".to_owned()
    };
    let mute = label(status.muted_until)
        .map_or_else(|| "off".to_owned(), |until| format!("until {until}"));
    let next = match (status.held_count, label(status.next_delivery_at)) {
        (0, _) => "nothing held".to_owned(),
        (held, Some(at)) => format!("{at} ({held} held)"),
        (held, None) => format!("{held} held"),
    };
    let every = config.digest_every.as_secs() / 60;
    let pairs = [
        (
            "Schedule",
            format!("{} {}", config.days_label(), config.hours_label()),
        ),
        ("Time zone", config.timezone.name().to_owned()),
        ("Instant DMs", config.instant.label()),
        (
            "Digest",
            format!("at most every {every} min in working hours"),
        ),
        ("Mute", mute),
        ("Next delivery", next),
    ];
    let mut elements = card_buttons(config)
        .into_iter()
        .filter_map(|(control, text, style)| {
            nav_button(nav, NavAction::Quiet(control), &text, style)
        })
        .collect::<Vec<_>>();
    elements.push(blocks::open_in_yard(ui_url));
    let text = format!(
        "Notification settings: {} ({}), instant DMs: {}, mute: {}, next delivery: {}.",
        pairs[0].1, pairs[1].1, pairs[2].1, pairs[4].1, pairs[5].1
    );
    let blocks = vec![
        blocks::header(":crescent_moon: Notification settings"),
        blocks::section(&state),
        blocks::fields(&pairs),
        blocks::actions("yard_quiet", elements),
        blocks::context(&[
            "Type `mute 2h`, `mute until 9am`, `unmute` or `digest`. The schedule comes from Yard's `YARD_SLACK_*` settings.".to_owned(),
            format!("Buttons work once and expire in {} min.", ACTION_TTL.as_secs() / 60),
        ]),
    ];
    OutgoingMessage {
        text,
        blocks: blocks::finish(blocks),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{DateTime, Utc, Weekday};

    use super::{MuteSpec, UntilDay, parse_mute, resolve};
    use crate::slack::{
        policy::tests::{WORKDAY_EVENING, WORKDAY_MORNING},
        quiet::QuietConfig,
    };

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn until(args: &str, now: &str) -> Result<DateTime<Utc>, String> {
        resolve(parse_mute(args)?, &QuietConfig::default(), at(now))
    }

    const HOUR: u64 = 60 * 60;

    #[test]
    fn mute_durations_and_until_phrases_parse() {
        for (text, secs) in [
            ("30m", 30 * 60),
            ("2h", 2 * HOUR),
            ("1d", 24 * HOUR),
            ("90 min", 90 * 60),
            ("2 hours", 2 * HOUR),
            ("14d", 14 * 24 * HOUR),
        ] {
            assert_eq!(
                parse_mute(text),
                Ok(MuteSpec::For(Duration::from_secs(secs))),
                "{text}"
            );
        }
        let until = |day, minutes| Ok(MuteSpec::Until { day, minutes });
        assert_eq!(parse_mute("until 9am"), until(UntilDay::Next, Some(540)));
        assert_eq!(
            parse_mute("until 5:30pm"),
            until(UntilDay::Next, Some(1050))
        );
        assert_eq!(parse_mute("until 17:00"), until(UntilDay::Next, Some(1020)));
        assert_eq!(parse_mute("until 12am"), until(UntilDay::Next, Some(0)));
        assert_eq!(parse_mute("until noon"), until(UntilDay::Next, Some(720)));
        assert_eq!(
            parse_mute("until tomorrow"),
            until(UntilDay::Tomorrow, None)
        );
        assert_eq!(
            parse_mute("until tomorrow at 9 am"),
            until(UntilDay::Tomorrow, Some(540))
        );
        assert_eq!(
            parse_mute("until monday"),
            until(UntilDay::On(Weekday::Mon), None)
        );
        assert_eq!(
            parse_mute("till fri 2pm"),
            until(UntilDay::On(Weekday::Fri), Some(840))
        );
        for bad in [
            "",
            "abc",
            "0m",
            "9",
            "5x",
            "until",
            "until 9",
            "until 25:00",
            "until 13pm",
            "until 9:5am",
            "until someday",
            "until monday 9",
            "15d",
            "300d",
        ] {
            assert!(parse_mute(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn mute_until_resolves_in_the_owners_time_zone() {
        // Thursday 20:30 PDT.
        let evening = WORKDAY_EVENING;
        assert_eq!(until("2h", evening), Ok(at("2026-10-02T05:30:00Z")));
        // Next 9am is Friday morning; `tomorrow` means the start of work.
        assert_eq!(until("until 9am", evening), Ok(at("2026-10-02T16:00:00Z")));
        assert_eq!(
            until("until tomorrow", evening),
            Ok(at("2026-10-02T16:00:00Z"))
        );
        // 9pm is still ahead tonight.
        assert_eq!(until("until 9pm", evening), Ok(at("2026-10-02T04:00:00Z")));
        assert_eq!(
            until("until monday", evening),
            Ok(at("2026-10-05T16:00:00Z"))
        );
        assert!(
            until("until today 5pm", evening)
                .unwrap_err()
                .contains("already passed")
        );
        // Thursday 10:00 PDT: today's 9am has passed, so `thursday` is next week.
        assert_eq!(
            until("until thursday", WORKDAY_MORNING),
            Ok(at("2026-10-08T16:00:00Z"))
        );
        assert_eq!(
            until("until today 5pm", WORKDAY_MORNING),
            Ok(at("2026-10-02T00:00:00Z"))
        );
    }

    #[test]
    fn mute_until_is_dst_correct_across_fall_back() {
        // Saturday 2026-10-31 20:00 PDT; Monday 2026-11-02 09:00 is PST (UTC-8).
        let saturday = "2026-11-01T03:00:00Z";
        assert_eq!(
            until("until monday", saturday),
            Ok(at("2026-11-02T17:00:00Z"))
        );
        // Spring forward: Saturday 2026-03-07 20:00 PST → Monday 09:00 PDT (UTC-7).
        assert_eq!(
            until("until monday", "2026-03-08T04:00:00Z"),
            Ok(at("2026-03-09T16:00:00Z"))
        );
    }

    fn reply(applied: super::Applied) -> (String, &'static str) {
        match applied {
            super::Applied::Reply { text, outcome } => (text, outcome),
            other => panic!("not a reply: {other:?}"),
        }
    }

    #[test]
    fn controls_mute_unmute_and_request_a_digest_through_the_policy() {
        use super::{Applied, Control, apply};
        use crate::slack::{
            detector::AttentionKind,
            policy::{DigestKind, Policy, tests as p},
        };
        let temp = tempfile::TempDir::new().unwrap();
        let clock = p::TestClock::at(WORKDAY_MORNING);
        let mut policy = Policy::new(p::parts(&clock, Some(&temp)));

        let (text, outcome) = reply(apply(
            &mut policy,
            Control::Mute(MuteSpec::For(Duration::from_secs(HOUR))),
        ));
        assert_eq!(outcome, "answered");
        assert!(
            text.contains("Muted until *Thu Oct 1, 11:00 AM PDT*"),
            "{text}"
        );
        assert!(policy.holding(policy.now()));
        // A blocked agent is held while muted, and the mute survives a restart.
        policy.admit(&p::event("w-1", AttentionKind::Blocked), None);
        assert_eq!(policy.held_count(), 1);
        let mut policy = Policy::new(p::parts(&clock, Some(&temp)));
        assert_eq!(policy.status().muted_until, Some(1_790_877_600_000));

        let (text, _) = reply(apply(&mut policy, Control::Unmute));
        assert!(text.contains("What I held follows in a digest"), "{text}");
        assert_eq!(policy.muted_until(policy.now()), None);
        assert_eq!(policy.digest_due(), Some(DigestKind::Away));
        let (text, _) = reply(apply(&mut policy, Control::Unmute));
        assert_eq!(text, "Yard was not muted.");

        // A mute ending at night says when the digest comes instead.
        clock.set(WORKDAY_EVENING);
        let (text, _) = reply(apply(
            &mut policy,
            Control::Mute(MuteSpec::For(Duration::from_secs(HOUR))),
        ));
        assert!(
            text.contains("the digest comes at Fri Oct 2, 9:00 AM PDT"),
            "{text}"
        );
        let (text, _) = reply(apply(&mut policy, Control::Unmute));
        assert!(text.contains("outside working hours"), "{text}");
        assert!(text.contains("Fri Oct 2, 9:00 AM PDT"), "{text}");

        // A refused mute changes nothing.
        let (_, outcome) = reply(apply(
            &mut policy,
            Control::Mute(MuteSpec::Until {
                day: UntilDay::Today,
                minutes: Some(9 * 60),
            }),
        ));
        assert_eq!(outcome, "refused:invalid");
        assert_eq!(policy.muted_until(policy.now()), None);

        // `digest` at night: requested, so it goes out now anyway.
        assert!(matches!(
            apply(&mut policy, Control::Digest),
            Applied::DigestRequested
        ));
        assert_eq!(policy.digest_due(), Some(DigestKind::Requested));
    }

    #[test]
    fn the_settings_card_shows_the_schedule_and_offers_opaque_buttons() {
        use super::{Applied, Control, apply, settings_card};
        use crate::slack::{
            actions::NavAction,
            detector::AttentionKind,
            policy::{Policy, tests as p},
        };
        let clock = p::TestClock::at(WORKDAY_EVENING);
        let mut policy = Policy::new(p::parts(&clock, None));
        policy.admit(&p::event("w-1", AttentionKind::Blocked), None);
        let Applied::Settings(snapshot) = apply(&mut policy, Control::Settings) else {
            panic!("settings card");
        };
        let mut issued = Vec::new();
        let card = settings_card(
            &snapshot,
            &mut |action| {
                issued.push(action);
                Some(format!("{:032x}", issued.len()))
            },
            "https://yard.example.test/",
        );
        let json = card.blocks.to_string();
        for needle in [
            "Quiet now",
            "Fri Oct 2, 9:00 AM PDT",
            "mon-fri 09:00-18:00",
            "America/Los_Angeles",
            "blocked",
            "at most every 120 min",
            "(1 held)",
        ] {
            assert!(json.contains(needle), "{needle}: {json}");
        }
        assert!(card.text.contains("mute: off"), "{}", card.text);
        let labels = [
            "Mute 1h",
            "Mute until tomorrow 9am",
            "Mute until Monday",
            "Unmute",
            "Digest now",
        ];
        for label in labels {
            assert!(json.contains(label), "{label}");
        }
        assert_eq!(issued.len(), 5);
        assert!(
            issued
                .iter()
                .all(|action| matches!(action, NavAction::Quiet(_)))
        );
        // Values are only the opaque ids, never the control.
        assert!(json.contains(&format!("{:032x}", 1)));
        assert!(!json.contains("Tomorrow"), "{json}");

        // Without ids (no RNG) the card still renders, without the buttons.
        let bare = settings_card(&snapshot, &mut |_| None, "https://yard.example.test/");
        assert!(!bare.blocks.to_string().contains("Mute 1h"));
    }
}

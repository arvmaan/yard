//! The digest card for held notifications: grouped by project, an "Answer"
//! button for each agent that is still blocked (the existing navigation
//! path, which re-reads the agent and posts its prompt card), compact counted
//! lines for the rest, and "+N more — open Yard" past Slack's limits.
//! Content is re-validated by the caller just before this is built.

use std::{fmt::Write as _, time::Instant};

use serde_json::Value;
use tracing::debug;

use super::{
    Delivery,
    actions::{DIGEST_ACTION_TTL, NavAction},
    blocks::{self, Style},
    commands, delivery_retry_delay,
    detector::{AttentionEvent, AttentionKind, EventSource},
    hub::event_target,
    lock,
    message::{NAME_CHARS, OutgoingMessage, clean, plural, subject_name},
    outbox::ThreadKey,
    policy::{Admission, DigestKind},
    prompt::parse_screen,
    unix_ms,
    views::{Nav, nav_button},
};

/// Projects with their own section.
pub const MAX_DIGEST_PROJECTS: usize = 10;
/// "Answer" buttons per digest.
pub const MAX_ANSWER_BUTTONS: usize = 10;
/// Named agents per compact line.
const MAX_NAMES: usize = 3;

struct Group<'a> {
    project: Option<&'a str>,
    events: Vec<&'a AttentionEvent>,
}

fn kind_order(kind: AttentionKind) -> u8 {
    match kind {
        AttentionKind::Blocked => 0,
        AttentionKind::CommandFailed => 1,
        AttentionKind::CommandAmbiguous => 2,
        AttentionKind::ReadyForReview => 3,
    }
}

fn kind_line(kind: AttentionKind, count: usize) -> String {
    match kind {
        AttentionKind::Blocked => format!(":raised_hand: {count} waiting for input"),
        AttentionKind::ReadyForReview => format!(":eyes: {count} ready for review"),
        AttentionKind::CommandFailed => format!(
            ":x: {count} {} failed",
            plural(count, "command", "commands")
        ),
        AttentionKind::CommandAmbiguous => format!(
            ":grey_question: {count} {} unclear",
            plural(count, "outcome", "outcomes")
        ),
    }
}

/// `None` when there is nothing to say (and the owner did not ask).
#[must_use]
pub fn digest_message(
    kind: DigestKind,
    events: &[AttentionEvent],
    dropped: usize,
    mut nav: Option<&mut Nav<'_>>,
    ui_url: &str,
) -> Option<OutgoingMessage> {
    if events.is_empty() && dropped == 0 && kind != DigestKind::Requested {
        return None;
    }
    let title = match kind {
        DigestKind::Away => ":sunny: While you were away",
        DigestKind::Periodic | DigestKind::Requested => ":bell: Yard digest",
    };
    let mut groups: Vec<Group<'_>> = Vec::new();
    for event in events {
        let project = event.project_name.as_deref();
        match groups.iter_mut().find(|group| group.project == project) {
            Some(group) => group.events.push(event),
            None => groups.push(Group {
                project,
                events: vec![event],
            }),
        }
    }
    let mut totals = [0usize; 4];
    for event in events {
        totals[usize::from(kind_order(event.kind))] += 1;
    }
    let summary = [
        AttentionKind::Blocked,
        AttentionKind::CommandFailed,
        AttentionKind::CommandAmbiguous,
        AttentionKind::ReadyForReview,
    ]
    .into_iter()
    .filter(|kind| totals[usize::from(kind_order(*kind))] > 0)
    .map(|kind| kind_line(kind, totals[usize::from(kind_order(kind))]))
    .collect::<Vec<_>>();
    let text = if summary.is_empty() {
        format!("{} — nothing new", title_plain(kind))
    } else {
        format!("{}: {}", title_plain(kind), plain_summary(&totals))
    };

    let mut out = vec![blocks::header(title)];
    if summary.is_empty() {
        out.push(blocks::section(
            "Nothing new — check Yard for the full picture.",
        ));
    } else {
        out.push(blocks::context(&summary));
    }
    let mut buttons = 0;
    let mut hidden = 0;
    for (index, group) in groups.iter().enumerate() {
        if index >= MAX_DIGEST_PROJECTS {
            hidden += group.events.len();
            continue;
        }
        project_blocks(group, &mut nav, &mut buttons, &mut out);
    }
    let mut notes = Vec::new();
    if buttons > 0 {
        notes.push(format!(
            "\"Answer\" buttons work once and expire in {} h.",
            DIGEST_ACTION_TTL.as_secs() / 3600
        ));
    }
    if hidden > 0 {
        notes.push(format!("+{hidden} more — open Yard."));
    }
    if dropped > 0 {
        notes.push(format!(
            "{dropped} older {} not kept — check Yard.",
            plural(dropped, "update was", "updates were")
        ));
    }
    if !notes.is_empty() {
        out.push(blocks::context(&notes));
    }
    out.push(blocks::actions(
        "yard_links",
        vec![blocks::open_in_yard(ui_url)],
    ));
    Some(OutgoingMessage {
        text,
        blocks: blocks::finish(out),
    })
}

/// One project's section: a counted line per kind (blocked agents beyond
/// the "Answer" buttons included), then one line with "Answer" per blocked
/// agent while buttons last, so the card stays inside Slack's block limit.
fn project_blocks(
    group: &Group<'_>,
    nav: &mut Option<&mut Nav<'_>>,
    buttons: &mut usize,
    out: &mut Vec<Value>,
) {
    let name = group
        .project
        .map_or_else(|| "Yard".to_owned(), |project| clean(project, NAME_CHARS));
    let mut answers = Vec::new();
    let mut by_kind: Vec<(AttentionKind, Vec<&AttentionEvent>)> = Vec::new();
    for event in &group.events {
        if event.kind == AttentionKind::Blocked && *buttons < MAX_ANSWER_BUTTONS {
            let button = nav.as_deref_mut().and_then(|nav| {
                let target = event_target(event)?;
                nav_button(
                    nav,
                    NavAction::Answer {
                        target,
                        title: commands::event_title(event),
                    },
                    "Answer",
                    Style::Primary,
                )
            });
            if let Some(button) = button {
                *buttons += 1;
                let line = format!(
                    ":raised_hand: *{}* is waiting for input",
                    subject_name(&event.subject)
                );
                answers.push(blocks::section_with(&line, button));
                continue;
            }
        }
        match by_kind.iter_mut().find(|(kind, _)| *kind == event.kind) {
            Some((_, list)) => list.push(event),
            None => by_kind.push((event.kind, vec![event])),
        }
    }
    by_kind.sort_by_key(|(kind, _)| kind_order(*kind));
    let mut lines = vec![format!("*{name}*")];
    for (kind, list) in &by_kind {
        let mut line = kind_line(*kind, list.len());
        let names = list
            .iter()
            .take(MAX_NAMES)
            .map(|event| subject_name(&event.subject))
            .collect::<Vec<_>>();
        let _ = write!(line, " — {}", names.join(", "));
        if list.len() > MAX_NAMES {
            let _ = write!(line, ", +{}", list.len() - MAX_NAMES);
        }
        lines.push(line);
    }
    out.push(blocks::section(&lines.join("\n")));
    out.extend(answers);
}

const fn title_plain(kind: DigestKind) -> &'static str {
    match kind {
        DigestKind::Away => "While you were away",
        DigestKind::Periodic | DigestKind::Requested => "Yard digest",
    }
}

fn plain_summary(totals: &[usize; 4]) -> String {
    let parts = [
        (totals[0], "waiting for input"),
        (totals[1], "failed"),
        (totals[2], "unclear"),
        (totals[3], "ready for review"),
    ];
    parts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, label)| format!("{count} {label}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl super::SlackNotifier {
    /// Run detected events through the quiet policy; what may go out now
    /// joins the send queue, the rest is held for a digest or dropped.
    pub(super) async fn admit(&self, events: Vec<AttentionEvent>) {
        if events.is_empty() {
            return;
        }
        let holding = {
            let policy = lock(&self.inner.policy);
            policy.holding(policy.now())
        };
        let mut fingerprints = Vec::with_capacity(events.len());
        for event in &events {
            let instant_blocked = !holding
                && event.kind == AttentionKind::Blocked
                && matches!(event.source, EventSource::Runtime { .. });
            fingerprints.push(if instant_blocked {
                self.prompt_fingerprint(event).await
            } else {
                None
            });
        }
        let mut instant = Vec::new();
        {
            let mut policy = lock(&self.inner.policy);
            for (event, fingerprint) in events.into_iter().zip(fingerprints) {
                match policy.admit(&event, fingerprint.as_deref()) {
                    Admission::Instant => instant.push(event),
                    Admission::Held => {
                        debug!(kind = ?event.kind, "Slack notification held for the digest");
                    }
                    Admission::Dropped(reason) => {
                        debug!(kind = ?event.kind, ?reason, "Slack notification dropped as noise");
                    }
                }
            }
        }
        lock(&self.inner.outbox).push(instant);
    }

    /// The prompt on a blocked agent's screen, hashed (inbound only: the
    /// console reads the terminal). `None` when it can't be read.
    async fn prompt_fingerprint(&self, event: &AttentionEvent) -> Option<String> {
        if !self.inbound_enabled() {
            return None;
        }
        let console = self.console()?;
        let target = event_target(event)?;
        let screen = console.screen(&target).await.ok()?;
        Some(parse_screen(&screen).prompt_fingerprint())
    }

    /// Post the digest when it is due: re-check every held item (still
    /// true, not open in Yard), and post nothing when none is left.
    pub(super) async fn deliver_digest(&self, delivery: &mut Delivery) {
        if !lock(&self.inner.outbox).can_send(Instant::now()) {
            return;
        }
        let Some(kind) = lock(&self.inner.policy).digest_due() else {
            return;
        };
        self.send_digest(delivery, kind, None).await;
    }

    /// Re-check and post everything held as one digest: in `reply_in` (the
    /// thread of the owner's `digest`, never held or delayed) or in the
    /// Yard thread. A failed post keeps the entries held, and a digest the
    /// owner asked for is asked for again.
    pub(super) async fn send_digest(
        &self,
        delivery: &mut Delivery,
        kind: DigestKind,
        reply_in: Option<&str>,
    ) {
        let taken = lock(&self.inner.policy).take_digest();
        let events = {
            let detector = lock(&self.inner.detector);
            let presence = &self.inner.presence;
            taken
                .entries
                .iter()
                .filter(|entry| {
                    detector.still_true(&entry.event)
                        && !entry
                            .event
                            .view_target
                            .as_ref()
                            .is_some_and(|target| presence.is_viewing(target, Instant::now()))
                })
                .map(|entry| entry.event.clone())
                .collect::<Vec<_>>()
        };
        let answers = self.inbound_enabled() && self.console().is_some();
        let message = {
            // A digest is read later: its "Answer" buttons last longer.
            let mut issuer = self.nav_issuer_for(Some(DIGEST_ACTION_TTL));
            let nav: Option<&mut Nav<'_>> = if answers { Some(&mut issuer) } else { None };
            digest_message(
                kind,
                &events,
                taken.dropped,
                nav,
                &self.inner.inbound.ui_url,
            )
        };
        let Some(message) = message else {
            debug!("Every held Slack notification resolved; no digest");
            lock(&self.inner.policy).digest_sent(&taken);
            return;
        };
        let posted = match reply_in {
            Some(thread) => delivery.post_inbound(&message, Some(thread)).await,
            None => {
                delivery
                    .post(Some(&ThreadKey::Yard), &message, unix_ms())
                    .await
            }
        };
        match (posted, reply_in) {
            (Ok(_), reply_in) => {
                if reply_in.is_none() {
                    *lock(&self.inner.delivery_failures) = 0;
                    lock(&self.inner.outbox).sent(Instant::now());
                }
                lock(&self.inner.policy).digest_sent(&taken);
                self.record_sent();
            }
            (Err(error), Some(_)) => {
                lock(&self.inner.policy).digest_failed(kind);
                self.log_failure(
                    "inbound reply",
                    &format!("Posting the requested Slack digest failed: {error}"),
                );
            }
            (Err(error), None) => {
                let failures = {
                    let mut failures = lock(&self.inner.delivery_failures);
                    *failures = failures.saturating_add(1);
                    *failures
                };
                let not_before = Instant::now() + delivery_retry_delay(&error, failures);
                lock(&self.inner.outbox).hold_off(not_before);
                lock(&self.inner.policy).digest_failed(kind);
                self.record_delivery_error(&error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use yard_domain::ObservedStatus;

    use super::{MAX_ANSWER_BUTTONS, MAX_DIGEST_PROJECTS, digest_message};
    use crate::slack::{
        actions::NavAction,
        detector::{AttentionEvent, AttentionKind, EventSource, Subject},
        policy::DigestKind,
        presence::ViewTarget,
    };

    fn event(project: &str, worker: &str, kind: AttentionKind) -> AttentionEvent {
        AttentionEvent {
            kind,
            source: EventSource::Runtime {
                worker_id: worker.to_owned(),
                status: ObservedStatus::Blocked,
            },
            project_id: Some(format!("id-{project}")),
            project_name: Some(project.to_owned()),
            subject: Subject::Worker {
                profile_name: Some(worker.to_owned()),
                objective: None,
            },
            view_target: Some(ViewTarget::Assignment(format!("a-{worker}"))),
            observed_at_unix_ms: 1,
            ready_at: Instant::now(),
            automatic: false,
        }
    }

    #[test]
    fn groups_by_project_with_answer_buttons_and_counted_lines() {
        let events = vec![
            event("Checkout", "alpha", AttentionKind::Blocked),
            event("Search", "beta", AttentionKind::ReadyForReview),
            event("Checkout", "gamma", AttentionKind::ReadyForReview),
            event("Checkout", "delta", AttentionKind::ReadyForReview),
            event("Checkout", "eps", AttentionKind::CommandFailed),
        ];
        let mut issued = Vec::new();
        let mut nav = |action: NavAction| {
            issued.push(action);
            Some(format!("{:032x}", issued.len()))
        };
        let message =
            digest_message(DigestKind::Away, &events, 0, Some(&mut nav), "https://y/").unwrap();
        assert_eq!(
            message.text,
            "While you were away: 1 waiting for input, 1 failed, 3 ready for review"
        );
        let blocks = message.blocks.to_string();
        assert!(blocks.contains(":sunny: While you were away"), "{blocks}");
        assert!(
            blocks.contains(":eyes: 2 ready for review — gamma, delta"),
            "{blocks}"
        );
        assert!(blocks.contains(":x: 1 command failed — eps"), "{blocks}");
        assert!(blocks.contains("*alpha* is waiting for input"), "{blocks}");
        assert!(blocks.contains("\"Answer\""), "{blocks}");
        assert!(blocks.find("Checkout").unwrap() < blocks.find("Search").unwrap());
        assert!(matches!(
            issued.as_slice(),
            [NavAction::Answer { title, .. }] if title.contains("alpha")
        ));

        // Without inbound there is no button; the line stays.
        let message = digest_message(DigestKind::Periodic, &events, 0, None, "https://y/").unwrap();
        let blocks = message.blocks.to_string();
        assert!(!blocks.contains("\"Answer\""), "{blocks}");
        assert!(blocks.contains(":bell: Yard digest"), "{blocks}");
    }

    #[test]
    fn empty_is_nothing_unless_asked_and_overflow_says_open_yard() {
        assert!(digest_message(DigestKind::Away, &[], 0, None, "https://y/").is_none());
        assert!(digest_message(DigestKind::Periodic, &[], 0, None, "https://y/").is_none());
        let asked = digest_message(DigestKind::Requested, &[], 0, None, "https://y/").unwrap();
        assert_eq!(asked.text, "Yard digest — nothing new");

        let events = (0..MAX_DIGEST_PROJECTS + 3)
            .map(|index| {
                event(
                    &format!("P{index}"),
                    &format!("w{index}"),
                    AttentionKind::ReadyForReview,
                )
            })
            .collect::<Vec<_>>();
        let message = digest_message(DigestKind::Periodic, &events, 2, None, "https://y/").unwrap();
        let blocks = message.blocks.to_string();
        assert!(blocks.contains("+3 more — open Yard."), "{blocks}");
        assert!(blocks.contains("2 older updates were not kept"), "{blocks}");
        assert!(!blocks.contains("*P11*"), "{blocks}");
    }

    #[test]
    fn many_blocked_agents_fold_into_a_counted_line_and_keep_the_footer() {
        let events = (0..60)
            .map(|index| event("Checkout", &format!("w{index}"), AttentionKind::Blocked))
            .collect::<Vec<_>>();
        let mut issued = 0;
        let mut nav = |_action: NavAction| {
            issued += 1;
            Some(format!("{issued:032x}"))
        };
        let message =
            digest_message(DigestKind::Away, &events, 4, Some(&mut nav), "https://y/").unwrap();
        let blocks = message.blocks.as_array().unwrap();
        assert!(
            blocks.len() < crate::slack::blocks::MAX_BLOCKS,
            "{}",
            blocks.len()
        );
        let json = message.blocks.to_string();
        assert_eq!(json.matches("\"Answer\"").count(), MAX_ANSWER_BUTTONS);
        assert!(
            json.contains(":raised_hand: 50 waiting for input — w10, w11, w12, +47"),
            "{json}"
        );
        assert!(json.contains("4 older updates were not kept"), "{json}");
        assert!(json.contains("Open in Yard"), "{json}");
        assert!(!json.contains("more — open Yard"), "{json}");
        // The digest is read later: it says how long its buttons work.
        assert!(
            json.contains("\\\"Answer\\\" buttons work once and expire in 9 h."),
            "{json}"
        );

        // Without inbound: one counted line, no per-agent blocks.
        let message = digest_message(DigestKind::Away, &events, 0, None, "https://y/").unwrap();
        let json = message.blocks.to_string();
        assert!(
            json.contains("60 waiting for input — w0, w1, w2, +57"),
            "{json}"
        );
        assert!(!json.contains("expire"), "{json}");
    }
}

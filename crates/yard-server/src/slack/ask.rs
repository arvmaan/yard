//! Block Kit cards for owner questions to the Superintendent or a project
//! orchestrator (see [`super::relay`] for the watch itself).
//!
//! One progress message per question is posted at once and then updated in
//! place (chat.update): asking → still waiting (throttled) → answered, or
//! an error / timeout card with "Retry" / "Check again". Those buttons are
//! navigation buttons built by the caller (opaque, single-use ids issued
//! server-side); `None` (RNG failure) leaves the button out. The answer
//! itself is its own message in the thread.

use std::time::Duration;

use serde_json::Value;
use yard_domain::{OrchestratorStatusReport, OrchestratorStatusState};

use super::{
    blocks,
    message::{OutgoingMessage, clean_prompt_line, since_token},
    prompt::cap,
    relay,
};

/// Answer text relayed in total (split into sections of [`SECTION_CHARS`]).
pub const MAX_RELAY_CHARS: usize = 12_000;
/// One answer section (Slack allows 3,000 per text).
pub const SECTION_CHARS: usize = 2_900;

/// "4 min", "1 h 5 min", "12 s".
#[must_use]
pub fn duration_words(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3_600 => format!("{} min", seconds / 60),
        _ if seconds % 3_600 < 60 => format!("{} h", seconds / 3_600),
        _ => format!("{} h {} min", seconds / 3_600, seconds % 3_600 / 60),
    }
}

/// "the Superintendent" → "The Superintendent".
#[must_use]
pub fn title(to: &str) -> String {
    let mut chars = to.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The answer header name: "the Superintendent" → "Superintendent".
fn speaker(to: &str) -> String {
    title(to.strip_prefix("the ").unwrap_or(to))
}

fn links(buttons: Vec<Option<Value>>, ui_url: &str) -> Value {
    let mut elements = buttons.into_iter().flatten().collect::<Vec<_>>();
    elements.push(blocks::open_in_yard(ui_url));
    blocks::actions("yard_ask", elements)
}

/// Posted at once, in the question's thread.
#[must_use]
pub fn asking_card(to: &str, at_unix_ms: u64, ui_url: &str) -> OutgoingMessage {
    OutgoingMessage {
        text: format!("Asking {to}… Its answer will be posted in this thread."),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":thinking_face: *Asking {to}…*")),
            blocks::context(&[
                format!("Sent from Slack · {}", since_token(at_unix_ms)),
                "The answer will be posted in this thread.".to_owned(),
            ]),
            links(Vec::new(), ui_url),
        ]),
    }
}

/// The throttled "still waiting" update of the progress message.
#[must_use]
pub fn progress_card(
    to: &str,
    elapsed: Duration,
    state: &str,
    window: Duration,
    ui_url: &str,
) -> OutgoingMessage {
    let waited = duration_words(elapsed);
    OutgoingMessage {
        text: format!("Still waiting for {to} ({waited}, {state})."),
        blocks: blocks::finish(vec![
            blocks::section(&format!(
                ":hourglass_flowing_sand: *Waiting for {to}* · {waited}"
            )),
            blocks::context(&[
                format!("Latest: {state}"),
                format!("Yard watches for up to {}.", duration_words(window)),
            ]),
            links(Vec::new(), ui_url),
        ]),
    }
}

/// The question was not sent: the agent is not reachable.
#[must_use]
pub fn unreachable_card(
    to: &str,
    reason: &str,
    retry: Option<Value>,
    ui_url: &str,
) -> OutgoingMessage {
    let reason = clean_prompt_line(reason);
    OutgoingMessage {
        text: format!(
            "{} isn't running ({reason}). Your question was not sent.",
            title(to)
        ),
        blocks: blocks::finish(vec![
            blocks::section(&format!(
                ":warning: *{} isn't running* ({reason}). Your question was not sent.",
                title(to)
            )),
            blocks::context(&["Start it in Yard, then press Retry.".to_owned()]),
            links(vec![retry], ui_url),
        ]),
    }
}

/// The question was not sent, though the agent runs (busy terminal, a
/// prompt waiting, a failed read).
#[must_use]
pub fn not_sent_card(
    to: &str,
    reason: &str,
    retry: Option<Value>,
    ui_url: &str,
) -> OutgoingMessage {
    let reason = clean_prompt_line(reason);
    OutgoingMessage {
        text: format!("Your question to {to} was not sent: {reason}."),
        blocks: blocks::finish(vec![
            blocks::section(&format!(
                ":warning: *Your question to {to} was not sent* — {reason}."
            )),
            blocks::context(&["Press Retry to send it again.".to_owned()]),
            links(vec![retry], ui_url),
        ]),
    }
}

/// Watching failed: the agent can't answer (disconnected, exited, …).
#[must_use]
pub fn failed_card(
    to: &str,
    reason: &str,
    elapsed: Duration,
    retry: Option<Value>,
    ui_url: &str,
) -> OutgoingMessage {
    let reason = clean_prompt_line(reason);
    OutgoingMessage {
        text: format!("{} can't answer right now: {reason}.", title(to)),
        blocks: blocks::finish(vec![
            blocks::section(&format!(
                ":x: *{} can't answer right now* — {reason}.",
                title(to)
            )),
            blocks::context(&[format!(
                "Noticed after {}. Retry sends the same question again.",
                duration_words(elapsed)
            )]),
            links(vec![retry], ui_url),
        ]),
    }
}

/// The window ended without an answer.
#[must_use]
pub fn timed_out_card(
    to: &str,
    window: Duration,
    late: Option<Duration>,
    check: Option<Value>,
    ui_url: &str,
) -> OutgoingMessage {
    let window = duration_words(window);
    let later = late.map_or_else(
        || "Check again re-reads its terminal now.".to_owned(),
        |late| {
            format!(
                "If it answers within {} more, the answer is still posted here. Check again re-reads its terminal now.",
                duration_words(late)
            )
        },
    );
    OutgoingMessage {
        text: format!("No answer from {to} after {window}. {later}"),
        blocks: blocks::finish(vec![
            blocks::section(&format!(
                ":hourglass: *No answer from {to} after {window}.*"
            )),
            blocks::context(&[later]),
            links(vec![check], ui_url),
        ]),
    }
}

/// "Check again" found nothing yet.
#[must_use]
pub fn still_nothing(to: &str, check: Option<Value>, ui_url: &str) -> OutgoingMessage {
    let text = format!("{} has not answered this question yet.", title(to));
    OutgoingMessage {
        text: text.clone(),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":hourglass: {text}")),
            links(vec![check], ui_url),
        ]),
    }
}

/// Shutdown while a question was watched: its answer won't be posted.
#[must_use]
pub fn restarted_card(to: &str, ui_url: &str) -> OutgoingMessage {
    let text = format!(
        "Yard restarted before {to} answered, so the answer won't be posted here. Open Yard to see it, or ask again."
    );
    OutgoingMessage {
        text: text.clone(),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":arrows_counterclockwise: {text}")),
            links(vec![], ui_url),
        ]),
    }
}

/// Sent, but every watch slot is busy: the owner checks with "Check
/// again" (or Yard) instead.
#[must_use]
pub fn not_watched_card(to: &str, check: Option<Value>, ui_url: &str) -> OutgoingMessage {
    let text = format!(
        "Sent to {to}. Yard is already waiting on several answers, so it won't post this one by itself: press Check again later, or open Yard."
    );
    OutgoingMessage {
        text: text.clone(),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":outbox_tray: {text}")),
            links(vec![check], ui_url),
        ]),
    }
}

/// The progress message once the answer was relayed (no buttons).
#[must_use]
pub fn answered_progress(to: &str, elapsed: Duration) -> OutgoingMessage {
    let waited = duration_words(elapsed);
    OutgoingMessage {
        text: format!("{} answered in {waited}.", title(to)),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":white_check_mark: *Answered in {waited}*")),
            blocks::context(&[format!("*{}*", title(to))]),
        ]),
    }
}

/// One cleaned line with common Markdown turned into Slack mrkdwn:
/// headings and `**bold**` → `*bold*`, list markers → `•`,
/// `[text](url)` → `text (url)`. Runs after redaction and escaping, so it
/// only rearranges characters the agent wrote.
#[must_use]
pub fn mrkdwn_line(line: &str) -> String {
    let line = clean_prompt_line(line);
    let line = match line.trim_start_matches('#') {
        rest if rest.len() < line.len() && rest.starts_with(' ') => {
            format!("*{}*", rest.trim().replace("**", ""))
        }
        _ => line.clone(),
    };
    let line = ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
        .map_or_else(|| line.clone(), |rest| format!("• {rest}"));
    let mut out = line.replace("**", "*").replace("__", "_");
    while let Some(open) = out.find("](") {
        let Some(start) = out[..open].rfind('[') else {
            break;
        };
        let Some(close) = out[open..].find(')').map(|at| open + at) else {
            break;
        };
        let replaced = format!("{} ({})", &out[start + 1..open], &out[open + 2..close]);
        out.replace_range(start..=close, &replaced);
    }
    out
}

/// Multi-line text converted line by line, blank runs collapsed.
fn mrkdwn(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines().map(mrkdwn_line) {
        if !(line.is_empty() && lines.last().is_none_or(String::is_empty)) {
            lines.push(line);
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// `text` split at line (or word) ends into parts of at most `max` chars.
fn split(text: &str, max: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    for line in text.lines() {
        let mut line = line.to_owned();
        while line.chars().count() > max {
            let cut = line
                .char_indices()
                .take_while(|(index, _)| line[..*index].chars().count() < max)
                .filter(|(_, character)| *character == ' ')
                .map(|(index, _)| index)
                .last()
                .filter(|index| *index > 0)
                .unwrap_or_else(|| {
                    line.char_indices()
                        .nth(max)
                        .map_or(line.len(), |(index, _)| index)
                });
            if !part.is_empty() {
                parts.push(std::mem::take(&mut part));
            }
            parts.push(line[..cut].trim_end().to_owned());
            line = line[cut..].trim_start().to_owned();
        }
        if part.chars().count() + line.chars().count() + 1 > max && !part.is_empty() {
            parts.push(std::mem::take(&mut part));
        }
        if !part.is_empty() {
            part.push('\n');
        }
        part.push_str(&line);
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// The relayed answer: header with the speaker, the answer in sections of
/// at most [`SECTION_CHARS`], next step and blockers, a state context.
/// Redacted and escaped line by line; past [`MAX_RELAY_CHARS`] it ends with
/// "(truncated)". `text` is the plain [`relay::report_text`].
#[must_use]
pub fn answer_message(to: &str, report: &OrchestratorStatusReport) -> OutgoingMessage {
    let state = match report.state {
        OrchestratorStatusState::Working => ":large_green_circle: working",
        OrchestratorStatusState::NeedsAttention => ":large_yellow_circle: needs attention",
        OrchestratorStatusState::Idle => ":white_circle: idle",
    };
    let mut body = mrkdwn(&report.last);
    let next = mrkdwn(&report.next);
    if !next.is_empty() {
        body.push_str("\n\n*Next:* ");
        body.push_str(&next);
    }
    let blockers = report
        .blockers
        .iter()
        .map(|blocker| format!("• {}", mrkdwn(blocker)))
        .collect::<Vec<_>>();
    if !blockers.is_empty() {
        body.push_str("\n\n*Blockers:*\n");
        body.push_str(&blockers.join("\n"));
    }
    if body.trim().is_empty() {
        "_(empty answer)_".clone_into(&mut body);
    }
    let body = cap(&body, MAX_RELAY_CHARS);
    let mut blocks = vec![blocks::header(&format!(":speech_balloon: {}", speaker(to)))];
    blocks.extend(
        split(&body, SECTION_CHARS)
            .iter()
            .map(|part| blocks::section(part)),
    );
    blocks.push(blocks::context(&[format!("Status: {state}")]));
    OutgoingMessage {
        text: relay::report_text(to, report),
        blocks: blocks::finish(blocks),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        MAX_RELAY_CHARS, SECTION_CHARS, answer_message, answered_progress, asking_card,
        duration_words, failed_card, mrkdwn_line, progress_card, timed_out_card, unreachable_card,
    };
    use crate::slack::{blocks, relay};

    const URL: &str = "https://yard.example.test/";

    fn report(last: &str) -> yard_domain::OrchestratorStatusReport {
        relay::scan_report(&relay::tests::report_line("c-1", last), "c-1").unwrap()
    }

    fn all_text(message: &crate::slack::message::OutgoingMessage) -> String {
        message.blocks.to_string()
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_words(Duration::from_secs(12)), "12 s");
        assert_eq!(duration_words(Duration::from_secs(4 * 60 + 5)), "4 min");
        assert_eq!(duration_words(Duration::from_secs(7_200)), "2 h");
        assert_eq!(duration_words(Duration::from_secs(3_900)), "1 h 5 min");
    }

    #[test]
    fn progress_cards_cover_the_lifecycle() {
        let asking = asking_card("the Superintendent", 1_695_900_000_000, URL);
        assert!(all_text(&asking).contains(":thinking_face: *Asking the Superintendent…*"));
        assert_eq!(asking.blocks[2]["elements"][0]["url"], URL);
        let waiting = progress_card(
            "the Superintendent",
            Duration::from_secs(180),
            "running tools",
            Duration::from_secs(7_200),
            URL,
        );
        assert!(all_text(&waiting).contains("Waiting for the Superintendent* · 3 min"));
        assert!(all_text(&waiting).contains("Latest: running tools"));
        assert!(all_text(&waiting).contains("up to 2 h"));
        let retry = blocks::button(
            "Retry",
            "yard_nav_retry_1",
            "0".repeat(32).as_str(),
            blocks::Style::Primary,
        );
        let down = unreachable_card(
            "the Superintendent",
            "its agent process exited <!here>",
            Some(retry.clone()),
            URL,
        );
        assert!(
            down.text.starts_with(
                "The Superintendent isn't running (its agent process exited &lt;!here&gt;)"
            ),
            "{}",
            down.text
        );
        let row = down.blocks[2]["elements"].as_array().unwrap();
        assert_eq!(row[0]["text"]["text"], "Retry");
        assert_eq!(row[1]["action_id"], blocks::OPEN_ACTION_ID);
        let failed = failed_card(
            "the Superintendent",
            "Codex: Connection lost",
            Duration::from_secs(70),
            Some(retry),
            URL,
        );
        assert!(
            failed
                .text
                .contains("can't answer right now: Codex: Connection lost")
        );
        assert!(all_text(&failed).contains("Noticed after 1 min"));
        let timed = timed_out_card(
            "the Superintendent",
            Duration::from_secs(7_200),
            Some(Duration::from_secs(21_600)),
            None,
            URL,
        );
        assert!(
            timed
                .text
                .starts_with("No answer from the Superintendent after 2 h."),
            "{}",
            timed.text
        );
        assert!(timed.text.contains("within 6 h more"));
        assert_eq!(
            timed.blocks[2]["elements"].as_array().unwrap().len(),
            1,
            "no id → no button"
        );
        let done = answered_progress("the Superintendent", Duration::from_secs(300));
        assert!(all_text(&done).contains(":white_check_mark: *Answered in 5 min*"));
        assert!(!all_text(&done).contains("\"actions\""), "buttons removed");
    }

    #[test]
    fn markdown_becomes_safe_mrkdwn() {
        assert_eq!(mrkdwn_line("## Plan **now**"), "*Plan now*");
        assert_eq!(mrkdwn_line("- **two** tasks"), "• *two* tasks");
        assert_eq!(
            mrkdwn_line("see [the PR](https://example.com/pr/1)"),
            "see the PR (https://example.com/pr/1)"
        );
        assert_eq!(
            mrkdwn_line("<!channel> & `code`"),
            "&lt;!channel&gt; &amp; `code`"
        );
        // Built at runtime so secret scanners do not flag the fixture.
        let fake_key = format!("token {}-{}-{}", "sk", "ant-api03", "SECRET".repeat(4));
        assert!(!mrkdwn_line(&fake_key).contains("SECRETSECRET"));
    }

    #[test]
    fn answers_are_split_capped_and_titled() {
        let answer = answer_message(
            "the Superintendent",
            &report("**Two** tasks left.\n\n\n- tests\n- docs"),
        );
        assert_eq!(
            answer.blocks[0]["text"]["text"],
            ":speech_balloon: Superintendent"
        );
        let first = answer.blocks[1]["text"]["text"].as_str().unwrap();
        assert!(
            first.starts_with("*Two* tasks left.\n\n• tests\n• docs\n\n*Next:* Merge after review"),
            "{first}"
        );
        assert!(first.contains("*Blockers:*\n• CI is red"));
        assert!(answer.text.starts_with("*Answer from the Superintendent*"));
        let long = yard_domain::OrchestratorStatusReport {
            last: "word ".repeat(800),
            next: "next ".repeat(800),
            blockers: vec!["blocker ".repeat(250); 4],
            ..report("x")
        };
        let long = answer_message("x", &long);
        let sections = long
            .blocks
            .as_array()
            .unwrap()
            .iter()
            .filter(|block| block["type"] == "section")
            .map(|block| block["text"]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(sections.len() >= 3, "{}", sections.len());
        assert!(
            sections
                .iter()
                .all(|part| part.chars().count() <= SECTION_CHARS)
        );
        let total = sections
            .iter()
            .map(|part| part.chars().count())
            .sum::<usize>();
        assert!(total <= MAX_RELAY_CHARS);
        assert!(sections.last().unwrap().ends_with("(truncated)"));
    }
}

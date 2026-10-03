//! Free-form owner questions for the Superintendent or a project
//! orchestrator, and relaying the answer back to the Slack thread.
//!
//! Mechanism: the question goes through the direct orchestrator prompt
//! command (`prompt_yard_orchestrator` / `prompt_orchestrator`, the same
//! guarded path as Yard's prompt box), which appends Yard's orchestrator
//! status protocol for that command id. The orchestrator answers with one
//! JSON status-report line (`state`, `last`, `next`, `blockers`) carrying
//! the command id. Yard re-reads the pane (bounded, binding re-validated)
//! every [`RelayTiming::poll`] and relays the first valid report for THAT
//! command id — never other terminal output. After [`RelayTiming::wait`]
//! without one it says so ("open Yard to follow") and keeps watching,
//! slower, until [`RelayTiming::follow`]; a report that arrives in that
//! window is still relayed.

use std::time::Duration;

use yard_domain::{
    MAX_STATUS_REPORT_LINE_BYTES, OrchestratorStatusReport, OrchestratorStatusState,
};

use super::{actions::SLACK_PROVENANCE, message::clean_prompt_line, prompt::cap};

/// Relayed answers are capped (Slack section text allows 3,000).
pub const MAX_ANSWER_CHARS: usize = 2_900;
/// Questions longer than this are refused (Slack allows 4,000 per DM).
pub const MAX_QUESTION_CHARS: usize = 3_000;
/// Pane lines scanned for the report.
pub const REPORT_LINES: u32 = 400;
/// At most this many answers are watched at once; more are sent but not
/// watched ("open Yard to follow").
pub const MAX_WATCHERS: usize = 4;

/// How the answer is watched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayTiming {
    /// Pane re-read interval until `wait`.
    pub poll: Duration,
    /// "Not answered yet" note after this long.
    pub wait: Duration,
    /// Re-read interval between `wait` and `follow`.
    pub follow_poll: Duration,
    /// Stop watching after this long.
    pub follow: Duration,
}

impl Default for RelayTiming {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(5),
            wait: Duration::from_secs(5 * 60),
            follow_poll: Duration::from_secs(30),
            follow: Duration::from_secs(30 * 60),
        }
    }
}

/// Appended to the owner's question so the answer lands in the report.
pub const ANSWER_INSTRUCTION: &str = "(The Yard owner asked this from Slack. Answer the question itself in the status report's `last` field, in plain text, at most 2,000 characters; Yard relays only that report back to Slack. Do not include secrets or file contents.)";

/// The prompt text: provenance marker, the question, the instruction.
#[must_use]
pub fn question_text(question: &str) -> String {
    format!(
        "{SLACK_PROVENANCE} {}\n\n{ANSWER_INSTRUCTION}",
        question.trim()
    )
}

/// Glyphs agent UIs put before (or around) an output line.
const LINE_MARKS: &[char] = &['⏺', '•', '●', '⎿', '│', '┃', '▌'];

/// Rows a wrapped report may span (Claude and Codex word-wrap their own
/// output, so a 2,000-character answer spans many terminal rows).
const MAX_REPORT_ROWS: usize = 60;

/// One pane row without the UI's indent and bullet or box glyphs.
fn strip_row(row: &str) -> &str {
    row.trim()
        .trim_start_matches(LINE_MARKS)
        .trim_end_matches(LINE_MARKS)
        .trim()
}

/// A valid report for `command_id` in `line`, or None.
fn parse_report(line: &str, command_id: &str) -> Option<OrchestratorStatusReport> {
    if !line.contains(command_id) {
        return None;
    }
    OrchestratorStatusReport::parse_line(line)
        .ok()
        .filter(|report| report.command_id == command_id)
}

/// The report starting at `rows[0]`, rejoining rows the agent UI wrapped.
/// Rows are first joined with a space (word wrap); if that breaks the
/// JSON (a long token such as the id was cut), rows as wide as the widest
/// row are joined with nothing, then all rows. The id and the strict
/// parser decide; a wrong guess only changes spacing inside the text.
fn joined_report(rows: &[&str], command_id: &str) -> Option<OrchestratorStatusReport> {
    let first = strip_row(rows[0]);
    if let Some(report) = parse_report(first, command_id) {
        return Some(report);
    }
    let mut parts = vec![first];
    let mut widths = vec![rows[0].trim_end().chars().count()];
    let mut bytes = first.len();
    for row in rows.iter().skip(1).take(MAX_REPORT_ROWS - 1) {
        let part = strip_row(row);
        if part.is_empty() {
            return None;
        }
        bytes += part.len() + 1;
        if bytes > MAX_STATUS_REPORT_LINE_BYTES {
            return None;
        }
        parts.push(part);
        widths.push(row.trim_end().chars().count());
        let widest = widths.iter().copied().max().unwrap_or(0);
        let mut guessed = String::with_capacity(bytes);
        for (index, part) in parts.iter().enumerate() {
            if index > 0 && widths[index - 1] < widest {
                guessed.push(' ');
            }
            guessed.push_str(part);
        }
        let candidates = [parts.join(" "), guessed, parts.concat()];
        if let Some(report) = candidates
            .iter()
            .find_map(|line| parse_report(line, command_id))
        {
            return Some(report);
        }
    }
    None
}

/// The last valid status report for `command_id` in `output`. Rows may
/// carry an agent UI's bullet or box glyph, and a report the UI wrapped
/// over several rows is rejoined; the result must still be one complete,
/// valid report for exactly this command id.
#[must_use]
pub fn scan_report(output: &str, command_id: &str) -> Option<OrchestratorStatusReport> {
    let rows = output.lines().collect::<Vec<_>>();
    (0..rows.len()).rev().find_map(|start| {
        if !strip_row(rows[start]).starts_with('{') {
            return None;
        }
        joined_report(&rows[start..], command_id)
    })
}

/// Multi-line text for Slack: each line redacted and escaped.
fn lines(text: &str) -> String {
    text.lines()
        .map(clean_prompt_line)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The relayed answer: redacted, escaped, capped with "(truncated)".
#[must_use]
pub fn report_text(to: &str, report: &OrchestratorStatusReport) -> String {
    let state = match report.state {
        OrchestratorStatusState::Working => "working",
        OrchestratorStatusState::NeedsAttention => "needs attention",
        OrchestratorStatusState::Idle => "idle",
    };
    let mut text = format!("*Answer from {to}* ({state})\n{}", lines(&report.last));
    let next = lines(&report.next);
    if !next.is_empty() {
        text.push_str("\n*Next:* ");
        text.push_str(&next);
    }
    let blockers = report
        .blockers
        .iter()
        .map(|blocker| format!("• {}", lines(blocker)))
        .collect::<Vec<_>>();
    if !blockers.is_empty() {
        text.push_str("\n*Blockers:*\n");
        text.push_str(&blockers.join("\n"));
    }
    cap(&text, MAX_ANSWER_CHARS)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{ANSWER_INSTRUCTION, MAX_ANSWER_CHARS, question_text, report_text, scan_report};
    use crate::slack::actions::SLACK_PROVENANCE;

    /// One status-report line for `command_id` with `last`.
    pub(crate) fn report_line(command_id: &str, last: &str) -> String {
        serde_json::json!({
            "version": 1, "command_id": command_id, "state": "working",
            "last": last, "next": "Merge after review", "blockers": ["CI is red"]
        })
        .to_string()
    }

    #[test]
    fn only_a_valid_report_for_this_command_is_relayed() {
        let ours = report_line("command-7", "Two tasks left: tests and docs.");
        let other = report_line("command-6", "An older answer");
        let screen = format!(
            "> From Slack (owner): what's left?\n{other}\n⏺ {ours}\n  some later output\n╭────╮\n│ >  │\n╰────╯"
        );
        let report = scan_report(&screen, "command-7").expect("our report");
        assert_eq!(report.last, "Two tasks left: tests and docs.");
        assert!(scan_report(&screen, "command-8").is_none(), "no report yet");
        // The id must be the report's own command id, not a substring of
        // another id or of its text.
        for other in [
            report_line("command-70", "a longer id"),
            report_line("command-6", "answering command-7 too"),
        ] {
            assert!(scan_report(&other, "command-7").is_none(), "{other}");
        }
        assert_eq!(
            scan_report(&format!("{other}\n"), "command-6")
                .unwrap()
                .last,
            "An older answer"
        );
        // Not a complete, valid report: prose that mentions the id, a
        // wrapped line, an extra field, a wrong version.
        for broken in [
            "Status protocol for command \"command-7\": respond with JSON".to_owned(),
            ours[..40].to_owned(),
            ours.replace("\"version\":1", "\"version\":2"),
            ours.replace("\"blockers\"", "\"extra\":1,\"blockers\""),
        ] {
            assert!(scan_report(&broken, "command-7").is_none(), "{broken}");
        }
    }

    /// `line` word-wrapped the way Claude (Ink) or Codex (ratatui) wrap
    /// assistant text: at spaces, long words cut, continuation indented.
    fn tui_wrap(line: &str, width: usize) -> String {
        let mut rows = Vec::new();
        let mut row = String::from("⏺ ");
        for word in line.split(' ') {
            let mut word = word.to_owned();
            if row.chars().count() + word.chars().count() + 1 > width && row.trim().len() > 1 {
                rows.push(std::mem::replace(&mut row, String::from("  ")));
            }
            while row.chars().count() + word.chars().count() > width {
                let room = width - row.chars().count();
                let rest = word.split_off(room);
                row.push_str(&word);
                rows.push(std::mem::replace(&mut row, String::from("  ")));
                word = rest;
            }
            if !row.trim().is_empty() && row.trim() != "⏺" {
                row.push(' ');
            }
            row.push_str(&word);
        }
        rows.push(row);
        rows.join("\n")
    }

    #[test]
    fn a_report_the_agent_ui_wrapped_is_rejoined() {
        let id = "6f1c2d3e-4b5a-4c7d-8e9f-0a1b2c3d4e5f";
        let answer = "Two tasks are left: the relay tests and the README section. \
            Both should land today; nothing is blocked on review right now.";
        let ours = report_line(id, answer);
        assert!(ours.len() > 200);
        for width in [120, 100, 80, 60, 40] {
            let wrapped = tui_wrap(&ours, width);
            assert!(wrapped.lines().count() >= 2, "{wrapped}");
            let screen =
                format!("> From Slack (owner): what's left?\n{wrapped}\n\n╭────╮\n│ >  │\n╰────╯");
            let report = scan_report(&screen, id).unwrap_or_else(|| panic!("{screen}"));
            assert_eq!(report.command_id, id);
            let squeeze = |text: &str| text.split_whitespace().collect::<String>();
            assert_eq!(squeeze(&report.last), squeeze(answer));
            if width == 100 {
                assert_eq!(report.last, answer, "word wrap rejoined exactly");
            }
            assert!(scan_report(&screen, "6f1c2d3e").is_none());
        }
        // Another command's wrapped report is still not ours, and a wrapped
        // report cut off by a blank row is not complete.
        let other = tui_wrap(
            &report_line("7a1c2d3e-4b5a-4c7d-8e9f-0a1b2c3d4e5f", answer),
            60,
        );
        assert!(scan_report(&other, id).is_none());
        let wrapped = tui_wrap(&ours, 60);
        let mut rows = wrapped.lines().collect::<Vec<_>>();
        rows.insert(2, "");
        assert!(scan_report(&rows.join("\n"), id).is_none());
    }

    #[test]
    fn relayed_answers_are_redacted_escaped_and_capped() {
        let line = report_line(
            "c",
            "Deploy with AWS_SECRET_ACCESS_KEY=abc123 from /Users/sample/secret.txt <!channel>\nsecond line",
        );
        let report = scan_report(&line, "c").unwrap();
        let text = report_text("the Superintendent", &report);
        assert!(
            text.starts_with("*Answer from the Superintendent* (working)\n"),
            "{text}"
        );
        assert!(
            !text.contains("abc123") && !text.contains("/Users/sample"),
            "{text}"
        );
        assert!(
            text.contains("&lt;!channel&gt;") && !text.contains("<!channel>"),
            "{text}"
        );
        assert!(
            text.contains("\nsecond line\n*Next:* Merge after review"),
            "{text}"
        );
        assert!(text.contains("*Blockers:*\n• CI is red"), "{text}");
        let long = scan_report(&report_line("c", &"word ".repeat(700)), "c").unwrap();
        let text = report_text("x", &long);
        assert!(text.chars().count() <= MAX_ANSWER_CHARS);
        assert!(text.ends_with("(truncated)"));

        let prompt = question_text("  what's left?  ");
        assert!(prompt.starts_with(&format!("{SLACK_PROVENANCE} what's left?\n\n")));
        assert!(prompt.ends_with(ANSWER_INSTRUCTION));
    }
}

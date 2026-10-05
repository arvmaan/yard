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

use super::{
    actions::SLACK_PROVENANCE,
    message::{clean_prompt_line, redact_command},
    prompt::cap,
};

/// Relayed answers are capped (Slack section text allows 3,000).
pub const MAX_ANSWER_CHARS: usize = 2_900;
/// Questions longer than this are refused (Slack allows 4,000 per DM).
pub const MAX_QUESTION_CHARS: usize = 3_000;
/// Pane lines scanned for the report.
pub const REPORT_LINES: u32 = 400;
/// At most this many answers are watched at once (each for up to the 2 h
/// window); more are sent but not watched (their card offers Check again).
pub const MAX_WATCHERS: usize = 16;
/// Answers still watched (slowly) after their window ended.
pub const MAX_LATE_WATCHERS: usize = 8;
/// Relayed command ids remembered so an answer is posted once.
pub const MAX_RELAYED: usize = 64;

/// How the answer is watched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayTiming {
    /// Pane re-read interval until `wait`.
    pub poll: Duration,
    /// After this long the pane is re-read every `follow_poll`.
    pub wait: Duration,
    /// Re-read interval between `wait` and `follow`.
    pub follow_poll: Duration,
    /// The watch window ([`ASK_TIMEOUT_VAR`]); then "Check again".
    pub follow: Duration,
    /// The progress message is updated at most this often.
    pub progress: Duration,
    /// After the window: re-read interval for a late answer.
    pub late_poll: Duration,
    /// After the window: a late answer is still relayed this long.
    pub late: Duration,
}

/// Minutes to watch for an answer (default [`DEFAULT_ASK_TIMEOUT_MINS`]).
pub const ASK_TIMEOUT_VAR: &str = "YARD_SLACK_ASK_TIMEOUT_MINS";
pub const DEFAULT_ASK_TIMEOUT_MINS: u64 = 120;
/// Accepted range of [`ASK_TIMEOUT_VAR`].
pub const ASK_TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 5..=1_440;

impl Default for RelayTiming {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(5),
            wait: Duration::from_secs(5 * 60),
            follow_poll: Duration::from_secs(20),
            follow: Duration::from_secs(DEFAULT_ASK_TIMEOUT_MINS * 60),
            progress: Duration::from_secs(90),
            late_poll: Duration::from_secs(120),
            late: Duration::from_secs(6 * 60 * 60),
        }
    }
}

impl RelayTiming {
    /// The defaults with the window from [`ASK_TIMEOUT_VAR`], plus a
    /// warning when the value is not a whole number of minutes in
    /// [`ASK_TIMEOUT_RANGE`] (the default is used then).
    pub fn from_env(raw: Option<&str>) -> (Self, Option<String>) {
        let mut timing = Self::default();
        let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return (timing, None);
        };
        match raw.parse::<u64>() {
            Ok(minutes) if ASK_TIMEOUT_RANGE.contains(&minutes) => {
                timing.follow = Duration::from_secs(minutes * 60);
                (timing, None)
            }
            _ => (
                timing,
                Some(format!(
                    "{ASK_TIMEOUT_VAR} must be whole minutes from {} to {}; using {DEFAULT_ASK_TIMEOUT_MINS}",
                    ASK_TIMEOUT_RANGE.start(),
                    ASK_TIMEOUT_RANGE.end()
                )),
            ),
        }
    }
}

/// Appended to the owner's question so the answer lands in the report,
/// quickly: a Slack question is not a task.
pub const ANSWER_INSTRUCTION: &str = "(The Yard owner asked this from Slack and is waiting for the reply. Answer directly and quickly: answer from what you already know or can read quickly; do not start long investigations or delegations, and do not wait on other agents. If you need to delegate or it needs more work, say so in the answer instead. Put the answer itself in the status report's `last` field, in plain text, at most 2,000 characters; Yard relays only that report back to Slack. Do not include secrets or file contents.)";

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

/// Lines the Codex TUI prints when its app-server session is gone (seen
/// live after the daemon auto-updated): the agent will never answer.
pub const SESSION_LOST_LINES: &[&str] = &[
    "Connection lost",
    "app-server session could not be restored",
    "Disconnected from this task",
];

/// The Codex TUI's own error-row marker; [`SESSION_LOST_LINES`] count only
/// on such a row, so tool or command output that merely mentions a lost
/// connection never ends the watch.
const ERROR_MARK: char = '■';

/// Whole Codex sentences that count on an unmarked (continuation) row.
const SESSION_LOST_SENTENCES: &[&str] = &[
    "Connection lost. Reconnecting",
    "The app-server session could not be restored",
    "Disconnected from this task. Start a new session",
];

/// Bottom rows checked for [`SESSION_LOST_LINES`] when the question's
/// command id is no longer (or not yet) visible in the pane.
const RECENT_ROWS: usize = 15;

/// Whether `row` is a Codex TUI session-loss line (not output that
/// happens to contain the words).
fn is_session_lost_row(row: &str) -> bool {
    let row = strip_row(row);
    if let Some(rest) = row.strip_prefix(ERROR_MARK) {
        let lower = rest.to_lowercase();
        return SESSION_LOST_LINES
            .iter()
            .any(|line| lower.contains(&line.to_lowercase()));
    }
    SESSION_LOST_SENTENCES
        .iter()
        .any(|sentence| row.starts_with(sentence))
}

/// The first [`SESSION_LOST_LINES`] row shown after the question (the
/// first row carrying `command_id`, i.e. the delivered prompt's status
/// protocol), or in the bottom rows when the id is not visible. Older
/// scrollback above the question never counts. Returned redacted and
/// capped at 200 characters but not escaped: the card that shows it
/// escapes it once.
#[must_use]
pub fn session_lost(output: &str, command_id: &str) -> Option<String> {
    let rows = output.lines().collect::<Vec<_>>();
    let from = rows
        .iter()
        .position(|row| row.contains(command_id))
        .map_or_else(|| rows.len().saturating_sub(RECENT_ROWS), |at| at + 1);
    rows[from..]
        .iter()
        .find_map(|row| is_session_lost_row(row).then(|| lost_line(row)))
}

/// A loss row for an error card: one line, redacted, capped, unescaped.
fn lost_line(row: &str) -> String {
    let collapsed = strip_row(row)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cap(&redact_command(&collapsed), 200)
}

/// Row starts that show the agent working (or a new session) after a
/// loss line: Claude/Codex output bullets, the thinking spinner, a new
/// session's banner box.
const ACTIVITY_MARKS: &[char] = &['⏺', '•', '●', '✻', '╭'];

/// Before a question is sent: the last session-loss line in the bottom
/// rows when nothing the agent did follows it (its TUI is still
/// disconnected, so a question would never be answered). Returned like
/// [`session_lost`].
#[must_use]
pub fn lost_before_asking(output: &str) -> Option<String> {
    let rows = output.lines().collect::<Vec<_>>();
    let from = rows.len().saturating_sub(RECENT_ROWS);
    let at = from
        + rows[from..]
            .iter()
            .rposition(|row| is_session_lost_row(row))?;
    let active = rows[at + 1..]
        .iter()
        .any(|row| row.trim_start().starts_with(ACTIVITY_MARKS));
    (!active).then(|| lost_line(rows[at]))
}

/// One look at the pane while waiting for an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watched {
    /// The status report for this command id, when shown.
    pub report: Option<OrchestratorStatusReport>,
    /// The agent UI lost its session (see [`session_lost`]).
    pub session_lost: Option<String>,
}

/// [`scan_report`] and [`session_lost`] over one pane read.
#[must_use]
pub fn watched(output: &str, command_id: &str) -> Watched {
    Watched {
        report: scan_report(output, command_id),
        session_lost: session_lost(output, command_id),
    }
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
    use std::time::Duration;

    use super::{
        ANSWER_INSTRUCTION, MAX_ANSWER_CHARS, RelayTiming, lost_before_asking, question_text,
        report_text, scan_report, session_lost, watched,
    };
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

    /// The lines the Codex TUI printed live when its app-server daemon
    /// auto-updated (2026-10-01).
    const CODEX_LOST: [&str; 3] = [
        "■ Connection lost. Reconnecting…",
        "■ The app-server session could not be restored.",
        "  Disconnected from this task. Start a new session to continue.",
    ];

    #[test]
    fn a_codex_session_loss_after_the_question_is_detected() {
        let question = "› Status protocol for command \"command-9\": respond with one JSON line";
        for lost in CODEX_LOST {
            let screen = format!("{question}\n• Working (3m 10s)\n{lost}\n› ");
            let found = session_lost(&screen, "command-9").unwrap_or_else(|| panic!("{lost}"));
            assert!(
                lost.contains(found.trim_start_matches(['■', ' '])),
                "{found}"
            );
        }
        // Older scrollback above the question never counts.
        let screen = format!("{}\n{question}\n• Working (3m 10s)\n› ", CODEX_LOST[0]);
        assert_eq!(session_lost(&screen, "command-9"), None);
        // Question scrolled away: only the bottom rows count.
        let recent = format!("{}\n{}", "output\n".repeat(30), CODEX_LOST[2]);
        assert!(session_lost(&recent, "command-9").is_some());
        let old = format!("{}\n{}", CODEX_LOST[2], "output\n".repeat(30));
        assert_eq!(session_lost(&old, "command-9"), None);
        // The agent's own tool or command output mentioning a lost
        // connection is not the Codex TUI losing its session.
        for output in [
            "  └ ssh: connect to host build-1: Connection lost",
            "    curl: (56) Recv failure: connection lost",
            "WARN herdr: Connection lost to wH:p1, retrying",
            "│ 2026-10-02T15:40Z disconnected from this task queue",
        ] {
            let screen = format!("{question}\n• Ran tail -f server.log\n{output}\n› ");
            assert_eq!(session_lost(&screen, "command-9"), None, "{output}");
        }
        // An answer and a loss in one read: both are reported.
        let both = format!(
            "{question}\n{}\n{}",
            report_line("command-9", "ok"),
            CODEX_LOST[1]
        );
        let seen = watched(&both, "command-9");
        assert!(seen.report.is_some() && seen.session_lost.is_some());
        // Redacted, but not escaped: the error card escapes it once.
        let screen = format!("{question}\n■ Connection lost <!here> & token=abc123");
        let line = session_lost(&screen, "command-9").unwrap();
        assert!(
            line.contains("<!here> &") && !line.contains("abc123"),
            "{line}"
        );
    }

    #[test]
    fn a_tui_still_disconnected_before_the_question_is_detected() {
        // The live 2026-10-02 15:34 screen: the loss lines, then only the
        // idle composer and footer.
        let lost = format!(
            "• Earlier answer\n{}\n{}\n{}\n\n› Ask Codex to do anything\n  ? for shortcuts",
            CODEX_LOST[0], CODEX_LOST[1], CODEX_LOST[2]
        );
        let line = lost_before_asking(&lost).expect("disconnected");
        assert!(line.contains("Disconnected from this task"), "{line}");
        // Recovered or restarted: agent output after the loss lines.
        for after in [
            "• Reconnected; working on it",
            "╭──────────╮",
            "✻ Thinking…",
        ] {
            let screen = format!("{lost}\n{after}\n› ");
            assert_eq!(lost_before_asking(&screen), None, "{after}");
        }
        // Long scrolled away, or tool output that only mentions it.
        let old = format!("{}\n{}", CODEX_LOST[2], "output\n".repeat(30));
        assert_eq!(lost_before_asking(&old), None);
        let tool = "  └ ssh: connect to host build-1: Connection lost\n› ";
        assert_eq!(lost_before_asking(tool), None);
        assert_eq!(
            lost_before_asking(crate::slack::prompt::tests::WORKING),
            None
        );
    }

    #[test]
    fn the_watch_window_is_configurable() {
        let (timing, warning) = RelayTiming::from_env(None);
        assert_eq!(timing.follow, Duration::from_secs(2 * 60 * 60));
        assert!(warning.is_none());
        assert!(
            timing.progress >= Duration::from_secs(60)
                && timing.progress <= Duration::from_secs(120)
        );
        let (timing, warning) = RelayTiming::from_env(Some(" 45 "));
        assert_eq!(timing.follow, Duration::from_secs(45 * 60));
        assert!(warning.is_none());
        for bad in ["0", "4", "1441", "two", "-5", "1.5"] {
            let (timing, warning) = RelayTiming::from_env(Some(bad));
            assert_eq!(timing, RelayTiming::default(), "{bad}");
            assert!(
                warning.unwrap().contains("YARD_SLACK_ASK_TIMEOUT_MINS"),
                "{bad}"
            );
        }
    }

    #[test]
    fn slack_questions_ask_for_a_quick_direct_answer() {
        for phrase in [
            "answer from what you already know or can read quickly",
            "do not start long investigations or delegations",
            "If you need to delegate or it needs more work, say so in the answer",
            "status report's `last` field",
        ] {
            assert!(ANSWER_INSTRUCTION.contains(phrase), "{phrase}");
        }
    }
}

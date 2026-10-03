//! Message content: titles and states only.
//!
//! A message names the project, the agent (profile, orchestrator,
//! superintendent or workstream), a short objective title, the state and the
//! time. It never carries terminal output, transcripts, prompts, error
//! messages, file contents or loopback URLs. Every interpolated string is
//! cleaned (control characters removed, whitespace collapsed), passed through
//! a defensive credential redactor, truncated, and escaped for Slack mrkdwn so
//! a project name cannot mention `@channel` or inject a link.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::detector::{AttentionEvent, AttentionKind, Subject};
use super::outbox::Batch;

const TITLE_CHARS: usize = 80;
const NAME_CHARS: usize = 60;
const MAX_DIGEST_LINES: usize = 5;
const REDACTED: &str = "[redacted]";
const REDACTED_PATH: &str = "[path]";

#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingMessage {
    /// Top-level fallback; mobile push shows only this.
    pub text: String,
    pub blocks: Value,
}

#[must_use]
pub fn batch_message(batch: &Batch) -> OutgoingMessage {
    let lines = batch
        .events
        .iter()
        .take(MAX_DIGEST_LINES)
        .map(|event| event_line(event, batch.events.len() > 1 || batch.dropped > 0))
        .collect::<Vec<_>>();
    let hidden = batch.events.len().saturating_sub(MAX_DIGEST_LINES);
    let mut body = lines.join("\n");
    if hidden > 0 {
        let _ = write!(
            body,
            "\n{hidden} more {} — check Yard.",
            plural(hidden, "update", "updates")
        );
    }
    if batch.dropped > 0 {
        if !body.is_empty() {
            body.push('\n');
        }
        let _ = write!(
            body,
            "{} older {} dropped while Slack was unreachable or busy — check Yard.",
            batch.dropped,
            plural(batch.dropped, "update was", "updates were")
        );
    }
    let text = match batch.events.as_slice() {
        [event] if batch.dropped == 0 => event_line(event, true),
        events => format!(
            "{} Yard {}",
            events.len() + batch.dropped,
            plural(events.len() + batch.dropped, "update", "updates")
        ),
    };
    let observed = batch
        .events
        .iter()
        .map(|event| event.observed_at_unix_ms)
        .max()
        .unwrap_or_default();
    OutgoingMessage {
        text,
        blocks: blocks(&body, observed),
    }
}

#[must_use]
pub fn test_message(now_unix_ms: u64) -> OutgoingMessage {
    let body = "Yard test message: Slack notifications reach this DM.";
    OutgoingMessage {
        text: body.to_owned(),
        blocks: blocks(body, now_unix_ms),
    }
}

fn blocks(body: &str, unix_ms: u64) -> Value {
    json!([
        { "type": "section", "text": { "type": "mrkdwn", "text": body } },
        { "type": "context", "elements": [ { "type": "mrkdwn", "text": time_token(unix_ms) } ] }
    ])
}

/// Slack renders `<!date^…>` in the reader's own time zone.
fn time_token(unix_ms: u64) -> String {
    let seconds = i64::try_from(unix_ms / 1000).unwrap_or_default();
    let fallback = DateTime::<Utc>::from_timestamp(seconds, 0).map_or_else(
        || "unknown time".to_owned(),
        |time| time.format("%H:%M UTC").to_string(),
    );
    format!("Yard · <!date^{seconds}^{{date_short_pretty}} {{time}}|{fallback}>")
}

fn event_line(event: &AttentionEvent, with_project: bool) -> String {
    let glyph = match event.kind {
        AttentionKind::Blocked => ":raised_hand:",
        AttentionKind::ReadyForReview => ":eyes:",
        AttentionKind::CommandFailed => ":x:",
        AttentionKind::CommandAmbiguous => ":grey_question:",
    };
    let subject = subject_label(&event.subject);
    let state = match (&event.kind, &event.subject) {
        (AttentionKind::Blocked, _) => format!(
            "{subject} is blocked: Herdr reports it is waiting on a prompt in its terminal. Answer it in Yard."
        ),
        (AttentionKind::ReadyForReview, _) => format!(
            "{subject}: Herdr reports the turn finished — ready for your review. No receipt has been recorded."
        ),
        (AttentionKind::CommandFailed, _) => format!("{subject} failed. Check Yard."),
        (AttentionKind::CommandAmbiguous, _) => format!(
            "{subject}: Yard can't tell whether this landed; it was not retried. Check Yard."
        ),
    };
    match event.project_name.as_deref() {
        Some(project) if with_project => {
            format!("{glyph} *{}* · {state}", clean(project, NAME_CHARS))
        }
        _ => format!("{glyph} {state}"),
    }
}

pub(crate) fn subject_label(subject: &Subject) -> String {
    match subject {
        Subject::Worker {
            profile_name,
            objective,
        } => {
            let name = profile_name
                .as_deref()
                .map_or_else(|| "A worker".to_owned(), |name| clean(name, NAME_CHARS));
            match objective.as_deref() {
                Some(objective) => {
                    format!("{name} ({})", clean(first_line(objective), TITLE_CHARS))
                }
                None => name,
            }
        }
        Subject::ProjectOrchestrator => "The project orchestrator".to_owned(),
        Subject::YardOrchestrator => "The superintendent".to_owned(),
        Subject::Workstream { name } => match name.as_deref() {
            Some(name) => format!("Workstream {}", clean(name, NAME_CHARS)),
            None => "A workstream".to_owned(),
        },
        Subject::Command {
            command_type,
            objective,
            node_name,
        } => {
            let label = command_label(command_type);
            match (objective.as_deref(), node_name.as_deref()) {
                (Some(objective), _) => {
                    format!("{label} ({})", clean(first_line(objective), TITLE_CHARS))
                }
                (None, Some(node)) => format!("{label} ({})", clean(node, NAME_CHARS)),
                (None, None) => label.to_owned(),
            }
        }
    }
}

/// An objective is the head of the worker's prompt; only its first non-empty
/// line is a title.
fn first_line(objective: &str) -> &str {
    objective
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

fn command_label(command_type: &str) -> &'static str {
    match command_type {
        "assignment_prompt" => "A prompt to a worker",
        "orchestrator_prompt" => "A prompt to the project orchestrator",
        "yard_orchestrator_prompt" => "A prompt to the superintendent",
        "yard_orchestrator_route" => "An order routed to a project",
        "coordination_node_prompt" => "A prompt to a workstream",
        "coordination_node_route" => "A workstream order to a project",
        "profile_allocation" | "worker_allocation" => "A worker allocation",
        "worker_handoff" => "A worker handoff",
        "assignment_disposition" => "An assignment completion or cancellation",
        _ => "A Yard command",
    }
}

const fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// Clean, redact, truncate and escape one user-supplied title.
#[must_use]
pub fn clean(value: &str, max_chars: usize) -> String {
    let collapsed = value
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
    let redacted = redact(&collapsed);
    let truncated = if redacted.chars().count() > max_chars {
        let mut kept = redacted
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        kept.push('…');
        kept
    } else {
        redacted
    };
    escape(&truncated)
}

/// Clean one line of prompt text for display: controls removed, whitespace
/// collapsed, redacted (commands also lose environment values and flag
/// secrets, see [`redact_command`]) and escaped. Not truncated; callers cap
/// the whole block.
#[must_use]
pub fn clean_prompt_line(value: &str) -> String {
    let collapsed = value
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
    escape(&redact_command(&collapsed))
}

/// Redact one line of a command an agent wants to run (or any prompt
/// line): every environment assignment's value (`FOO=bar` →
/// `FOO=[redacted]`), the value of a secret-looking flag (`--token x`,
/// `--password=x`), credentials in URLs, then everything [`redact`] covers
/// (tokens, key ids, paths, loopback URLs, `secret=value` pairs).
#[must_use]
pub fn redact_command(value: &str) -> String {
    /// Tools whose `-p` carries a password (`-phunter2`, `-p hunter2`).
    const PASSWORD_P_TOOLS: &[&str] = &["mysql", "mysqldump", "mysqladmin", "mariadb", "sshpass"];
    let mut words = Vec::new();
    let mut redact_next = false;
    let mut password_p = false;
    for word in value.split(' ') {
        if redact_next && !word.is_empty() {
            words.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }
        let bare = word.trim_start_matches(['"', '\'', '(', '`']);
        let program = bare.rsplit('/').next().unwrap_or(bare);
        if PASSWORD_P_TOOLS.contains(&program) {
            password_p = true;
        }
        if password_p && bare == "-p" {
            words.push(word.to_owned());
            redact_next = true;
            continue;
        }
        if password_p && bare.len() > 2 && bare.starts_with("-p") && !bare.starts_with("-p=") {
            words.push(format!("-p{REDACTED}"));
            continue;
        }
        // `curl -u user:pass`, `--user=user:pass`.
        if bare == "-u" || bare == "--user" {
            words.push(word.to_owned());
            redact_next = true;
            continue;
        }
        if let Some(value) = bare.strip_prefix("--user=")
            && !value.is_empty()
        {
            words.push(format!("--user={REDACTED}"));
            continue;
        }
        if let Some((name, _)) = bare.split_once('=')
            && environment_name(name)
        {
            words.push(format!("{name}={REDACTED}"));
            continue;
        }
        if let Some(flag) = bare.strip_prefix('-') {
            let flag = flag.trim_start_matches('-');
            match flag.split_once('=') {
                Some((name, _)) if secret_key(&name.to_ascii_lowercase()) => {
                    words.push(format!("--{name}={REDACTED}"));
                    continue;
                }
                None if !flag.is_empty() && secret_key(&flag.to_ascii_lowercase()) => {
                    words.push(word.to_owned());
                    redact_next = true;
                    continue;
                }
                _ => {}
            }
        }
        words.push(strip_url_credentials(word));
    }
    redact(&words.join(" "))
}

/// `FOO`, `AWS_SECRET_ACCESS_KEY`, `_X1`: a conventional environment name.
fn environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// `https://user:pass@host/x` → `https://[redacted]@host/x`.
fn strip_url_credentials(word: &str) -> String {
    let Some(scheme_end) = word.find("://") else {
        return word.to_owned();
    };
    let rest = &word[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}://{REDACTED}@{}", &word[..scheme_end], &rest[at + 1..]),
        None => word.to_owned(),
    }
}

/// Slack mrkdwn escaping: `&`, `<` and `>` are the only control characters.
pub(crate) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Replace anything that looks like a credential or a loopback URL.
///
/// Titles should never hold secrets; this is a defensive last line, so it
/// errs toward redacting: Slack and bearer tokens, AWS key ids, JWTs, PEM
/// markers, `key=value` pairs whose key names a secret, long opaque runs,
/// and URLs pointing at this machine.
#[must_use]
pub fn redact(value: &str) -> String {
    let mut words = Vec::new();
    let mut redact_next = false;
    // Inside an `Authorization:` value: every word up to the closing quote
    // (or the end of the line), so `Basic <b64>` loses its credential too.
    let mut in_header = false;
    let mut previous_secret = false;
    for word in value.split(' ') {
        if in_header && !word.is_empty() {
            match word.chars().last() {
                Some(quote @ ('"' | '\'')) => {
                    words.push(format!("{REDACTED}{quote}"));
                    in_header = false;
                }
                _ => words.push(REDACTED.to_owned()),
            }
            continue;
        }
        if redact_next && !word.is_empty() {
            words.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        let unquoted = lower.trim_start_matches(['"', '\'']);
        if unquoted == "authorization:" || unquoted == "proxy-authorization:" {
            in_header = true;
            words.push(word.to_owned());
            continue;
        }
        if lower == "bearer" {
            redact_next = true;
            words.push(word.to_owned());
            continue;
        }
        // `db_password = "hunter2"`: the value follows a lone `=` / `:`.
        if (word == "=" || word == ":") && previous_secret {
            redact_next = true;
            words.push(word.to_owned());
            continue;
        }
        previous_secret = !word.is_empty()
            && !word.contains(['=', ':'])
            && secret_key(unquoted.trim_end_matches(['"', '\'']));
        if sensitive_word(word, &lower) {
            words.push(REDACTED.to_owned());
            continue;
        }
        if path_word(word, &lower) {
            words.push(REDACTED_PATH.to_owned());
            continue;
        }
        match word.split_once(['=', ':']) {
            // `password: hunter2` — the value is the next word.
            Some((key, "")) if secret_key(&key.to_ascii_lowercase()) => {
                redact_next = true;
                words.push(word.to_owned());
            }
            Some((key, _)) if secret_key(&key.to_ascii_lowercase()) => {
                words.push(format!("{key}={REDACTED}"));
            }
            _ => words.push(word.to_owned()),
        }
    }
    words.join(" ")
}

fn sensitive_word(word: &str, lower: &str) -> bool {
    let trimmed = word.trim_matches(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
    });
    let slack_token = [
        "xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-", "xoxe-", "xapp-",
    ]
    .iter()
    .any(|prefix| lower.contains(prefix));
    let aws_key_id = ["akia", "asia", "aida", "aroa"].iter().any(|prefix| {
        trimmed.len() == 20
            && trimmed.to_ascii_lowercase().starts_with(prefix)
            && trimmed
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    });
    let jwt = trimmed.starts_with("eyJ") && word.matches('.').count() >= 2;
    let pem = lower.contains("-----begin") || lower.contains("private key");
    // Any scheme (`http`, `https`, `ws`, `wss`, …) anywhere in the word, so
    // `(http://…)` and `See:http://…` are caught, plus scheme-less
    // `localhost:4317/…` and `127.0.0.1:4317/…`.
    let loopback_host = |rest: &str| {
        ["127.", "localhost", "[::1]", "::1", "0.0.0.0"]
            .iter()
            .any(|host| rest.starts_with(host))
    };
    let bare = lower.trim_start_matches(|character: char| !character.is_ascii_alphanumeric());
    let loopback_url = lower
        .match_indices("://")
        .any(|(index, _)| loopback_host(&lower[index + 3..]))
        || ["localhost:", "localhost/", "127.0.0.1:", "127.0.0.1/"]
            .iter()
            .any(|prefix| bare.starts_with(prefix));
    let opaque = trimmed.len() >= 32
        && trimmed.bytes().any(|byte| byte.is_ascii_digit())
        && trimmed.bytes().any(|byte| byte.is_ascii_alphabetic())
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+/=_-".contains(&byte));
    slack_token || aws_key_id || jwt || pem || loopback_url || opaque
}

/// A local file path: `/a/b`, `~/a`, `./a/b`, `../a`, or anything naming a
/// home directory. Paths are local detail and never belong in a DM.
fn path_word(word: &str, lower: &str) -> bool {
    if lower.contains("/users/") || lower.contains("/home/") || lower.contains("~/") {
        return true;
    }
    let stripped = word.trim_start_matches(|character: char| "([{\"'`<".contains(character));
    let rooted = ["/", "./", "../"]
        .iter()
        .any(|prefix| stripped.starts_with(prefix));
    rooted
        && stripped
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
            .count()
            >= 2
}

fn secret_key(key: &str) -> bool {
    [
        "token",
        "secret",
        "password",
        "passwd",
        "cookie",
        "session",
        "apikey",
        "api_key",
        "credential",
        "auth",
        "api-key",
        "_key",
        "-key",
        "private",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use yard_domain::ObservedStatus;

    use super::{batch_message, clean, clean_prompt_line, redact, redact_command, test_message};
    use crate::slack::detector::{AttentionEvent, AttentionKind, EventSource, Subject};
    use crate::slack::outbox::{Batch, ThreadKey};

    fn event(kind: AttentionKind, project: &str, objective: &str) -> AttentionEvent {
        AttentionEvent {
            kind,
            source: EventSource::Runtime {
                worker_id: "w-1".to_owned(),
                status: ObservedStatus::Blocked,
            },
            project_id: Some("p-1".to_owned()),
            project_name: Some(project.to_owned()),
            subject: Subject::Worker {
                profile_name: Some("Implementer".to_owned()),
                objective: Some(objective.to_owned()),
            },
            view_target: None,
            observed_at_unix_ms: 1_695_900_000_000,
            ready_at: Instant::now(),
        }
    }

    #[test]
    fn single_event_names_project_agent_objective_and_state() {
        let message = batch_message(&Batch {
            events: vec![event(AttentionKind::Blocked, "Checkout", "Ship the banner")],
            dropped: 0,
            thread: ThreadKey::Project("p-1".to_owned()),
        });
        assert_eq!(
            message.text,
            ":raised_hand: *Checkout* · Implementer (Ship the banner) is blocked: Herdr reports it is waiting on a prompt in its terminal. Answer it in Yard."
        );
        let blocks = message.blocks.to_string();
        assert!(blocks.contains("<!date^1695900000^"), "{blocks}");
        assert!(blocks.contains("|11:20 UTC>"), "{blocks}");
    }

    #[test]
    fn objective_titles_keep_only_the_first_prompt_line() {
        let message = batch_message(&Batch {
            events: vec![event(
                AttentionKind::Blocked,
                "Checkout",
                "\n  Ship the banner\nContext: edit src/banner.rs and read the design doc",
            )],
            dropped: 0,
            thread: ThreadKey::Project("p-1".to_owned()),
        });
        assert!(
            message
                .text
                .contains("Implementer (Ship the banner) is blocked")
        );
        assert!(!message.text.contains("Context"), "{}", message.text);
    }

    #[test]
    fn ready_for_review_never_claims_completion() {
        let message = batch_message(&Batch {
            events: vec![event(AttentionKind::ReadyForReview, "Checkout", "x")],
            dropped: 0,
            thread: ThreadKey::Project("p-1".to_owned()),
        });
        assert!(message.text.contains("ready for your review"));
        assert!(message.text.contains("No receipt has been recorded"));
        assert!(!message.text.to_lowercase().contains("completed"));
    }

    #[test]
    fn digests_cap_lines_and_report_drops() {
        let events = (0..7)
            .map(|index| event(AttentionKind::Blocked, &format!("P{index}"), "x"))
            .collect::<Vec<_>>();
        let message = batch_message(&Batch {
            events,
            dropped: 3,
            thread: ThreadKey::Yard,
        });
        assert_eq!(message.text, "10 Yard updates");
        let body = message.blocks[0]["text"]["text"].as_str().unwrap();
        assert_eq!(body.matches(":raised_hand:").count(), 5, "{body}");
        assert!(body.contains("2 more updates — check Yard."), "{body}");
        assert!(body.contains("3 older updates were dropped"), "{body}");
    }

    #[test]
    fn titles_are_escaped_truncated_and_stripped_of_controls() {
        assert_eq!(
            clean("<!channel> & <@U1>", 80),
            "&lt;!channel&gt; &amp; &lt;@U1&gt;"
        );
        assert_eq!(clean("a\u{1b}[31mb\n\tc", 80), "a [31mb c");
        let long = "x".repeat(100);
        let cleaned = clean(&long, 80);
        assert_eq!(cleaned.chars().count(), 80);
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn redactor_removes_credentials_and_loopback_urls() {
        for (input, leaked) in [
            (
                concat!("use xo", "xb-1234-5678-abcdef now"),
                concat!("xo", "xb-1234"),
            ),
            (
                concat!("key AK", "IAIOSFODNN7EXAMPLE here"),
                concat!("AK", "IAIOSFODNN7EXAMPLE"),
            ),
            ("Bearer abc.def.ghi", "abc.def.ghi"),
            ("jwt eyJhbGciOi.eyJzdWIiOi.c2lnbmF0dXJl", "eyJzdWIiOi"),
            ("password=hunter2", "hunter2"),
            ("session:abc123", "abc123"),
            ("open http://127.0.0.1:4317/projects/1", "127.0.0.1"),
            ("see http://localhost:4317/", "localhost"),
            ("-----BEGIN RSA PRIVATE KEY-----", "BEGIN"),
            ("id 0123456789abcdef0123456789abcdef01", "0123456789abcdef"),
            ("Fix (http://127.0.0.1:4317/projects/p1)", "127.0.0.1"),
            ("See:http://localhost:4317/x", "localhost"),
            ("open ws://127.0.0.1:4317/term", "127.0.0.1"),
            ("open wss://[::1]:4317/term", "[::1]"),
            ("visit localhost:4317/projects", "localhost"),
            ("password: hunter2", "hunter2"),
            ("secret: hunter2", "hunter2"),
            ("token: hunter2", "hunter2"),
            ("api_key= hunter2", "hunter2"),
        ] {
            let redacted = redact(input);
            assert!(!redacted.contains(leaked), "{input} -> {redacted}");
            assert!(redacted.contains("[redacted]"), "{input} -> {redacted}");
        }
        for (input, leaked) in [
            (
                "Fix /Users/sample/yard-worktrees/slack-notify/crates/yard-server/src/slack/mod.rs tick",
                "sample",
            ),
            ("edit (/home/dev/app/main.rs)", "dev/app"),
            ("see ~/notes.md", "notes"),
            ("run ./scripts/check.sh", "scripts"),
            ("open /etc/hosts", "hosts"),
        ] {
            let redacted = redact(input);
            assert!(!redacted.contains(leaked), "{input} -> {redacted}");
            assert!(redacted.contains("[path]"), "{input} -> {redacted}");
        }
        assert_eq!(redact("Ship the retry banner"), "Ship the retry banner");
        assert_eq!(
            redact("Fix and/or improve /health w/o regressions"),
            "Fix and/or improve /health w/o regressions"
        );
    }

    #[test]
    fn command_redaction_strips_environment_values_flag_secrets_and_url_credentials() {
        let token = concat!("xo", "xb-1-2-abcdefghijklmnop");
        for (input, expected) in [
            (
                "AWS_SECRET_ACCESS_KEY=abc123 FOO=bar cargo test",
                "AWS_SECRET_ACCESS_KEY=[redacted] FOO=[redacted] cargo test",
            ),
            (
                "curl --token hunter2 --password=pw -H x",
                "curl --token [redacted] --password=[redacted] -H x",
            ),
            (
                "git clone https://alice:s3cret@example.com/repo.git",
                "git clone https://[redacted]@example.com/repo.git",
            ),
            (
                "export GITHUB_TOKEN=ghp_x && make",
                "export GITHUB_TOKEN=[redacted] && make",
            ),
            ("npm run build -- --watch", "npm run build -- --watch"),
            ("rm -rf /Users/sample/proj/build", "rm -rf [path]"),
            (
                "curl -u admin:hunter2 https://api.example.com",
                "curl -u [redacted] https://api.example.com",
            ),
            ("curl --user=admin:hunter2 x", "curl --user=[redacted] x"),
            (
                "curl -H \"Authorization: Basic dXNlcjpodW50ZXIy\" https://api.example.com",
                "curl -H \"Authorization: [redacted] [redacted]\" https://api.example.com",
            ),
            (
                "Authorization: Token abc def",
                "Authorization: [redacted] [redacted] [redacted]",
            ),
            (
                "curl -H \"X-Api-Key: k3y-short-99\" x",
                "curl -H \"X-Api-Key: [redacted] x",
            ),
            (
                "mysql -u root -phunter2 prod",
                "mysql -u [redacted] -p[redacted] prod",
            ),
            (
                "sshpass -p hunter2 ssh prod-host",
                "sshpass -p [redacted] ssh prod-host",
            ),
            ("db_password = \"hunter2\"", "db_password = [redacted]"),
            ("private_key: abc", "private_key: [redacted]"),
        ] {
            assert_eq!(redact_command(input), expected, "{input}");
        }
        assert_eq!(
            redact_command(&format!("SLACK={token} echo {token}")),
            "SLACK=[redacted] echo [redacted]"
        );
        assert_eq!(
            clean_prompt_line("  echo\t<!channel> &\u{7}  done "),
            "echo &lt;!channel&gt; &amp; done"
        );
    }

    #[test]
    fn test_message_is_plain() {
        let message = test_message(0);
        assert!(message.text.starts_with("Yard test message"));
    }
}

//! What the board can tell about an agent's conversation from the
//! provider's own session files (read-only, bounded): the conversation
//! title Claude / Codex give it (the same text they set as the terminal
//! title), when the user or the agent last wrote, and a short digest of
//! the latest messages for the optional AI pass.
//!
//! Claude: `<claude config>/projects/<dir>/<session id>.jsonl` (entries
//! `ai-title`, `user`, `assistant`). Codex: the thread name from
//! `<codex home>/session_index.jsonl` and the rollout file
//! `<codex home>/sessions/YYYY/MM/DD/rollout-…-<session id>.jsonl`.
//! Only regular files are read (no symlinks), only the last
//! [`MAX_TAIL_BYTES`] of a transcript (plus, when that tail misses the
//! opening request, a line-by-line scan of at most [`MAX_HEAD_BYTES`] from
//! the start that stops at the first typed message), and nothing is ever
//! written.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde_json::Value;

/// Bytes read from the end of one transcript.
pub(crate) const MAX_TAIL_BYTES: u64 = 256 * 1024;
/// Bytes scanned from the start of a long conversation for its opening
/// request (stops at the first typed message).
pub(crate) const MAX_HEAD_BYTES: u64 = 256 * 1024;
/// Largest Codex `session_index.jsonl` read (the newest part is kept).
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;
/// Directory entries visited while looking for Codex rollout files.
const MAX_WALK_ENTRIES: usize = 20_000;
/// Conversations read per generation.
pub(crate) const MAX_CONVERSATIONS: usize = 96;
/// Messages kept in a digest, and characters per message.
const DIGEST_MESSAGES: usize = 12;
const DIGEST_MESSAGE_CHARS: usize = 300;
/// Characters kept of the opening request and the latest agent message.
const EXCERPT_CHARS: usize = 600;

/// One agent conversation, as far as its files tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Conversation {
    /// The provider's own title of the conversation (Claude `ai-title`,
    /// Codex thread name), unredacted (callers clean it).
    pub title: Option<String>,
    /// Newest message of anyone (user or agent), Unix ms.
    pub last_activity_ms: Option<u64>,
    /// Newest message the user (or Yard on their behalf) typed, Unix ms.
    pub last_user_ms: Option<u64>,
    /// Whether the read part has a message a person typed.
    pub human: bool,
    /// The whole file was read (not just its tail), so `human == false`
    /// really means nobody typed to it.
    pub complete: bool,
    /// One of Yard's own summary workers (its contract prompt or title).
    pub yard_summary: bool,
    /// The latest messages, oldest first, as `You: …` / `Agent: …` lines.
    pub digest: String,
    /// The earliest message a person typed in the part read (the opening
    /// request when the whole file was read), unredacted, its opening only.
    pub first_user: Option<String>,
    /// The agent's latest message, unredacted, its opening only.
    pub last_agent: Option<String>,
    /// The newest message a person typed (not Yard's own prompts),
    /// unredacted, its opening only: what the agent is on now.
    pub last_user: Option<String>,
}

/// The opening of the prompt Yard sends its summary workers.
const SUMMARY_PROMPT: &str = "Complete the summary objective now";
/// The title providers give that prompt's conversation.
const SUMMARY_TITLE: &str = "summarize worker progress";

impl Conversation {
    /// Someone may talk to it: typed text was seen, or only a tail was read.
    pub(crate) fn maybe_human(&self) -> bool {
        self.human || !self.complete
    }

    /// One of Yard's summary workers.
    pub(crate) fn is_yard_summary(&self) -> bool {
        self.yard_summary
            || self
                .title
                .as_deref()
                .is_some_and(|title| title.trim().eq_ignore_ascii_case(SUMMARY_TITLE))
    }
}

/// Where the provider files live.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConversationRoots {
    pub claude_projects: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

impl ConversationRoots {
    /// `$CLAUDE_CONFIG_DIR` (else `~/.claude`) and `$CODEX_HOME` (else
    /// `~/.codex`), like the child-agent observer.
    pub(crate) fn from_env() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".claude")));
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".codex")));
        Self {
            claude_projects: claude.map(|path| path.join("projects")),
            codex_home: codex,
        }
    }

    /// Read the conversations of `sessions` (`(provider, session id)`),
    /// keyed by session id. Unknown providers, unsafe ids and missing files
    /// are skipped. Blocking: run it off the async runtime.
    pub(crate) fn read(&self, sessions: &[(String, String)]) -> HashMap<String, Conversation> {
        let mut claude = Vec::new();
        let mut codex = HashSet::new();
        for (provider, id) in sessions.iter().take(MAX_CONVERSATIONS) {
            if !safe_id(id) {
                continue;
            }
            match provider.to_ascii_lowercase().as_str() {
                "claude" => claude.push(id.clone()),
                "codex" => {
                    codex.insert(id.clone());
                }
                _ => {}
            }
        }
        let mut out = HashMap::new();
        if let Some(root) = self.claude_projects.as_deref()
            && !claude.is_empty()
        {
            let dirs = subdirectories(root);
            for id in claude {
                let file = format!("{id}.jsonl");
                let Some(path) = dirs
                    .iter()
                    .map(|dir| dir.join(&file))
                    .find(|path| regular_file(path))
                else {
                    continue;
                };
                if let Some((bytes, complete)) = read_tail_complete(&path, MAX_TAIL_BYTES) {
                    let mut conversation = parse_claude(&bytes);
                    conversation.complete = complete;
                    if !complete {
                        opening(&path, parse_claude, &mut conversation);
                    }
                    out.insert(id, conversation);
                }
            }
        }
        if let Some(home) = self.codex_home.as_deref()
            && !codex.is_empty()
        {
            let names = read_tail(&home.join("session_index.jsonl"), MAX_INDEX_BYTES)
                .map(|bytes| codex_thread_names(&bytes))
                .unwrap_or_default();
            let files = codex_rollouts(&home.join("sessions"), &codex);
            for id in codex {
                let mut conversation = files
                    .get(&id)
                    .and_then(|path| read_tail_complete(path, MAX_TAIL_BYTES))
                    .map(|(bytes, complete)| {
                        let mut conversation = parse_codex(&bytes);
                        conversation.complete = complete;
                        if !complete && let Some(path) = files.get(&id) {
                            opening(path, parse_codex, &mut conversation);
                        }
                        conversation
                    })
                    .unwrap_or_default();
                conversation.title = names.get(&id).cloned();
                if conversation != Conversation::default() {
                    out.insert(id, conversation);
                }
            }
        }
        out
    }
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn subdirectories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .take(MAX_WALK_ENTRIES)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// The rollout file of each wanted Codex session (`…-<id>.jsonl`), found
/// by walking `sessions/YYYY/MM/DD` (bounded, no symlinks).
fn codex_rollouts(root: &Path, wanted: &HashSet<String>) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let mut visited = 0_usize;
    let mut stack = vec![(root.to_path_buf(), 0_u8)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            visited += 1;
            if visited > MAX_WALK_ENTRIES {
                return out;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 3 {
                stack.push((entry.path(), depth + 1));
            } else if kind.is_file() {
                let name = entry.file_name();
                let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".jsonl")) else {
                    continue;
                };
                if let Some(id) = wanted.iter().find(|id| {
                    stem.len() > id.len()
                        && stem.ends_with(id.as_str())
                        && stem.as_bytes()[stem.len() - id.len() - 1] == b'-'
                }) {
                    out.insert(id.clone(), entry.path());
                }
            }
        }
    }
    out
}

/// The opening request of a conversation whose tail did not reach it: the
/// first typed message within the file's first [`MAX_HEAD_BYTES`], read
/// line by line and stopping there (Codex files open with long injected
/// context, so the first prompt can sit 100+ KiB in).
fn opening(path: &Path, parse: fn(&[u8]) -> Conversation, conversation: &mut Conversation) {
    use std::io::BufRead as _;
    if !regular_file(path) {
        return;
    }
    let Ok(file) = File::open(path) else {
        return;
    };
    let mut reader = std::io::BufReader::new(file.take(MAX_HEAD_BYTES));
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.last() != Some(&b'\n') {
            return;
        }
        if let Some(first) = parse(&line).first_user {
            conversation.first_user = Some(first);
            return;
        }
    }
}

fn read_tail(path: &Path, limit: u64) -> Option<Vec<u8>> {
    read_tail_complete(path, limit).map(|(bytes, _)| bytes)
}

/// The last `limit` bytes (from a line start) and whether that is the whole file.
fn read_tail_complete(path: &Path, limit: u64) -> Option<(Vec<u8>, bool)> {
    if !regular_file(path) {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    if start > 0 {
        let first_newline = bytes.iter().position(|byte| *byte == b'\n')?;
        bytes.drain(..=first_newline);
    }
    Some((bytes, start == 0))
}

fn timestamp_ms(value: &Value) -> Option<u64> {
    let text = value.get("timestamp")?.as_str()?;
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    u64::try_from(parsed.timestamp_millis()).ok()
}

/// Text a person typed (not a tool result, an injected context block or a
/// command echo).
fn typed_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty()
        || text.starts_with('<')
        || text.starts_with("# AGENTS.md")
        || text.starts_with("Caveat:")
        || text.starts_with("[Request interrupted")
        || text.starts_with("Base directory for this skill")
    {
        return None;
    }
    Some(text.to_owned())
}

fn bounded(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= DIGEST_MESSAGE_CHARS {
        return line;
    }
    let mut cut = line.chars().take(DIGEST_MESSAGE_CHARS).collect::<String>();
    cut.push('\u{2026}');
    cut
}

/// A message Yard (or another agent) sent that is not the chat's work: a
/// summary request or a relayed session message. The reply to it is not
/// the agent's latest outcome either.
fn yard_message(text: &str) -> bool {
    const OPENINGS: &[&str] = &[
        "AUTOMATIC WORKER SUMMARY REQUEST",
        "Another Claude session sent a message",
        "Another Codex session sent a message",
    ];
    let text = text.trim_start();
    OPENINGS.iter().any(|opening| text.starts_with(opening))
}

/// A prompt Yard wrote (a task, a continuation, a directive): the task
/// itself says what the work is, so it is not the chat's opening request.
fn yard_prompt(text: &str) -> bool {
    const OPENINGS: &[&str] = &[
        "Yard assignment",
        "Yard orchestrator workflow profile",
        "Continue:",
        "Final coordinator directive",
    ];
    let text = text.trim_start();
    OPENINGS.iter().any(|opening| text.starts_with(opening))
}

/// Who the agent is answering, for its latest outcome and the opening
/// request. A reply to one of Yard's own messages (a summary request, a
/// relayed message) is not the agent's outcome. A reply read before any
/// user message in the tail is kept only if the next user message is a
/// person's (or none follows).
#[derive(Default)]
struct Turns {
    yard_turn: bool,
    seen_user: bool,
    tentative: Option<String>,
}

impl Turns {
    fn user(&mut self, out: &mut Conversation, text: &str) {
        self.yard_turn = yard_message(text);
        if !self.seen_user {
            self.seen_user = true;
            let tentative = self.tentative.take();
            if !self.yard_turn {
                out.last_agent = tentative.or(out.last_agent.take());
            }
        }
        if !self.yard_turn && !yard_prompt(text) && !shell_paste(text) {
            if out.first_user.is_none() {
                out.first_user = Some(excerpt(text));
            }
            out.last_user = Some(excerpt(text));
        }
    }

    fn agent(&mut self, out: &mut Conversation, text: &str) {
        if !self.seen_user {
            self.tentative = Some(excerpt(text));
        } else if !self.yard_turn {
            out.last_agent = Some(excerpt(text));
        }
    }

    fn finish(&mut self, out: &mut Conversation) {
        if !self.seen_user {
            out.last_agent = self.tentative.take();
        }
    }
}

/// A pasted shell prompt ("user@host ~ % …"): no topic in it.
fn shell_paste(text: &str) -> bool {
    let first = text.split_whitespace().next().unwrap_or_default();
    first.contains('@') && (text.contains(" % ") || text.contains(" $ "))
}

/// The opening of a message (line breaks kept), for one-line summaries.
fn excerpt(text: &str) -> String {
    text.trim().chars().take(EXCERPT_CHARS).collect()
}

fn content_texts(content: &Value, kinds: &[&str]) -> Vec<String> {
    match content {
        Value::String(text) => vec![text.clone()],
        Value::Array(parts) => parts
            .iter()
            .filter(|part| {
                part.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kinds.contains(&kind))
            })
            .filter_map(|part| part.get("text").and_then(Value::as_str).map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn digest(messages: &[String]) -> String {
    let start = messages.len().saturating_sub(DIGEST_MESSAGES);
    messages[start..].join("\n")
}

/// One Claude transcript tail.
pub(crate) fn parse_claude(bytes: &[u8]) -> Conversation {
    let mut out = Conversation::default();
    let mut messages = Vec::new();
    let mut turns = Turns::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "ai-title" || kind == "custom-title" {
            let key = if kind == "ai-title" {
                "aiTitle"
            } else {
                "customTitle"
            };
            if let Some(title) = entry.get(key).and_then(Value::as_str)
                && !title.trim().is_empty()
            {
                out.title = Some(title.trim().to_owned());
            }
            continue;
        }
        if kind != "user" && kind != "assistant" {
            continue;
        }
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let at = timestamp_ms(&entry);
        if let Some(at) = at {
            out.last_activity_ms = Some(out.last_activity_ms.map_or(at, |old| old.max(at)));
        }
        let content = entry.pointer("/message/content").unwrap_or(&Value::Null);
        if kind == "user" {
            for text in content_texts(content, &["text"])
                .iter()
                .filter_map(|text| typed_text(text))
            {
                if text.starts_with(SUMMARY_PROMPT) {
                    out.yard_summary = true;
                    continue;
                }
                out.human = true;
                if let Some(at) = at {
                    out.last_user_ms = Some(out.last_user_ms.map_or(at, |old| old.max(at)));
                }
                turns.user(&mut out, &text);
                messages.push(format!("You: {}", bounded(&text)));
            }
        } else {
            for text in content_texts(content, &["text"]) {
                if !text.trim().is_empty() {
                    turns.agent(&mut out, &text);
                    messages.push(format!("Agent: {}", bounded(&text)));
                }
            }
        }
    }
    turns.finish(&mut out);
    out.digest = digest(&messages);
    out
}

/// One Codex rollout tail (the thread name comes from the index).
pub(crate) fn parse_codex(bytes: &[u8]) -> Conversation {
    let mut out = Conversation::default();
    let mut messages = Vec::new();
    let mut turns = Turns::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let at = timestamp_ms(&entry);
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = entry.get("payload").unwrap_or(&Value::Null);
        let payload_kind = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (user, texts) = match (kind, payload_kind) {
            ("response_item", "message") => {
                let user = payload.get("role").and_then(Value::as_str) == Some("user");
                let kinds: &[&str] = if user {
                    &["input_text"]
                } else {
                    &["output_text"]
                };
                (
                    user,
                    content_texts(payload.get("content").unwrap_or(&Value::Null), kinds),
                )
            }
            ("response_item" | "event_msg", _) => {
                if let Some(at) = at {
                    out.last_activity_ms = Some(out.last_activity_ms.map_or(at, |old| old.max(at)));
                }
                continue;
            }
            _ => continue,
        };
        if let Some(at) = at {
            out.last_activity_ms = Some(out.last_activity_ms.map_or(at, |old| old.max(at)));
        }
        for text in texts {
            if user {
                let Some(text) = typed_text(&text) else {
                    continue;
                };
                if text.starts_with(SUMMARY_PROMPT) {
                    out.yard_summary = true;
                    continue;
                }
                out.human = true;
                if let Some(at) = at {
                    out.last_user_ms = Some(out.last_user_ms.map_or(at, |old| old.max(at)));
                }
                turns.user(&mut out, &text);
                messages.push(format!("You: {}", bounded(&text)));
            } else if !text.trim().is_empty() {
                turns.agent(&mut out, &text);
                messages.push(format!("Agent: {}", bounded(&text)));
            }
        }
    }
    turns.finish(&mut out);
    out.digest = digest(&messages);
    out
}

/// Codex `session_index.jsonl`: the newest thread name per session id.
pub(crate) fn codex_thread_names(bytes: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (
            entry.get("id").and_then(Value::as_str),
            entry.get("thread_name").and_then(Value::as_str),
        ) && !name.trim().is_empty()
        {
            out.insert(id.to_owned(), name.trim().to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn claude_title_activity_and_digest() {
        let text = concat!(
            r#"{"type":"user","timestamp":"2026-10-20T10:00:00Z","message":{"role":"user","content":"Fix the upload retry"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-10-20T10:01:00Z","message":{"content":[{"type":"text","text":"Looking at it"},{"type":"tool_use","name":"Bash"}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-10-20T10:02:00Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-10-20T10:03:00Z","isSidechain":true,"message":{"content":"helper prompt"}}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"Upload retry fix"}"#,
            "\n",
            "not json\n",
        );
        let conversation = parse_claude(text.as_bytes());
        assert_eq!(conversation.title.as_deref(), Some("Upload retry fix"));
        assert!(conversation.human);
        let user = chrono::DateTime::parse_from_rfc3339("2026-10-20T10:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(conversation.last_user_ms, u64::try_from(user).ok());
        // The tool result (10:02) is activity; the side chain is not.
        assert_eq!(
            conversation.last_activity_ms,
            u64::try_from(user + 120_000).ok()
        );
        assert_eq!(
            conversation.digest,
            "You: Fix the upload retry\nAgent: Looking at it"
        );
    }

    #[test]
    fn codex_messages_skip_injected_context() {
        let text = concat!(
            r#"{"timestamp":"2026-10-20T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-10-20T10:00:05Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Audit the caches"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-10-20T10:09:00Z","type":"event_msg","payload":{"type":"token_count"}}"#,
            "\n",
        );
        let conversation = parse_codex(text.as_bytes());
        assert!(conversation.human);
        assert_eq!(conversation.digest, "You: Audit the caches");
        assert!(conversation.last_activity_ms > conversation.last_user_ms);
    }

    #[test]
    fn a_conversation_without_typed_text_is_not_human() {
        let text = r#"{"type":"user","timestamp":"2026-10-20T10:00:00Z","message":{"content":[{"type":"tool_result","content":"x"}]}}"#;
        let mut conversation = parse_claude(text.as_bytes());
        assert!(!conversation.human);
        // Only a whole file proves nobody typed to it.
        assert!(conversation.maybe_human());
        conversation.complete = true;
        assert!(!conversation.maybe_human());
    }

    #[test]
    fn yard_summary_workers_are_recognised() {
        let text = r#"{"timestamp":"2026-10-20T10:00:05Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Complete the summary objective now. Persist exactly one artifact"}]}}"#;
        let conversation = parse_codex(text.as_bytes());
        assert!(conversation.is_yard_summary() && !conversation.human);
        let titled = Conversation {
            title: Some("Summarize worker progress".to_owned()),
            ..Conversation::default()
        };
        assert!(titled.is_yard_summary());
    }

    #[test]
    fn reads_files_by_session_id_and_never_follows_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let claude = temp.path().join("claude/projects");
        let codex = temp.path().join("codex");
        write(
            &claude.join("-home-yard/abc-1.jsonl"),
            r#"{"type":"ai-title","aiTitle":"Docs pass"}"#,
        );
        write(
            &codex.join("sessions/2026/10/20/rollout-2026-10-20T01-29-58-01a1-77.jsonl"),
            r#"{"timestamp":"2026-10-20T10:00:05Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}}"#,
        );
        write(
            &codex.join("session_index.jsonl"),
            "{\"id\":\"01a1-77\",\"thread_name\":\"Old name\"}\n{\"id\":\"01a1-77\",\"thread_name\":\"Cache audit\"}\n",
        );
        let outside = temp.path().join("secret.jsonl");
        write(&outside, r#"{"type":"ai-title","aiTitle":"Leaked"}"#);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, claude.join("-home-yard/link-2.jsonl")).unwrap();
        let roots = ConversationRoots {
            claude_projects: Some(claude),
            codex_home: Some(codex),
        };
        let read = roots.read(&[
            ("claude".to_owned(), "abc-1".to_owned()),
            ("claude".to_owned(), "link-2".to_owned()),
            ("claude".to_owned(), "../secret".to_owned()),
            ("Codex".to_owned(), "01a1-77".to_owned()),
        ]);
        assert_eq!(read["abc-1"].title.as_deref(), Some("Docs pass"));
        assert!(!read.contains_key("link-2"));
        assert!(!read.contains_key("../secret"));
        assert_eq!(read["01a1-77"].title.as_deref(), Some("Cache audit"));
        assert!(read["01a1-77"].human);
    }
}

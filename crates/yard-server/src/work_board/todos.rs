//! To-do extraction helpers: the `next-actions.md` reader, text
//! normalisation and the dedupe pass.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Most list items kept from one project's `next-actions.md`.
pub(crate) const MAX_SNAPSHOT_ITEMS: usize = 10;
/// Longest kept to-do text, in characters.
pub(crate) const MAX_TODO_CHARS: usize = 200;
/// Token Jaccard similarity at or above which two to-dos of one project merge.
pub(crate) const NEAR_DUPLICATE: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Urgency {
    Now,
    Soon,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TodoKind {
    Reply,
    Unblock,
    Investigate,
    Review,
    Next,
    ReceiptBlocker,
    Snapshot,
    Close,
    Triage,
    CleanUp,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct TodoSource {
    /// `item` (a board card), `project` or `node`.
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Todo {
    /// Stable hash of the normalised text; the web keys dismissals on it.
    pub id: String,
    pub text: String,
    pub kind: TodoKind,
    pub urgency: Urgency,
    pub project_id: Option<String>,
    pub sources: Vec<TodoSource>,
    pub age_ms: u64,
}

/// One snapshot list item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotLine {
    pub text: String,
    pub checkbox: bool,
}

/// The open list items of a `next-actions.md`: `- [ ] `, `- `, `* ` and
/// `1. ` lines outside code fences (checked `- [x]` items are skipped),
/// at most [`MAX_SNAPSHOT_ITEMS`], each at most [`MAX_TODO_CHARS`].
pub(crate) fn parse_next_actions(markdown: &str) -> Vec<SnapshotLine> {
    let mut lines = Vec::new();
    let mut fenced = false;
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let item = if let Some(rest) = line
            .strip_prefix("- [ ] ")
            .or_else(|| line.strip_prefix("* [ ] "))
        {
            Some((rest, true))
        } else if line.starts_with("- [x]")
            || line.starts_with("- [X]")
            || line.starts_with("* [x]")
            || line.starts_with("* [X]")
        {
            None
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            Some((rest, false))
        } else {
            numbered(line).map(|rest| (rest, false))
        };
        let Some((text, checkbox)) = item else {
            continue;
        };
        let text = crate::redact::clean_text(text, MAX_TODO_CHARS);
        if text.is_empty() {
            continue;
        }
        lines.push(SnapshotLine { text, checkbox });
        if lines.len() == MAX_SNAPSHOT_ITEMS {
            break;
        }
    }
    lines
}

fn numbered(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    line[digits..].strip_prefix(". ")
}

/// Lowercase, strip punctuation and markdown, collapse whitespace.
pub(crate) fn normalise(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The stable id of a to-do text.
pub(crate) fn todo_id(text: &str) -> String {
    let digest = Sha256::digest(normalise(text).as_bytes());
    digest[..8].iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn jaccard(left: &BTreeSet<&str>, right: &BTreeSet<&str>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = left.intersection(right).count() as f64 / union as f64;
    ratio
}

/// Free-text kinds may be near-duplicates of each other; generated per-item
/// kinds (reply, unblock, review, investigate, close, ...) name their item and
/// only ever merge on identical text.
const fn fuzzy_kind(kind: TodoKind) -> bool {
    matches!(
        kind,
        TodoKind::Next | TodoKind::Snapshot | TodoKind::Model | TodoKind::ReceiptBlocker
    )
}

/// Exact-hash dedupe within a project, then, within a project and among
/// free-text kinds only, merge a later entry whose token Jaccard similarity is
/// at least [`NEAR_DUPLICATE`]. A merge keeps the higher urgency, the older
/// age, and the union of sources. Order is kept. The same text in two
/// projects stays two to-dos (the second gets a project-qualified id).
pub(crate) fn dedupe(todos: Vec<Todo>) -> Vec<Todo> {
    let mut kept: Vec<Todo> = Vec::new();
    let mut by_key: HashMap<(Option<String>, String), usize> = HashMap::new();
    let mut normalised: Vec<String> = Vec::new();
    for mut todo in todos {
        let key = (todo.project_id.clone(), todo.id.clone());
        let found = by_key.get(&key).copied().or_else(|| {
            if !fuzzy_kind(todo.kind) {
                return None;
            }
            let norm = normalise(&todo.text);
            let tokens = norm.split(' ').collect::<BTreeSet<_>>();
            kept.iter()
                .zip(&normalised)
                .position(|(other, other_norm)| {
                    other.project_id == todo.project_id
                        && fuzzy_kind(other.kind)
                        && jaccard(&tokens, &other_norm.split(' ').collect()) >= NEAR_DUPLICATE
                })
        });
        if let Some(index) = found {
            let existing = &mut kept[index];
            existing.urgency = existing.urgency.min(todo.urgency);
            existing.age_ms = existing.age_ms.max(todo.age_ms);
            for source in todo.sources {
                if !existing.sources.contains(&source) {
                    existing.sources.push(source);
                }
            }
            continue;
        }
        by_key.insert(key, kept.len());
        if kept.iter().any(|other| other.id == todo.id) {
            let project = todo.project_id.as_deref().unwrap_or("");
            todo.id = todo_id(&format!("{project} {}", todo.text));
        }
        normalised.push(normalise(&todo.text));
        kept.push(todo);
    }
    kept
}

/// Split a free-text `next` into bullets: by lines, list markers and `; `.
pub(crate) fn split_bullets(text: &str) -> Vec<String> {
    text.lines()
        .flat_map(|line| line.split("; "))
        .map(|part| {
            let part = part.trim();
            let part = part
                .strip_prefix("- [ ] ")
                .or_else(|| part.strip_prefix("- "))
                .or_else(|| part.strip_prefix("* "))
                .or_else(|| numbered(part))
                .unwrap_or(part);
            crate::redact::clean_text(part, MAX_TODO_CHARS)
        })
        .filter(|part| !part.is_empty())
        .collect()
}

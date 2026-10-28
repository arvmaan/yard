//! The optional model pass (P1 with a model): for the cards on the main
//! board (and ambiguous stale ones), write the three plain card lines
//! (About / Status / Your move) and, for an untitled card, a short title;
//! for R5 "ambiguous" items also a verdict and cited next actions (only
//! those verdicts can move a card). Bounded: at most 4 batches of 5,
//! temperature 0, 1000 output tokens per batch, <= 40k input / 4k output tokens, <= $0.10 per
//! generate and <= $1.00 per local day, all checked before any process is
//! spawned. Every string sent or shown passes through [`crate::redact`].

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

use serde::Deserialize;
use serde_json::{Value, json};

use super::board::{AiNote, Item};
use super::rules::Verdict;
use crate::model::{self, ConverseRunner, Stop, price};

pub(crate) const MAX_BATCH_ITEMS: usize = 5;
pub(crate) const MAX_BATCHES: usize = 4;
pub(crate) const MAX_MODEL_ITEMS: usize = MAX_BATCH_ITEMS * MAX_BATCHES;
/// Output tokens allowed per item in a batch (`maxTokens` = this x items,
/// so a full batch of 5 gets 1000 and 4 batches stay within 4k).
pub(crate) const ITEM_MAX_TOKENS: u64 = 200;
pub(crate) const BATCH_MAX_TOKENS: u64 = ITEM_MAX_TOKENS * MAX_BATCH_ITEMS as u64;
pub(crate) const MAX_GENERATE_USD: f64 = 0.10;
pub(crate) const DAILY_USD: f64 = 1.00;
pub(crate) const MAX_INPUT_TOKENS: u64 = 40_000;
pub(crate) const MAX_OUTPUT_TOKENS: u64 = 4_000;
pub(crate) const TAIL_LINES: u32 = 60;
pub(crate) const MAX_TAIL_BYTES: usize = 4 * 1024;
const MAX_OBJECTIVE_CHARS: usize = 600;
const MAX_ONE_LINE_CHARS: usize = 100;
const MAX_NEXT_CHARS: usize = 120;
const MAX_QUOTE_CHARS: usize = 120;
const MAX_AI_TITLE_CHARS: usize = 60;
const MAX_LINE_CHARS: usize = 140;
const MAX_DIGEST_BYTES: usize = 2 * 1024;
const MAX_NEXT_PER_ITEM: usize = 3;
const LRU_CAPACITY: usize = 256;
/// Rough bytes per token for the pre-flight estimate (over-, not under-).
const BYTES_PER_TOKEN: u64 = 4;
/// Worst-case evidence bytes of one item (objective, report, blockers,
/// snapshot lines, tail, conversation digest and JSON framing), for the
/// estimate at `POST`.
const WORST_ITEM_BYTES: u64 = 8 * 1024 + 512;

pub(crate) const SYSTEM_PROMPT: &str = "You triage coding-agent work items. Use only the given evidence. Reply with JSON only, matching the schema.";

const SCHEMA_HINT: &str = r#"Reply with a JSON array, one object per item: [{"id": string (an input id), "verdict": "needs_work"|"finished"|"waiting_on_user"|"unclear", "confidence": number 0-1, "one_line": string (<=100 chars, a plain-English sentence for someone who has not seen the terminal: what the agent is working on and where it stands; no ids, paths, commands or log lines), "next_actions": [{"text": string (<=120 chars, a short imperative sentence a person can act on), "source_id": string (an input id)}] (<=3), "evidence": "report"|"receipt"|"snapshot"|"tail"|"objective", "quote": string (<=120 chars, copied from the evidence), "title": string (only when the item has "needs_title": true: 3-7 words naming the work, like a document title; else ""), "about": string (<=120 chars: what the work is, for someone who never saw it; a request, not a question or a detail from the middle of the chat; "" when nothing more than the title is known), "status": string (<=120 chars: where it is at in plain words; no times, dates or ages; start with "Finished:" only when the agent said the work is complete, "Found:" for a result or warning that is not a completion, "Blocked:" when it waits on something only the user can do), "your_move": string (<=120 chars: one short sentence telling the user what to do, naming the decision or the object (which CR, doc or deploy), never a generic "Review the result"; or "Nothing, it is working" when it is busy)}]. Never use ids, paths, commands, profile names or log lines in title, about, status or your_move. Copy nothing that looks like a secret."#;

/// The configured model. `None` in settings means the pass is off.
#[derive(Clone)]
pub(crate) struct ModelConfig {
    pub model_id: String,
    pub runner: Arc<dyn ConverseRunner>,
}

/// One item's evidence, already redacted and bounded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Evidence {
    pub id: String,
    pub title: String,
    pub objective: String,
    /// The deterministic one-line status and the age in whole days.
    pub status: String,
    pub age_days: u64,
    pub report: Option<Value>,
    pub receipt_blockers: Vec<String>,
    pub snapshot_next: Vec<String>,
    pub tail: String,
    /// Where the board placed it, in plain words ("Needs you").
    pub place: String,
    /// Ask for a 3-7 word title (nothing better exists).
    pub needs_title: bool,
    /// The latest messages of its conversation, scrubbed.
    pub digest: String,
}

impl Evidence {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "objective": self.objective,
            "status": self.status,
            "age_days": self.age_days,
            "report": self.report,
            "receipt_blockers": self.receipt_blockers,
            "snapshot_next": self.snapshot_next,
            "tail": self.tail,
            "board_place": self.place,
            "needs_title": self.needs_title,
            "conversation": self.digest,
        })
    }
}

/// A conversation digest for the model: scrubbed like a tail, at most
/// [`MAX_DIGEST_BYTES`] (newest kept).
pub(crate) fn scrub_digest(digest: &str) -> String {
    scrub_tail(digest, MAX_DIGEST_BYTES)
}

pub(crate) fn bounded_objective(objective: &str) -> String {
    crate::redact::clean_text(objective, MAX_OBJECTIVE_CHARS)
}

fn env_assignment(line: &str) -> bool {
    let trimmed = line.trim_start();
    let trimmed = trimmed
        .strip_prefix("export ")
        .map_or(trimmed, str::trim_start);
    let Some((name, _)) = trimmed.split_once('=') else {
        return false;
    };
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Board-only scrubs before anything is sent: lines with environment
/// assignments, `Authorization:` / `Bearer ` lines and PEM blocks are
/// dropped; every kept line goes through [`crate::redact::redact_command`]
/// (env values, flag secrets, URL credentials, tokens, key ids, paths).
/// Keeps the last [`TAIL_LINES`] lines and at most `max_bytes` (newest kept).
pub(crate) fn scrub_tail(text: &str, max_bytes: usize) -> String {
    let mut kept = Vec::new();
    let mut in_pem = false;
    for raw in text.lines() {
        let lower = raw.to_ascii_lowercase();
        if lower.contains("-----begin") {
            in_pem = true;
            continue;
        }
        if in_pem {
            if lower.contains("-----end") {
                in_pem = false;
            }
            continue;
        }
        if env_assignment(raw) || lower.contains("authorization:") || lower.contains("bearer ") {
            continue;
        }
        let line = raw
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>();
        let line = crate::redact::redact_command(line.trim_end());
        if !line.trim().is_empty() {
            kept.push(line);
        }
    }
    let start = kept.len().saturating_sub(TAIL_LINES as usize);
    let mut out = Vec::new();
    let mut bytes = 0;
    for line in kept[start..].iter().rev() {
        if bytes + line.len() + 1 > max_bytes {
            break;
        }
        bytes += line.len() + 1;
        out.push(line.as_str());
    }
    out.reverse();
    out.join("\n")
}

/// The user message of one batch.
pub(crate) fn user_message(now_ms: u64, batch: &[Evidence]) -> String {
    let body = json!({
        "now": now_ms,
        "items": batch.iter().map(Evidence::to_json).collect::<Vec<_>>(),
    });
    format!("{SCHEMA_HINT}\n\n{body}")
}

fn tokens(bytes: usize) -> u64 {
    (u64::try_from(bytes).unwrap_or(u64::MAX)).div_ceil(BYTES_PER_TOKEN)
}

/// `maxTokens` of a batch of `items`.
pub(crate) fn batch_max_tokens(items: usize) -> u64 {
    ITEM_MAX_TOKENS * u64::try_from(items.clamp(1, MAX_BATCH_ITEMS)).unwrap_or(1)
}

/// Pre-flight estimate of one generate of `items` in `batches`: input
/// tokens, output tokens (every batch's `maxTokens`) and USD.
pub(crate) fn estimate(
    model_id: &str,
    input_bytes: usize,
    batches: usize,
    items: usize,
) -> (u64, u64, f64) {
    let input = tokens(input_bytes + SYSTEM_PROMPT.len() * batches);
    let full = items / MAX_BATCH_ITEMS;
    let rest = items % MAX_BATCH_ITEMS;
    let output = BATCH_MAX_TOKENS * u64::try_from(full).unwrap_or(u64::MAX)
        + if rest > 0 { batch_max_tokens(rest) } else { 0 };
    let rate = price::rate_for(model_id, input);
    #[allow(clippy::cast_precision_loss)]
    let usd = (input as f64 * rate.input + output as f64 * rate.output) / 1_000_000.0;
    (input, output, usd)
}

/// The worst-case estimate for `items` ambiguous items (used on `POST`).
pub(crate) fn worst_case(model_id: &str, items: usize) -> f64 {
    let items = items.clamp(1, MAX_MODEL_ITEMS);
    let bytes = (WORST_ITEM_BYTES * u64::try_from(items).unwrap_or(u64::MAX))
        .min(MAX_INPUT_TOKENS * BYTES_PER_TOKEN);
    estimate(
        model_id,
        usize::try_from(bytes).unwrap_or(usize::MAX),
        items.div_ceil(MAX_BATCH_ITEMS),
        items,
    )
    .2
}

/// The worst case of the largest item count, up to `items`, that fits the
/// per-generate cap (at least one item: the pass trims, it never sends more).
pub(crate) fn worst_case_within_cap(model_id: &str, items: usize) -> f64 {
    let mut count = items.clamp(1, MAX_MODEL_ITEMS);
    while count > 1 && worst_case(model_id, count) > MAX_GENERATE_USD {
        count -= 1;
    }
    worst_case(model_id, count)
}

/// Why the model pass was refused before any call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CapError {
    PerGenerate(f64),
    Daily(f64),
    InputTokens(u64),
    OutputTokens(u64),
}

impl std::fmt::Display for CapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PerGenerate(usd) => write!(
                formatter,
                "cost cap: estimate ${usd:.3} is over ${MAX_GENERATE_USD:.2} per generate"
            ),
            Self::Daily(usd) => write!(
                formatter,
                "cost cap: ${usd:.3} more would pass the ${DAILY_USD:.2} daily cap"
            ),
            Self::InputTokens(count) => write!(
                formatter,
                "cost cap: {count} input tokens is over {MAX_INPUT_TOKENS}"
            ),
            Self::OutputTokens(count) => write!(
                formatter,
                "cost cap: {count} output tokens is over {MAX_OUTPUT_TOKENS}"
            ),
        }
    }
}

/// In-memory spend per local day (resets at local midnight).
#[derive(Default)]
pub(crate) struct Budget {
    day: Mutex<Option<(chrono::NaiveDate, f64)>>,
}

impl Budget {
    fn today() -> chrono::NaiveDate {
        chrono::Local::now().date_naive()
    }

    pub(crate) fn spent_today(&self) -> f64 {
        let day = self.day.lock().unwrap_or_else(PoisonError::into_inner);
        match *day {
            Some((date, spent)) if date == Self::today() => spent,
            _ => 0.0,
        }
    }

    pub(crate) fn add(&self, usd: f64) {
        let mut day = self.day.lock().unwrap_or_else(PoisonError::into_inner);
        let today = Self::today();
        let spent = match *day {
            Some((date, spent)) if date == today => spent,
            _ => 0.0,
        };
        *day = Some((today, (spent + usd).max(0.0)));
    }

    pub(crate) fn check(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        usd: f64,
    ) -> Result<(), CapError> {
        if input_tokens > MAX_INPUT_TOKENS {
            return Err(CapError::InputTokens(input_tokens));
        }
        if output_tokens > MAX_OUTPUT_TOKENS {
            return Err(CapError::OutputTokens(output_tokens));
        }
        if usd > MAX_GENERATE_USD {
            return Err(CapError::PerGenerate(usd));
        }
        if self.spent_today() + usd > DAILY_USD {
            return Err(CapError::Daily(usd));
        }
        Ok(())
    }
}

/// A validated verdict for one item.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ItemVerdict {
    pub verdict: Verdict,
    pub confidence: f64,
    pub one_line: String,
    pub next_actions: Vec<String>,
    pub evidence: String,
    pub quote: String,
    pub title: String,
    pub about: String,
    pub status: String,
    pub your_move: String,
}

impl ItemVerdict {
    /// Attach the AI note. Only an ambiguous item takes the one-line
    /// summary as its status line (others keep the agent's own words); the
    /// verdict itself only ever moves ambiguous items (`rules`).
    pub(crate) fn apply(&self, item: &mut Item) {
        item.ai = Some(AiNote {
            verdict: self.verdict,
            confidence: self.confidence,
            evidence: self.evidence.clone(),
            quote: self.quote.clone(),
            next_actions: self.next_actions.clone(),
            title: self.title.clone(),
            about: self.about.clone(),
            status: self.status.clone(),
            your_move: self.your_move.clone(),
        });
        let ambiguous = item.base.rule == super::rules::Rule::R5
            && item.base.section == Some(super::rules::Section::Ambiguous);
        if ambiguous && !self.one_line.is_empty() {
            item.status_line.clone_from(&self.one_line);
        }
        item.pending = false;
    }
}

/// A model-written card line: bounded, redacted, one line, and dropped
/// when it carries an id-like token, a path or a profile-style name.
fn card_line(text: &str, max: usize) -> String {
    let line = crate::redact::clean_text(text.lines().next().unwrap_or_default().trim(), max);
    let suspicious = line.split_whitespace().any(|word| {
        word.contains('/') && word.len() > 1
            || word.starts_with("yard-")
            || word.starts_with("term_")
            || word.bytes().filter(u8::is_ascii_hexdigit).count() >= 12
    });
    if suspicious || line.contains('[') {
        String::new()
    } else {
        line
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNext {
    text: String,
    source_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVerdict {
    id: String,
    verdict: Verdict,
    confidence: f64,
    #[serde(default)]
    one_line: String,
    #[serde(default)]
    next_actions: Vec<RawNext>,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    about: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    your_move: String,
}

/// A 3-7 word AI title, else empty (also when it reads like an id).
fn ai_title(text: &str) -> String {
    let title = card_line(text, MAX_AI_TITLE_CHARS)
        .trim_end_matches(['.', '!'])
        .trim_matches(['"', '\'', '\u{201c}', '\u{201d}'])
        .to_owned();
    let words = title.split_whitespace().count();
    if (2..=8).contains(&words) && super::titles::meaningful(&title, &HashSet::new()) {
        title
    } else {
        String::new()
    }
}

const EVIDENCE_KINDS: &[&str] = &["report", "receipt", "snapshot", "tail", "objective"];

/// Strictly parse and validate one batch reply. Unknown ids and uncited
/// next actions are dropped; strings are trimmed, bounded and re-redacted.
pub(crate) fn validate(
    text: &str,
    batch_ids: &HashSet<String>,
) -> Result<HashMap<String, ItemVerdict>, String> {
    let trimmed = text.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |rest| rest.trim_end().trim_end_matches("```"))
        .trim();
    let raw: Vec<RawVerdict> = serde_json::from_str(trimmed)
        .map_err(|_| "the model reply was not valid JSON for the schema".to_owned())?;
    let mut out = HashMap::new();
    for entry in raw {
        if !batch_ids.contains(&entry.id) || out.contains_key(&entry.id) {
            continue;
        }
        if !entry.confidence.is_finite() || !(0.0..=1.0).contains(&entry.confidence) {
            continue;
        }
        let next_actions = entry
            .next_actions
            .into_iter()
            .filter(|next| batch_ids.contains(&next.source_id))
            .map(|next| crate::redact::clean_text(&next.text, MAX_NEXT_CHARS))
            .filter(|text| !text.is_empty())
            .take(MAX_NEXT_PER_ITEM)
            .collect();
        let evidence = if EVIDENCE_KINDS.contains(&entry.evidence.as_str()) {
            entry.evidence
        } else {
            "objective".to_owned()
        };
        out.insert(
            entry.id,
            ItemVerdict {
                verdict: entry.verdict,
                confidence: entry.confidence,
                one_line: crate::redact::clean_text(&entry.one_line, MAX_ONE_LINE_CHARS),
                next_actions,
                evidence,
                quote: crate::redact::clean_text(&entry.quote, MAX_QUOTE_CHARS),
                title: ai_title(&entry.title),
                about: card_line(&entry.about, MAX_LINE_CHARS),
                status: card_line(&entry.status, MAX_LINE_CHARS),
                your_move: card_line(&entry.your_move, MAX_LINE_CHARS),
            },
        );
    }
    Ok(out)
}

/// Results of one batch call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BatchOutcome {
    pub verdicts: HashMap<String, ItemVerdict>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
    /// The reply carried usage, so `usd` is the real cost. Otherwise the
    /// caller keeps the pre-flight estimate charged.
    pub measured: bool,
    pub error: Option<String>,
}

/// Run one batch through the runner and validate the reply.
pub(crate) async fn run_batch(
    config: &ModelConfig,
    now_ms: u64,
    batch: &[Evidence],
) -> BatchOutcome {
    let ids = batch
        .iter()
        .map(|item| item.id.clone())
        .collect::<HashSet<_>>();
    let request = model::request(
        &config.model_id,
        SYSTEM_PROMPT,
        &user_message(now_ms, batch),
        batch_max_tokens(batch.len()),
    );
    let failed = |error: String| BatchOutcome {
        verdicts: HashMap::new(),
        input_tokens: 0,
        output_tokens: 0,
        usd: 0.0,
        measured: false,
        error: Some(crate::redact::clean_text(&error, 160)),
    };
    let response = match config.runner.converse(&request).await {
        Ok(response) => response,
        Err(error) => return failed(error.to_string()),
    };
    let reply = match model::parse(&config.model_id, &response) {
        Ok(reply) => reply,
        Err(error) => return failed(error.to_string()),
    };
    let spent = |mut outcome: BatchOutcome| {
        outcome.input_tokens = reply.input_tokens;
        outcome.output_tokens = reply.output_tokens;
        outcome.usd = reply.cost_usd;
        outcome.measured = true;
        outcome
    };
    if reply.stop != Stop::Complete {
        return spent(failed(match &reply.stop {
            Stop::MaxTokens => "the model reply hit maxTokens".to_owned(),
            Stop::Other(reason) => format!("the model stopped: {reason}"),
            Stop::Complete => unreachable!(),
        }));
    }
    match validate(&reply.text, &ids) {
        Ok(verdicts) => spent(BatchOutcome {
            verdicts,
            input_tokens: 0,
            output_tokens: 0,
            usd: 0.0,
            measured: true,
            error: None,
        }),
        Err(error) => spent(failed(error)),
    }
}

/// Per-item results keyed by an evidence fingerprint, so an unchanged item
/// is never sent twice.
#[derive(Default)]
pub(crate) struct Lru {
    entries: HashMap<String, ItemVerdict>,
    order: VecDeque<String>,
}

impl Lru {
    pub(crate) fn get(&mut self, key: &str) -> Option<ItemVerdict> {
        let found = self.entries.get(key).cloned()?;
        if let Some(position) = self.order.iter().position(|entry| entry == key) {
            self.order.remove(position);
        }
        self.order.push_back(key.to_owned());
        Some(found)
    }

    pub(crate) fn put(&mut self, key: String, verdict: ItemVerdict) {
        if self.entries.insert(key.clone(), verdict).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > LRU_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

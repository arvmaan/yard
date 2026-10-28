//! Plain-language text for the board. Every sentence a person reads on a
//! card or in the to-do list is built here from templates, never from raw
//! status fields: what the agent is, what is happening now, and what (if
//! anything) to do next. Rule ids, availability codes, runtime ids and
//! model numbers stay out of these strings (the web shows them under
//! "Details").

use yard_domain::{ObservedStatus, WorkerAvailability};

use super::board::{Action, Item};
use super::reply::{self, Reply};
use super::rules::{Column, ItemKind, Section};

/// The agent's latest message says it is blocked on something only the
/// user can clear (credentials, access, an approval, a decision).
pub(crate) const CHIP_BLOCKED_ON_YOU: &str = "blocked on you";
/// The agent's latest message ends with a question for the user.
pub(crate) const CHIP_ASKED_YOU: &str = "asked you";
/// Needs-you groups: a question or a blocker to answer, or finished work to
/// review (less urgent).
pub(crate) const NEED_ANSWER: &str = "answer";
pub(crate) const NEED_REVIEW: &str = "review";

/// Why a card is a helper (hidden unless "Show helpers").
pub(crate) const HELPER_TEMPORARY: &str = "A temporary session Yard started";
pub(crate) const HELPER_NO_CONVERSATION: &str = "No conversation with a person";
pub(crate) const HELPER_SUMMARY: &str = "A Yard summary worker";
pub(crate) const HELPER_IDLE: &str = "A Yard coordinator waiting for work";

/// A Yard-level coordinator (Yard orchestrator, workstream) that is quiet
/// with nothing open: Yard's own plumbing, asking nothing of the user.
pub(crate) fn idle_coordinator(item: &Item) -> bool {
    item.kind == ItemKind::Worker
        && coordinator(item)
        && item.column == Column::Stale
        && item.next_actions.is_empty()
        && !item.facts.explicit_next
}

/// A Herdr agent name Yard generates for its own short-lived sessions
/// ("yard-a1b2c3d-0123456789abcdef", "yard-0f1e2d3c4b5a69788796a5b4").
pub(crate) fn temporary_session_name(name: &str) -> bool {
    let Some(rest) = name.trim().strip_prefix("yard-") else {
        return false;
    };
    let hex = rest.bytes().filter(u8::is_ascii_hexdigit).count();
    !rest.is_empty()
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        && hex >= 12
}

/// Labels a prompt may open with that say nothing ("Task:", "Objective:").
const LABELS: &[&str] = &[
    "task:",
    "objective:",
    "goal:",
    "context:",
    "request:",
    "prompt:",
    "todo:",
    "ask:",
];

/// Words that point at context the card does not have ("seen here").
const DEICTIC: &[&str] = &["here", "above", "below", "attached"];
const DEICTIC_LEAD: &[&str] = &[
    "seen",
    "shown",
    "listed",
    "mentioned",
    "noted",
    "linked",
    "pasted",
];

/// Words a clause must not end on.
const DANGLING: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "into", "of", "on", "or", "so",
    "that", "the", "to", "we", "with", "how", "can", "our", "my", "your", "is", "are", "this",
    "just",
];

/// One glanceable clause from free text (a prompt or an agent message):
/// markdown, links, opening labels ("Task:") and pleasantries dropped, cut
/// at the first sentence or clause break, pointing words ("seen here")
/// removed, at most `max_words` words (never ending on a joining word),
/// first letter upper-cased, redacted. `None` when fewer than three words
/// are left or it would carry a path or secret.
#[allow(clippy::too_many_lines)]
pub(crate) fn clause(text: &str, max_words: usize) -> Option<String> {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| line.chars().any(char::is_alphanumeric))?;
    let line = first
        .split_whitespace()
        .filter(|word| !word.contains("://"))
        .map(|word| word.trim_matches(|ch: char| matches!(ch, '*' | '`' | '#' | '>' | '_' | '~')))
        // Inline code and emphasis marks inside a word ("sev3_alarms`.").
        .map(|word| word.replace(['`', '*'], ""))
        .filter(|word| !word.is_empty() && word != "-" && word != "\u{2022}")
        .collect::<Vec<_>>()
        .join(" ");
    let mut text = line.trim();
    loop {
        let lower = text.to_lowercase();
        if let Some(label) = LABELS.iter().find(|label| lower.starts_with(*label)) {
            text = text[label.len()..].trim_start();
            continue;
        }
        let stripped = super::titles::strip_fillers(text);
        if stripped.len() == text.len() {
            break;
        }
        text = stripped;
    }
    let cut = text
        .find(['.', '?', '!', ';', ':', '(', '\n', '\u{b7}', '|'])
        .map_or(text, |end| &text[..end]);
    let lower = cut.to_lowercase();
    let cut = [
        ", and ",
        ", but ",
        ", so ",
        " so that ",
        ", which ",
        " - ",
        " \u{2014} ",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min()
    .map_or(cut, |end| &cut[..end]);
    let mut words: Vec<&str> = Vec::new();
    for word in cut.split_whitespace() {
        if !word.chars().any(char::is_alphanumeric) {
            continue;
        }
        let bare = word
            .trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_lowercase();
        if DEICTIC.contains(&bare.as_str()) {
            if words.last().is_some_and(|last| {
                DEICTIC_LEAD.contains(
                    &last
                        .to_lowercase()
                        .trim_matches(|ch: char| !ch.is_alphanumeric()),
                )
            }) {
                words.pop();
            }
            continue;
        }
        words.push(word);
    }
    if words.len() > max_words
        && let Some(at) = words
            .iter()
            .position(|word| word.eq_ignore_ascii_case("and"))
            .filter(|at| *at >= 3 && *at <= max_words)
    {
        words.truncate(at);
    }
    let cut_short = words.len() > max_words;
    words.truncate(max_words);
    while words.len() > 2
        && words.last().is_some_and(|word| {
            let bare = word
                .to_lowercase()
                .trim_matches(|ch: char| !ch.is_alphanumeric())
                .to_owned();
            // "where they come from" ends a clause; "copies from" does not.
            let phrasal = bare == "from"
                && words.len() >= 2
                && ["come", "comes", "came", "coming"].contains(
                    &words[words.len() - 2]
                        .to_lowercase()
                        .trim_matches(|ch: char| !ch.is_alphanumeric()),
                );
            !phrasal && DANGLING.contains(&bare.as_str())
        })
    {
        words.pop();
    }
    if words.len() < 3 {
        return None;
    }
    let mut short = words
        .join(" ")
        .trim_end_matches([',', ';', ':', '-', ' ', '"', '\'', '\u{201c}', '\u{201d}'])
        .to_owned();
    if cut_short {
        short.push('\u{2026}');
    }
    let mut chars = short.chars();
    let first = chars.next()?;
    let short = format!("{}{}", first.to_uppercase(), chars.as_str());
    let short = crate::redact::clean_text(&short, 140);
    let unmatched = short.matches(')').count() > short.matches('(').count();
    if unmatched || short.contains("[path]") || short.contains("[redacted") || short.contains('[') {
        return None;
    }
    Some(short)
}

/// Most words in an About line.
pub(crate) const ABOUT_WORDS: usize = 12;

/// "About" from a task prompt: one clause of at most 12 words, unless the
/// title already says it.
pub(crate) fn about_task(objective: &str, title: &str) -> Option<String> {
    let line = objective
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let short = clause(line, ABOUT_WORDS)?;
    (good_about(line, &short) && !same_words(&short, title)).then_some(short)
}

/// Fewest words in an About line taken from a prompt ("Add the setter"
/// says too little).
const ABOUT_MIN_WORDS: usize = 4;

/// Whether a clause can say what the work is: not a question to the agent
/// ("Is it X or Y?"), not a condition or a reason ("When several services
/// list one FQDN…"), not cut off mid-sentence, at least four words.
fn good_about(sentence: &str, short: &str) -> bool {
    const ASKING: &[&str] = &[
        "is", "are", "was", "were", "what", "which", "why", "how", "where", "when", "who", "does",
        "do", "did", "should", "would", "could", "can", "will", "have", "has",
    ];
    const SETTING: &[&str] = &[
        "when", "if", "because", "since", "while", "although", "once", "unless", "whenever",
    ];
    let first = |text: &str| {
        text.split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_lowercase()
    };
    let stripped = super::titles::strip_fillers(sentence.trim());
    let question = sentence.trim_end().ends_with('?') && ASKING.contains(&first(stripped).as_str());
    // Commit ids and hashes ("a1b2c3d") mean nothing on a card.
    let hash = short.split_whitespace().any(|word| {
        let word = word.trim_matches(|ch: char| !ch.is_alphanumeric());
        word.len() >= 7
            && word.chars().all(|ch| ch.is_ascii_hexdigit())
            && word.chars().any(|ch| ch.is_ascii_digit())
            && word.chars().any(|ch| ch.is_ascii_alphabetic())
    });
    !question
        && !hash
        && !SETTING.contains(&first(short).as_str())
        && !short.ends_with('\u{2026}')
        && short.split_whitespace().count() >= ABOUT_MIN_WORDS
}

/// "About" from the conversation's opening request (the first thing the
/// user typed), unless the title already says it. Set-up sentences
/// ("Read-only research task", "FINAL LOCAL-ONLY TASK", "Do not …") are
/// skipped for the first one that says what the work is.
pub(crate) fn about_request(first_user: Option<&str>, title: &str) -> Option<String> {
    let mut rest = first_user?.trim();
    for index in 0..8 {
        if rest.is_empty() {
            break;
        }
        let end = sentence_end(rest, &['.', '!', '?', '\n']);
        let sentence = &rest[..end];
        rest = rest[end..].trim_start();
        if setup_sentence(sentence) {
            continue;
        }
        let Some(short) = clause(sentence, ABOUT_WORDS).filter(|short| good_about(sentence, short))
        else {
            continue;
        };
        // Past the opening sentence only a request says what the work is
        // ("Find where … come from"); statements there are context or
        // findings ("MConnSync writes these docs …").
        if index > 0 && !request_opening(&short) {
            continue;
        }
        return (!same_words(&short, title)).then_some(short);
    }
    None
}

/// The end of the first sentence of `text` (just past the mark): a mark
/// only ends one when a space, a line break or the end follows it, so
/// "notes.md" or "17:06" do not.
pub(crate) fn sentence_end(text: &str, marks: &[char]) -> usize {
    text.char_indices()
        .find(|(at, ch)| {
            marks.contains(ch)
                && (*ch == '\n'
                    || text[at + ch.len_utf8()..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace))
        })
        .map_or(text.len(), |(at, ch)| at + ch.len_utf8())
}

/// A clause that opens with a request verb ("Find", "Review", "Plan").
fn request_opening(short: &str) -> bool {
    const VERBS: &[&str] = &[
        "add",
        "analyze",
        "audit",
        "build",
        "check",
        "compare",
        "compile",
        "continue",
        "create",
        "debug",
        "decide",
        "deploy",
        "design",
        "determine",
        "draft",
        "explain",
        "figure",
        "find",
        "fix",
        "help",
        "identify",
        "implement",
        "inspect",
        "investigate",
        "list",
        "look",
        "make",
        "map",
        "migrate",
        "plan",
        "prepare",
        "research",
        "review",
        "run",
        "set",
        "summarize",
        "test",
        "trace",
        "update",
        "verify",
        "write",
    ];
    let first = short
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    VERBS.contains(&first.as_str())
}

/// A sentence that sets rules instead of saying what the work is.
fn setup_sentence(sentence: &str) -> bool {
    const OPENINGS: &[&str] = &[
        "read-only",
        "read only",
        "do not",
        "don't",
        "never",
        "important",
        "note",
        "context",
        "background",
        "you are",
        "you're",
        "rules",
        "constraints",
        "work independently",
        "you start",
        "you will",
        "your task",
        "complete the task",
    ];
    let text = sentence.trim_matches(|ch: char| !ch.is_alphanumeric());
    let lower = text.to_lowercase();
    let letters = text
        .chars()
        .filter(|ch| ch.is_alphabetic())
        .collect::<Vec<_>>();
    let shouting = letters.len() >= 6 && letters.iter().all(|ch| ch.is_uppercase());
    shouting || OPENINGS.iter().any(|opening| lower.starts_with(opening))
}

/// What the agent last said it did (or asked), as a short clause; `None`
/// when its latest message is only a lead-in ("Done.", "Here is the
/// summary:") or carries nothing usable.
pub(crate) fn outcome(last_agent: Option<&str>) -> Option<(String, bool)> {
    const LEAD_INS: &[&str] = &[
        "done",
        "all set",
        "all done",
        "perfect",
        "great",
        "ok",
        "okay",
        "sure",
        "got it",
        "summary",
        "here's",
        "here is",
        "here are",
        "status",
        "update",
        "result",
        "results",
        "finished",
        "complete",
        "completed",
        "excellent",
        "good",
    ];
    let text = last_agent?.trim();
    let asked = text.trim_end().ends_with('?');
    // Try each sentence in turn: skip lead-ins and tiny fragments.
    let mut rest = text;
    for _ in 0..4 {
        if rest.trim().is_empty() {
            break;
        }
        let end = sentence_end(rest, &['.', '!', ':', '\n']);
        let sentence = &rest[..end];
        rest = &rest[end..];
        if reply::ack_sentence(sentence) {
            continue;
        }
        let bare = sentence
            .trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_lowercase();
        if bare.split_whitespace().count() < 3
            || LEAD_INS.iter().any(|lead| {
                bare == *lead
                    || bare.starts_with(&format!("{lead} ")) && bare.split_whitespace().count() <= 4
            })
        {
            continue;
        }
        if let Some(short) = clause(without_pronoun(sentence), ABOUT_WORDS) {
            return Some((short, asked));
        }
    }
    None
}

/// A reply without its "I've" / "You're right that" opening, so it reads
/// as what was done ("Updated the doc").
fn without_pronoun(sentence: &str) -> &str {
    const OPENINGS: &[&str] = &[
        "you're right that ",
        "you\u{2019}re right that ",
        "you're right, ",
        "you\u{2019}re right, ",
        "good catch, ",
        "i've ",
        "i\u{2019}ve ",
        "i have ",
        "we've ",
        "we have ",
        "i'm ",
        "i\u{2019}m ",
        "i am ",
        "we're ",
        "we\u{2019}re ",
        "i ",
    ];
    let mut text = sentence.trim_start_matches(|ch: char| !ch.is_alphanumeric());
    loop {
        let lower = text.to_lowercase();
        match OPENINGS.iter().find(|opening| lower.starts_with(*opening)) {
            Some(opening) => text = text[opening.len()..].trim_start(),
            None => return text,
        }
    }
}

/// The noun a result is about ("plan", "CR"), for a specific Your move.
pub(crate) fn artifact(last_agent: Option<&str>) -> Option<&'static str> {
    // The opening of the message says what it delivered; later lines
    // mention everything else.
    let text = last_agent?.chars().take(240).collect::<String>();
    let words = text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let lower = words
        .iter()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>();
    let has = |word: &str| lower.iter().any(|candidate| candidate == word);
    if words.iter().any(|word| *word == "CR" || *word == "CRs") {
        return Some("CR");
    }
    if words.iter().any(|word| *word == "PR" || *word == "PRs") {
        return Some("pull request");
    }
    for (word, noun) in [
        ("plan", "plan"),
        ("design", "design"),
        ("doc", "doc"),
        ("document", "doc"),
        ("report", "report"),
        ("draft", "draft"),
        ("proposal", "proposal"),
        ("fix", "fix"),
        ("tests", "tests"),
        ("script", "script"),
    ] {
        if has(word) {
            return Some(noun);
        }
    }
    None
}

/// "About" from the conversation title, unless the card's title is it.
pub(crate) fn about_conversation(topic: Option<&str>, title: &str) -> Option<String> {
    let topic = topic.map(str::trim).filter(|topic| !topic.is_empty())?;
    let topic = crate::redact::clean_text(topic, 140);
    (!same_words(&topic, title)).then_some(topic)
}

/// A conversation topic a person can read: not Yard's own bookkeeping
/// ("web-app yard orchestration").
pub(crate) fn plain_topic(topic: &str) -> bool {
    let lower = topic.to_lowercase();
    !lower.contains("orchestrat") && !lower.split_whitespace().any(|word| word == "yard")
}

fn same_words(left: &str, right: &str) -> bool {
    let words = |text: &str| {
        text.to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (left, right) = (words(left), words(right));
    let set = |list: &[String]| {
        list.iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    !left.is_empty()
        && (left == right
            || (right.len() >= 3 && left.starts_with(&right))
            // The same words in another order ("CR 1 review comments" /
            // "Review comments on CR 1") say nothing new either.
            || (left.len() >= 2 && set(&left).is_subset(&set(&right))))
}

/// "Status:" — where the work is at, in plain words, without an age (the
/// web adds "2 h ago" when the age is known). Never "N/A" or a made-up age;
/// empty when nothing beyond the age is known. "Finished:" only when the
/// agent said the work is complete; a result that is not ("gamma still
/// runs an old image") is "Found:".
#[allow(clippy::too_many_lines)]
pub(crate) fn status(item: &Item) -> String {
    let open_steps = if !item.next_actions.is_empty() {
        ", with open steps"
    } else if item.facts.explicit_next {
        ", its project has open steps"
    } else {
        ""
    };
    let said = last_outcome(item);
    let reply = item.reply.as_ref();
    let question = reply::question_text(item.last_agent.as_deref());
    let from_reply = |said: &Option<(String, bool)>| -> Option<String> {
        let (text, asked) = said.as_ref()?;
        let text = lower_first(text);
        Some(match reply? {
            _ if *asked => format!(
                "Asks: {}",
                lower_first(question.as_deref().unwrap_or(&text))
            ),
            Reply::Blocked(need) => format!("{}: {text}", reply::blocked_lead(need)),
            Reply::Remaining(left) => format!("Stopped with {left}"),
            Reply::Done => format!("Finished: {text}"),
            Reply::Finding => format!("Found: {text}"),
            Reply::Question => {
                format!(
                    "Asks: {}",
                    lower_first(question.as_deref().unwrap_or(&text))
                )
            }
            Reply::Ack => return None,
        })
    };
    match item.column {
        Column::Closed => "Closed".to_owned(),
        Column::Doing => {
            if has(item, "starting") {
                "Starting up".to_owned()
            } else if has(item, "reported working") && !has(item, "working") {
                "Working, according to its latest report".to_owned()
            } else {
                working(item)
            }
        }
        Column::NeedsYou => match lead_chip(item) {
            Some(CHIP_BLOCKED_ON_YOU | CHIP_ASKED_YOU | "ready for review") => {
                from_reply(&said).unwrap_or_else(|| summary(item))
            }
            _ => summary(item),
        },
        Column::Close => match (agent_note(item), reply, &said) {
            (None, Some(Reply::Done), Some((done, false))) if !has(item, "dead runtimes") => {
                format!("Finished: {}", lower_first(done))
            }
            _ => summary(item),
        },
        Column::Stale if item.section == Some(Section::LikelyDone) => {
            let base = if ai_finished(item) {
                "Quiet, looks finished"
            } else {
                "Probably finished, nothing left open"
            };
            match (reply, &said) {
                (Some(Reply::Done), Some((done, _))) => format!("Finished: {}", lower_first(done)),
                (Some(Reply::Finding), Some((found, _))) => {
                    format!("{base}; last reply: {}", lower_first(found))
                }
                _ => base.to_owned(),
            }
        }
        Column::Stale => {
            let finding = match (reply, &said) {
                (Some(Reply::Finding), Some((found, _))) => Some(lower_first(found)),
                _ => None,
            };
            let base = if let Some(Reply::Remaining(left)) = reply {
                format!("Stopped with {left}")
            } else if has(item, "loose agent") || has(item, "unassigned") {
                // Without a reply only its age is known (the web shows it);
                // with no age either, say where it is.
                match finding {
                    Some(found) => format!("Last reply: {found}"),
                    None if item.activity_known => String::new(),
                    None => "Open in a terminal".to_owned(),
                }
            } else if has(item, "resumable") {
                format!("Stopped, can be resumed{open_steps}")
            } else if coordinator(item) {
                format!("Idle, waiting for work{open_steps}")
            } else if has(item, "unknown") {
                "Status unclear".to_owned()
            } else if let Some(found) = finding {
                format!("Gone quiet{open_steps}; last reply: {found}")
            } else {
                format!("Gone quiet{open_steps}")
            };
            if has(item, "?") && !base.is_empty() {
                format!("{base} (unclear if finished)")
            } else {
                base
            }
        }
    }
}

/// A working card's Status: what it says it is doing, and the subagents
/// working under it ("Working: applying the fixes, with 3 subagents").
fn working(item: &Item) -> String {
    let note = agent_note(item).or_else(|| {
        last_outcome(item)
            .filter(|(_, asked)| !asked)
            .map(|(text, _)| text)
    });
    let helpers = match item.subagents.len() {
        0 => String::new(),
        1 => format!("1 subagent: {}", lower_first(&item.subagents[0])),
        count => format!("{count} subagents"),
    };
    match (note, helpers.is_empty()) {
        (Some(note), true) => format!("Working: {}", lower_first(&note)),
        (Some(note), false) => format!("Working: {}, with {helpers}", lower_first(&note)),
        (None, false) => format!("Working with {helpers}"),
        (None, true) => "Working now".to_owned(),
    }
}

/// "Your move:" — what the user should do, or "Nothing — it's working".
/// Names its object (the CR, the doc) when the agent's message does.
pub(crate) fn your_move(item: &Item) -> String {
    if item.column == Column::Doing {
        return "Nothing \u{2014} it's working".to_owned();
    }
    if item.column == Column::Closed {
        return "Nothing \u{2014} it's closed".to_owned();
    }
    if item.column == Column::Stale && coordinator(item) && item.next_actions.is_empty() {
        return "Nothing \u{2014} it's idle".to_owned();
    }
    if item.suggest_archive && item.relevance == super::board::Relevance::Older {
        // The age is on the card and in the review's banner.
        return "Archive it if you're done with it.".to_owned();
    }
    let mark = item.actions.contains(&Action::MarkDone);
    if item.column == Column::NeedsYou && lead_chip(item) == Some("ready for review") {
        if let Some((_, true)) = last_outcome(item) {
            return "Answer its question, or mark it done if that's all.".to_owned();
        }
        let object = reply::cr_id(item.last_agent.as_deref()).or_else(|| {
            artifact(item.last_agent.as_deref())
                .filter(|noun| *noun != "fix" && *noun != "tests")
                .map(|noun| format!("the {noun}"))
        });
        match (&item.reply, object) {
            (Some(Reply::Blocked(need)), _) => return reply::unblock_move(need),
            (Some(Reply::Remaining(left)), _) => {
                return format!("Tell it to continue ({left}), or mark it done if that's enough.");
            }
            (Some(Reply::Finding), Some(object)) => {
                return format!("Read what it found on {object} and decide the next step.");
            }
            (Some(Reply::Finding), None) => {
                return "Read what it found and decide the next step.".to_owned();
            }
            (_, Some(object)) => {
                return if mark {
                    format!("Review {object}, then mark it done.")
                } else {
                    format!("Review {object}.")
                };
            }
            _ => {}
        }
    }
    let step = next_step(item);
    if step.is_empty() {
        "Nothing right now".to_owned()
    } else {
        step
    }
}

/// What the agent last said, as a clause, and whether it asked something.
fn last_outcome(item: &Item) -> Option<(String, bool)> {
    outcome(item.last_agent.as_deref())
}

fn ai_finished(item: &Item) -> bool {
    item.ai
        .as_ref()
        .is_some_and(|ai| ai.verdict == super::rules::Verdict::Finished)
}

/// "under a minute", "5 min", "2 hr", "3 days", "2 months" (the same units
/// the web uses for card ages).
pub(crate) fn duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes < 1 {
        return "under a minute".to_owned();
    }
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} hr");
    }
    let days = hours / 24;
    let plural =
        |count: u64, unit: &str| format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    if days < 60 {
        return plural(days, "day");
    }
    plural(days / 30, "month")
}

/// The plain label of a placement chip (the chip itself is a stable code).
pub(crate) fn chip_label(chip: &str) -> Option<&'static str> {
    Some(match chip {
        "working" | "reported working" => "working",
        "starting" => "starting up",
        "ready for review" => "finished a turn",
        "failed" => "task failed",
        "ambiguous runtime" => "terminal unclear",
        "blocked" => "stuck",
        "question" | "waiting on you" | CHIP_ASKED_YOU => "has a question",
        CHIP_BLOCKED_ON_YOU => "blocked",
        "needs attention" => "asks for help",
        "command failed" => "message not delivered",
        "dead runtimes" => "not running",
        "receipt" => "left a report",
        "reported nothing next" => "says it is done",
        "yard orchestrator" => "main orchestrator",
        "workstream" => "workstream",
        "orchestrator" => "orchestrator",
        "stale" | "idle" => "quiet",
        "resumable" => "can be resumed",
        "unassigned" => "no task",
        "loose agent" => "outside Yard",
        // "?" (model unsure) and "unknown" have their own wording.
        _ => return None,
    })
}

/// Chips the column heading already says (a card in "Working now" need not
/// also say "working").
fn column_says(column: Column, chip: &str) -> bool {
    match column {
        Column::Doing => matches!(chip, "working" | "reported working" | "starting"),
        Column::Stale => matches!(chip, "stale" | "idle"),
        Column::Close => matches!(chip, "receipt" | "reported nothing next" | "dead runtimes"),
        Column::NeedsYou | Column::Closed => false,
    }
}

/// Plain labels for an item's chips, in order, without duplicates, and
/// without the ones its column or its summary already says: a chip on the
/// card face adds a fact (call it after [`summary`]).
pub(crate) fn labels(item: &Item) -> Vec<String> {
    let summary = item.summary.to_lowercase();
    let lead = lead_chip(item);
    let mut out: Vec<String> = Vec::new();
    for chip in &item.chips {
        let Some(label) = chip_label(chip) else {
            continue;
        };
        // "Outside Yard" is the card's own chip (said once).
        if chip == "loose agent"
            || column_says(item.column, chip)
            || lead == Some(chip.as_str())
            || summary.contains(&label.to_lowercase())
            || out.iter().any(|seen| seen == label)
        {
            continue;
        }
        out.push(label.to_owned());
    }
    out
}

/// The Needs-you chip the summary is written from (in the summary's order).
fn lead_chip(item: &Item) -> Option<&'static str> {
    if item.column != Column::NeedsYou {
        return None;
    }
    [
        CHIP_BLOCKED_ON_YOU,
        CHIP_ASKED_YOU,
        "question",
        "waiting on you",
        "blocked",
        "failed",
        "command failed",
        "ambiguous runtime",
        "needs attention",
        "ready for review",
    ]
    .into_iter()
    .find(|chip| has(item, chip))
}

/// A question the agent asked (an open step ending in "?") on a card that
/// needs the user.
pub(crate) fn question(item: &Item) -> Option<&str> {
    if item.column != Column::NeedsYou {
        return None;
    }
    item.next_actions
        .iter()
        .map(|step| step.trim())
        .find(|step| step.ends_with('?'))
}

/// How a Herdr status reads on a card.
pub(crate) const fn herdr_phrase(status: ObservedStatus, live: bool) -> &'static str {
    if !live {
        return "Not running right now";
    }
    match status {
        ObservedStatus::Working => "Working",
        ObservedStatus::Blocked => "Waiting for your input",
        ObservedStatus::Done => "Finished its turn",
        ObservedStatus::Idle => "Quiet",
        ObservedStatus::Unknown => "Open, status not reported",
    }
}

/// How a worker without a task reads on a card (instead of the store's
/// reservation reason, which goes to Details).
pub(crate) const fn availability_phrase(availability: WorkerAvailability) -> &'static str {
    match availability {
        WorkerAvailability::YardOrchestrator => "Coordinates Yard itself",
        WorkerAvailability::CoordinationNode => "Runs a workstream",
        WorkerAvailability::Orchestrator => "Coordinates its project",
        WorkerAvailability::Assigned => "Has a task in a project",
        WorkerAvailability::UnassignedLive => "Open, with no task assigned",
        WorkerAvailability::Resumable => "Stopped, can be resumed",
        WorkerAvailability::Unavailable => "Stopped, can't be resumed",
        WorkerAvailability::Ambiguous => "Yard can't tell which terminal is this agent's",
        WorkerAvailability::Ended => "Session ended",
    }
}

/// The agent named in the middle of a to-do: its title in quotes, so a
/// capitalised name does not read as a typo. The project is left out: the
/// to-do list is grouped by project already.
pub(crate) fn who(item: &Item) -> String {
    format!("\u{201c}{}\u{201d}", item.title)
}

fn has(item: &Item, chip: &str) -> bool {
    item.chips.iter().any(|candidate| candidate == chip)
}

/// An orchestrator or workstream: it coordinates others, so quiet is normal.
pub(crate) fn is_coordinator(item: &Item) -> bool {
    coordinator(item)
}

/// Which Needs-you group a card is in: finished work to review, or
/// something to answer (a question, a blocker, a failure).
pub(crate) fn need(item: &Item) -> Option<&'static str> {
    if item.column != Column::NeedsYou {
        return None;
    }
    let review = lead_chip(item) == Some("ready for review")
        && !item.reply.as_ref().is_some_and(Reply::waits_on_user);
    Some(if review { NEED_REVIEW } else { NEED_ANSWER })
}

fn coordinator(item: &Item) -> bool {
    item.kind == ItemKind::Orchestrator
        || has(item, "yard orchestrator")
        || has(item, "workstream")
        || has(item, "orchestrator")
}

/// What is happening now: a short fragment, no full stop.
#[allow(clippy::too_many_lines)]
pub(crate) fn summary(item: &Item) -> String {
    let unsure = has(item, "?");
    match item.column {
        Column::Closed => "Closed, nothing left to do".to_owned(),
        Column::NeedsYou => match lead_chip(item) {
            Some(CHIP_BLOCKED_ON_YOU) => "Blocked, waiting on you".to_owned(),
            Some("question" | "waiting on you" | CHIP_ASKED_YOU) => {
                "Has a question for you".to_owned()
            }
            Some("blocked") => "Stuck, waiting for your input".to_owned(),
            Some("failed") => "Its task failed".to_owned(),
            Some("command failed") => "Your last message to it did not go through".to_owned(),
            Some("ambiguous runtime") => {
                "Yard can't tell which terminal belongs to this agent".to_owned()
            }
            Some("needs attention") if question(item).is_some() => {
                "Has a question for you".to_owned()
            }
            Some("needs attention") => "Asking for your help in its latest report".to_owned(),
            Some("ready for review") => "Finished, waiting for your review".to_owned(),
            _ => "Waiting for you".to_owned(),
        },
        Column::Doing => {
            if has(item, "starting") {
                "Starting up".to_owned()
            } else if has(item, "reported working") && !has(item, "working") {
                "Working, according to its latest report".to_owned()
            } else {
                // What it is doing, in its own words; empty when it has said
                // nothing beyond its task (the card shows the task instead of
                // repeating the column heading).
                agent_note(item).unwrap_or_else(|| {
                    if item.objective.trim().is_empty() {
                        "Busy in its terminal".to_owned()
                    } else {
                        String::new()
                    }
                })
            }
        }
        Column::Close => {
            if has(item, "receipt") {
                // What it finished, in its own words, when it said.
                agent_note(item).map_or_else(
                    || "Finished and left a report".to_owned(),
                    |done| format!("Done: {}", lower_first(&done)),
                )
            } else if has(item, "reported nothing next") {
                "Says it is done, with nothing next".to_owned()
            } else if has(item, "dead runtimes") {
                "No longer running".to_owned()
            } else {
                "Finished".to_owned()
            }
        }
        Column::Stale if item.section == Some(Section::LikelyDone) => {
            let quiet =
                for_age(item).map_or_else(|| "Quiet".to_owned(), |age| format!("Quiet for {age}"));
            if ai_finished(item) {
                format!("{quiet}, looks finished")
            } else {
                format!("{quiet} with nothing left open, probably finished")
            }
        }
        Column::Stale => {
            // The steps themselves are listed under the summary.
            let open = if !item.next_actions.is_empty() {
                ", still has open steps"
            } else if item.facts.explicit_next {
                ", its project still has open steps"
            } else {
                ""
            };
            let fresh = item.activity_known && item.age_ms < 60_000;
            let quiet_for = for_age(item)
                .map(|age| format!(" for {age}"))
                .unwrap_or_default();
            let base = if has(item, "loose agent") {
                if fresh || quiet_for.is_empty() {
                    "Open in a terminal outside Yard".to_owned()
                } else {
                    format!("Open in a terminal outside Yard, quiet{quiet_for}")
                }
            } else if has(item, "resumable") {
                match for_age(item) {
                    Some(age) => format!("Stopped {age} ago, can be resumed{open}"),
                    None => format!("Stopped, can be resumed{open}"),
                }
            } else if has(item, "unassigned") {
                format!("Quiet{quiet_for}, with no task assigned")
            } else if coordinator(item) {
                if fresh {
                    format!("Open and quiet{open}")
                } else {
                    format!("Open and quiet{quiet_for}{open}")
                }
            } else if has(item, "unknown") {
                match for_age(item) {
                    Some(age) => format!("Status unclear, last activity {age} ago"),
                    None => "Status unclear".to_owned(),
                }
            } else if fresh {
                format!("Just went quiet{open}")
            } else {
                format!("Quiet{quiet_for}{open}")
            };
            if unsure {
                format!("{base} (the AI can't tell if it is finished)")
            } else {
                base
            }
        }
    }
}

/// The item's age for a sentence ("3 days"), `None` when it is not known.
fn for_age(item: &Item) -> Option<String> {
    item.activity_known.then(|| duration(item.age_ms))
}

/// What the user should do next: one or two full sentences, each ending
/// with a full stop (or the agent's own question mark); empty when nothing
/// is needed, including every "Working now" card (its column says so).
#[allow(clippy::too_many_lines)]
pub(crate) fn next_step(item: &Item) -> String {
    let can_open = item.actions.contains(&Action::OpenTerminal);
    match item.column {
        Column::Closed | Column::Doing => String::new(),
        Column::NeedsYou => match lead_chip(item) {
            Some(CHIP_BLOCKED_ON_YOU) => match &item.reply {
                Some(Reply::Blocked(need)) => reply::unblock_move(need),
                _ => "Open it and give it what it needs.".to_owned(),
            },
            Some("question" | "waiting on you" | CHIP_ASKED_YOU) => {
                "Open it and answer the question.".to_owned()
            }
            Some("blocked") => "Open it and give it what it needs.".to_owned(),
            Some("failed") => "Open it to see what went wrong, then retry or close it.".to_owned(),
            Some("command failed") => "Open it and send your message again.".to_owned(),
            Some("ambiguous runtime") => "Open the agent and pick the right terminal.".to_owned(),
            Some("needs attention") => match (question(item), can_open) {
                (Some(asked), true) => format!("Answer its question: {}", lower_first(asked)),
                (Some(asked), false) => {
                    format!("Open its project page to answer: {}", lower_first(asked))
                }
                (None, true) => "Open it, read its report and unblock it.".to_owned(),
                (None, false) => {
                    "Open its project page to read its report and unblock it.".to_owned()
                }
            },
            Some("ready for review") => {
                if item.actions.contains(&Action::MarkDone) {
                    "Review what it did, then mark it done.".to_owned()
                } else {
                    "Review what it did.".to_owned()
                }
            }
            _ => "Open it and see what it needs.".to_owned(),
        },
        Column::Close => {
            let kept = if has(item, "receipt") {
                " (its report is kept)"
            } else {
                ""
            };
            if item.actions.contains(&Action::ArchiveProject) {
                format!(
                    "Close it to free the terminal{kept}. You can archive the whole project too."
                )
            } else if item.kind == ItemKind::DeadAggregate {
                "Clean up the closed agents.".to_owned()
            } else {
                format!("Close it to free the terminal{kept}.")
            }
        }
        Column::Stale if item.section == Some(Section::LikelyDone) => {
            if item.reply == Some(Reply::Done) {
                if item.actions.contains(&Action::MarkDone) {
                    "Mark it done or close it.".to_owned()
                } else {
                    "Close it if you're done with it.".to_owned()
                }
            } else {
                "Check it is finished, then mark it done or close it.".to_owned()
            }
        }
        Column::Stale => {
            if let Some(Reply::Remaining(left)) = &item.reply {
                format!("Tell it to continue ({left}), or close it if you no longer need it.")
            } else if has(item, "loose agent") || has(item, "unassigned") {
                if matches!(last_outcome(item), Some((_, true))) {
                    "Answer its question, or close it if you're done with it.".to_owned()
                } else {
                    "Close it if you're done with it.".to_owned()
                }
            } else if has(item, "resumable") {
                "Resume it if you still need it.".to_owned()
            } else if coordinator(item) {
                "Nothing to do unless you have work for it.".to_owned()
            } else if let Some(step) = item.next_actions.first() {
                format!(
                    "Tell it to continue with \u{201c}{}\u{201d}, or close it if you no longer need it.",
                    step.trim().trim_end_matches('.')
                )
            } else if item.facts.explicit_next {
                "Tell it to continue with its project's next step, or close it if you no longer need it."
                    .to_owned()
            } else {
                "Check whether it is still needed. Close it if not.".to_owned()
            }
        }
    }
}

/// The now to-do a Needs-you chip asks for, naming the agent (every to-do
/// is an order with the agent in quotes). `item` gives the review its
/// result line.
pub(crate) fn chip_todo(chip: &str, item: &Item, who: &str) -> Option<String> {
    Some(match chip {
        "failed" => format!("Find out why {who} failed"),
        "blocked" => format!("Unblock {who}: it is waiting for your input"),
        "question" | "waiting on you" | CHIP_ASKED_YOU => {
            format!("Answer the question from {who}")
        }
        CHIP_BLOCKED_ON_YOU => match &item.reply {
            Some(Reply::Blocked(need)) => {
                format!("Unblock {who}: {}", lower_first(&reply::unblock_move(need)))
            }
            _ => format!("Unblock {who}: it is waiting on you"),
        },
        "ambiguous runtime" => format!("Sort out which terminal {who} is using"),
        "command failed" => format!("Resend your last message to {who}"),
        "ready for review" => match agent_note(item) {
            Some(result) => format!("Review the result from {who}: {}", lower_first(&result)),
            None if item.activity_known => {
                format!("Review {who}: finished {} ago", duration(item.age_ms))
            }
            None => format!("Review {who}: it finished a turn"),
        },
        _ => return None,
    })
}

/// A blocker from an orchestrator's report, as a to-do naming the agent: a
/// question is answered, anything else is unblocked.
pub(crate) fn blocker_todo(who: &str, blocker: &str) -> String {
    if blocker.trim_end().ends_with('?') {
        format!("Answer {who}: {}", lower_first(blocker))
    } else {
        format!("Unblock {who}: {blocker}")
    }
}

/// The fallback now to-do of a Needs-you card no chip maps to.
pub(crate) fn check_todo(who: &str, note: &str) -> String {
    if note.is_empty() {
        format!("Check on {who}")
    } else {
        format!("Check on {who}: {note}")
    }
}

/// A finished-work follow-up (receipt blocker) naming the agent.
pub(crate) fn follow_up_todo(who: &str, blocker: &str) -> String {
    format!("Follow up on {who}: {blocker}")
}

/// The later to-do of a card that can be closed (or is probably finished).
pub(crate) fn close_todo(item: &Item, who: &str) -> String {
    if item.kind == ItemKind::DeadAggregate {
        return format!("Clean up {}", item.title);
    }
    if item.section == Some(Section::LikelyDone) {
        return if item.reply == Some(Reply::Done) {
            format!("Close {who}: it says it is finished")
        } else {
            format!("Check {who} is finished, then close it")
        };
    }
    if let Some(result) = agent_note(item) {
        format!("Close {who} (done: {})", lower_first(&result))
    } else if has(item, "reported nothing next") {
        format!("Close {who}: it says it is done")
    } else {
        format!("Close {who}: it is finished")
    }
}

/// The later to-do of a terminal agent Yard does not track.
pub(crate) fn loose_todo(who: &str) -> String {
    format!("Decide whether to keep {who} (outside Yard)")
}

/// One to-do for an agent's own unfinished steps (never one row per step:
/// the steps are the agent's work, listed on its card, not the user's).
/// `quiet`: the agent went quiet, so closing it is the other choice.
pub(crate) fn steps_todo(who: &str, steps: &[String], quiet: bool) -> Option<String> {
    let steps = steps
        .iter()
        .map(|step| step.trim().trim_end_matches('.'))
        .filter(|step| !step.is_empty())
        .collect::<Vec<_>>();
    let count = steps.len();
    let left = format!("{count} step{} left", if count == 1 { "" } else { "s" });
    match (steps.first(), quiet) {
        (None, _) => None,
        (Some(_), true) => Some(format!("Ask {who} to continue ({left}) or close it")),
        (Some(step), false) if count == 1 => {
            Some(format!("Ask {who} to continue: {}", lower_first(step)))
        }
        (Some(_), false) => Some(format!("Ask {who} to continue ({left})")),
    }
}

/// `text` with its first letter lower-cased for use after a colon in the
/// middle of a sentence ("Use v2?" -> "use v2?"), unless the first word
/// looks like a name or an acronym ("PR", "API", "Stripe's").
pub(crate) fn lower_first(text: &str) -> String {
    let text = text.trim();
    let first_word = text.split_whitespace().next().unwrap_or_default();
    let mut chars = first_word.chars();
    let Some(first) = chars.next() else {
        return text.to_owned();
    };
    let rest_lower = chars.clone().all(|ch| !ch.is_uppercase());
    let is_name = first_word.contains('\'') || first_word.contains('\u{2019}');
    if !first.is_uppercase() || !rest_lower || first_word.len() < 2 || is_name {
        return text.to_owned();
    }
    let mut out = first.to_lowercase().collect::<String>();
    out.push_str(&text[first.len_utf8()..]);
    out
}

/// The agent's own latest line when it says more than its task and is not
/// one of the board's own status phrases: what it is doing, or what it
/// finished. Trailing full stops and semicolons are tidied so it reads as a
/// fragment.
pub(crate) fn agent_note(item: &Item) -> Option<String> {
    let note = item.status_line.lines().next().unwrap_or_default().trim();
    let note = note.trim_end_matches(['.', ';']).trim();
    if note.is_empty()
        || note.eq_ignore_ascii_case(item.objective.trim().trim_end_matches('.'))
        || empty_note(note)
    {
        return None;
    }
    if board_phrase(note) {
        return None;
    }
    Some(note.replace("; ", ", ").replace(';', ","))
}

/// Placeholder notes that say nothing ("N/A", "none", "-", "TBD").
fn empty_note(note: &str) -> bool {
    let bare = note
        .trim()
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_ascii_lowercase();
    bare.is_empty()
        || matches!(
            bare.as_str(),
            "n/a"
                | "n a"
                | "na"
                | "none"
                | "nothing"
                | "null"
                | "tbd"
                | "todo"
                | "unknown"
                | "no"
                | "nil"
        )
}

/// Status lines the board writes itself (never the agent's own words).
fn board_phrase(text: &str) -> bool {
    const HERDR: [ObservedStatus; 5] = [
        ObservedStatus::Working,
        ObservedStatus::Blocked,
        ObservedStatus::Done,
        ObservedStatus::Idle,
        ObservedStatus::Unknown,
    ];
    const AVAILABILITY: [WorkerAvailability; 9] = [
        WorkerAvailability::YardOrchestrator,
        WorkerAvailability::CoordinationNode,
        WorkerAvailability::Orchestrator,
        WorkerAvailability::Assigned,
        WorkerAvailability::UnassignedLive,
        WorkerAvailability::Resumable,
        WorkerAvailability::Unavailable,
        WorkerAvailability::Ambiguous,
        WorkerAvailability::Ended,
    ];
    text == herdr_phrase(ObservedStatus::Working, false)
        || HERDR
            .iter()
            .any(|status| text == herdr_phrase(*status, true))
        || AVAILABILITY
            .iter()
            .any(|availability| text == availability_phrase(*availability))
}

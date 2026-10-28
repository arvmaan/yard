//! Card titles a person recognises. In order: the user's own display name;
//! the conversation's title (the terminal title Claude / Codex set, or the
//! title in their session files); the Herdr agent / tab name when someone
//! chose it; the first clause of the task; an AI title (see `pass`); else
//! "Untitled <provider> chat in <place>". Never a profile name
//! ("Generalist"), a session or worker id, or a bare provider name.

use std::collections::HashSet;

const MAX_TITLE_CHARS: usize = 80;
const MAX_CLAUSE_WORDS: usize = 9;

/// Where a title came from (shown under Details).
pub(crate) const FROM_NAME: &str = "your name for it";
pub(crate) const FROM_TERMINAL: &str = "terminal title";
pub(crate) const FROM_CONVERSATION: &str = "conversation title";
pub(crate) const FROM_AGENT: &str = "agent name";
pub(crate) const FROM_TAB: &str = "tab name";
pub(crate) const FROM_TASK: &str = "task";
pub(crate) const FROM_ROLE: &str = "role";
pub(crate) const FROM_GOAL: &str = "what it is on now";
pub(crate) const FROM_AI: &str = "AI title";
pub(crate) const UNTITLED: &str = "untitled";

/// Generic words that never make a title on their own.
const GENERIC: &[&str] = &[
    "agent",
    "assistant",
    "bash",
    "chat",
    "claude",
    "claude code",
    "codex",
    "generalist",
    "investigator",
    "new tab",
    "orchestrator",
    "reviewer",
    "shell",
    "terminal",
    "untitled",
    "worker",
    "workstream",
    "yard",
    "zsh",
];

/// Words a cut title must not end on.
const DANGLING: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "into", "of", "on", "or", "so",
    "that", "the", "to", "we", "with", "how", "can", "our", "my", "your",
];

/// Leading phrases of a task prompt that say nothing about the work.
const FILLERS: &[&str] = &[
    "i want you to ",
    "i want to ",
    "i need you to ",
    "i would like you to ",
    "i'd like you to ",
    "we want to ",
    "we need to ",
    "can you please ",
    "can you ",
    "can we ",
    "let's ",
    "we should ",
    "could you please ",
    "could you ",
    "please ",
    "you will ",
    "your task is to ",
    "your job is to ",
    "hey, ",
    "hi, ",
    "ok, ",
    "okay, ",
    "so ",
];

/// Whether `name` reads as a real title: not empty, not a profile or role
/// name (`profiles` are lower-cased, with "-2"-style copies matching too),
/// not a provider name, not an id ("yard-a1b2c3d-0123…", a UUID, a hex
/// run, `term_…`), not a numbered default tab ("1") and not a Yard
/// bookkeeping label ("Yard replacement […]", "… orchestrator").
pub(crate) fn meaningful(name: &str, profiles: &HashSet<String>) -> bool {
    let name = name.trim();
    let lower = name.to_lowercase();
    if name.chars().count() < 3 || name.chars().count() > 120 {
        return false;
    }
    let base = lower
        .rsplit_once(['-', ' '])
        .filter(|(_, suffix)| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
        .map_or(lower.as_str(), |(base, _)| base);
    if GENERIC.contains(&lower.as_str())
        || GENERIC.contains(&base)
        || profiles.contains(&lower)
        || profiles.contains(base)
    {
        return false;
    }
    if lower.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if lower.starts_with("yard replacement")
        || lower.ends_with(" orchestrator")
        || lower.starts_with("yard-")
        || lower.starts_with("coord-")
        || lower.starts_with("term_")
        || lower.starts_with("provider-child:")
    {
        return false;
    }
    !looks_like_id(&lower)
}

/// A run of 8+ hex characters mixing digits and letters (ids, hashes,
/// UUID parts), 16+ digits, or no letters at all. A ticket or CR number
/// ("PR-1234567890") is not an id.
fn looks_like_id(text: &str) -> bool {
    for run in text.split(|ch: char| !ch.is_ascii_hexdigit()) {
        let digits = run.bytes().any(|byte| byte.is_ascii_digit());
        let letters = run.bytes().any(|byte| byte.is_ascii_alphabetic());
        if (run.len() >= 8 && digits && letters) || run.len() >= 16 {
            return true;
        }
    }
    !text.chars().any(char::is_alphabetic)
}

/// A slug ("docs-beta-review") as words ("docs beta review"); anything with
/// spaces is kept as written.
pub(crate) fn humanize(name: &str) -> String {
    let name = name.trim();
    if name.contains(' ') {
        return name.to_owned();
    }
    name.replace(['-', '_'], " ")
}

/// Short technical words written in capitals in a title.
const ACRONYMS: &[&str] = &[
    "api", "aws", "cdk", "ci", "cli", "cr", "crs", "dns", "iam", "kms", "llm", "poc", "pr", "s3",
    "sdk", "sso", "tls", "ui", "url", "vpc",
];

/// A title's lowercase words in the capitals the conversation uses for
/// them ("orbit review" -> "ORBIT review" when the chat writes "ORBIT").
pub(crate) fn capitals_like(title: &str, context: &str) -> String {
    // Runs of capitals, also inside CamelCase names ("ABCConfigService"
    // gives "ABC": the capital that starts the next word is not part of it).
    let mut upper = HashSet::new();
    for word in context.split(|ch: char| !ch.is_alphanumeric()) {
        let chars = word.chars().collect::<Vec<_>>();
        let mut start = 0;
        while start < chars.len() {
            if !chars[start].is_ascii_uppercase() {
                start += 1;
                continue;
            }
            let mut end = start;
            while end < chars.len() && chars[end].is_ascii_uppercase() {
                end += 1;
            }
            let run_end = if end < chars.len() && chars[end].is_ascii_lowercase() {
                end - 1
            } else {
                end
            };
            if (2..=6).contains(&(run_end - start)) {
                upper.insert(chars[start..run_end].iter().collect::<String>());
            }
            start = end.max(start + 1);
        }
    }
    title
        .split(' ')
        .map(|word| {
            let capital = word.to_ascii_uppercase();
            if word.chars().all(|ch| ch.is_ascii_lowercase()) && upper.contains(&capital) {
                capital
            } else if let Some(rest) = word.strip_prefix(|ch: char| ch.is_ascii_uppercase())
                && rest.chars().all(|ch| ch.is_ascii_lowercase())
                && upper.contains(&capital)
            {
                // The first word was upper-cased only for the sentence start.
                capital
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// An all-lowercase agent name as a title: known acronyms in capitals,
/// glued numbers apart ("phase2" -> "phase 2"), first letter upper-cased
/// ("api cdk review" -> "API CDK review"). Names with any capital
/// are the user's own casing and stay.
pub(crate) fn case_words(name: &str) -> String {
    if name.chars().any(char::is_uppercase) {
        return name.to_owned();
    }
    let words = name
        .split_whitespace()
        .map(|word| {
            if ACRONYMS.contains(&word) {
                return word.to_uppercase();
            }
            let letters = word.trim_end_matches(|ch: char| ch.is_ascii_digit());
            if letters.len() >= 3 && letters.len() < word.len() {
                format!("{letters} {}", &word[letters.len()..])
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        format!("{}{}", first.to_uppercase(), chars.as_str())
    })
}

/// The first clause of a task prompt as a short title: filler phrases and
/// links dropped, cut at the first sentence or clause break, at most
/// nine words (never ending on a joining word), first letter upper-cased. `None` when fewer than two words
/// are left.
pub(crate) fn objective_clause(objective: &str) -> Option<String> {
    let line = objective
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let words = line
        .split_whitespace()
        .filter(|word| !word.contains("://"))
        .collect::<Vec<_>>()
        .join(" ");
    let text = strip_fillers(&words);
    let cut = text
        .find(['.', '?', '!', ';', ':', '(', '\n'])
        .map_or(text, |end| &text[..end]);
    let cut = [", and ", ", but ", ", so ", " so that ", ", which "]
        .iter()
        .filter_map(|marker| cut.to_lowercase().find(marker))
        .min()
        .map_or(cut, |end| &cut[..end]);
    let words = cut
        .split_whitespace()
        .take(MAX_CLAUSE_WORDS)
        .collect::<Vec<_>>();
    if words.len() < 2 {
        return None;
    }
    let mut words = words;
    while words.len() > 2
        && words.last().is_some_and(|word| {
            DANGLING.contains(&word.to_lowercase().trim_end_matches([',', ';']))
        })
    {
        words.pop();
    }
    let clause = words.join(" ").trim_end_matches([',', '-', ' ']).to_owned();
    let mut chars = clause.chars();
    let first = chars.next()?;
    Some(crate::redact::clean_text(
        &format!("{}{}", first.to_uppercase(), chars.as_str()),
        MAX_TITLE_CHARS,
    ))
}

/// A prompt without its opening pleasantries and set-up clauses ("I want
/// you to", "Can you", "Based on this design:", "I will provide you
/// context but"), links dropped.
pub(crate) fn strip_fillers(prompt: &str) -> &str {
    let mut text = prompt.trim();
    loop {
        let lower = text.to_lowercase();
        if let Some(filler) = FILLERS.iter().find(|filler| lower.starts_with(*filler)) {
            text = text[filler.len()..].trim_start();
            continue;
        }
        // "Based on this design: …", "I will provide you context but …":
        // keep what follows the set-up.
        let setup = [
            "based on ",
            "following ",
            "given ",
            "i will provide ",
            "i'll give you ",
        ]
        .iter()
        .any(|opener| lower.starts_with(opener));
        if setup {
            let breaks = [
                ", but ",
                " but ",
                ": ",
                ", ",
                " can we ",
                " can you ",
                " i want you to ",
                " please ",
            ];
            if let Some((at, len)) = breaks
                .iter()
                .filter_map(|mark| lower.find(mark).map(|at| (at, mark.len())))
                .min()
            {
                text = text[at + len..].trim_start();
                continue;
            }
        }
        return text;
    }
}

/// "Claude" / "Codex" / "agent" for the untitled fallback.
pub(crate) fn provider_word(provider: Option<&str>) -> &'static str {
    match provider.map(str::to_ascii_lowercase).as_deref() {
        Some("claude") => "Claude",
        Some("codex") => "Codex",
        _ => "agent",
    }
}

/// "Untitled Claude chat in web-app" (or "Untitled agent chat").
pub(crate) fn untitled(provider: Option<&str>, place: Option<&str>) -> String {
    let provider = provider_word(provider);
    match place.map(str::trim).filter(|place| !place.is_empty()) {
        Some(place) => crate::redact::clean_text(
            &format!("Untitled {provider} chat in {place}"),
            MAX_TITLE_CHARS,
        ),
        None => format!("Untitled {provider} chat"),
    }
}

/// Everything a title can be chosen from.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TitleSources<'a> {
    pub display_name: Option<&'a str>,
    pub terminal_title: Option<&'a str>,
    pub conversation_title: Option<&'a str>,
    pub agent_name: Option<&'a str>,
    pub tab_label: Option<&'a str>,
    pub objective: &'a str,
    /// A fixed role name (Yard orchestrator, Workstream, "<project>
    /// orchestrator"), used before the task and the fallback.
    pub role: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub place: Option<&'a str>,
}

/// A squashed CamelCase name as words, for display only (`ReleaseNotes`
/// -> "Release Notes", `APIKeyRotation` -> "API Key Rotation"); a name
/// with spaces, or with nothing to split, is kept as written.
pub(crate) fn split_camel(name: &str) -> String {
    let name = name.trim();
    if name.contains(char::is_whitespace) {
        return name.to_owned();
    }
    let chars = name.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(name.len() + 4);
    for (index, &ch) in chars.iter().enumerate() {
        if index > 0 {
            let prev = chars[index - 1];
            let next = chars.get(index + 1).copied();
            // Only a clear word start splits: a capital followed by a small
            // letter ("FollowUp", "CAKey", "GateXSetup" -> "GateX Setup").
            // Runs of capitals and glued tokens ("PlanXCA", "WEBSDK")
            // stay exactly as typed.
            let boundary = (ch.is_uppercase()
                && prev.is_alphanumeric()
                && next.is_some_and(char::is_lowercase))
                || (ch == '(' && prev.is_alphanumeric());
            if boundary {
                out.push(' ');
            }
        }
        out.push(ch);
    }
    out
}

/// A raw review number as words: "CR-1234 review comments" ->
/// "Review comments on CR 1234"; any other "CR-123" reads "CR 123".
pub(crate) fn tidy_ids(title: &str) -> String {
    let title = title.trim();
    for prefix in ["CR-", "PR-"] {
        if let Some(rest) = title.strip_prefix(prefix) {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            let tail = rest[digits..].trim();
            if digits > 0 && tail.split_whitespace().count() >= 1 {
                let number = &rest[..digits];
                let mut chars = tail.chars();
                if let Some(first) = chars.next() {
                    return format!(
                        "{}{} on {} {number}",
                        first.to_uppercase(),
                        chars.as_str(),
                        &prefix[..2]
                    );
                }
            }
        }
    }
    title.replace("CR-", "CR ").replace("PR-", "PR ")
}

/// The title and where it came from.
pub(crate) fn choose(
    sources: &TitleSources<'_>,
    profiles: &HashSet<String>,
) -> (String, &'static str) {
    let clean = |text: &str| crate::redact::clean_text(text.trim(), MAX_TITLE_CHARS);
    if let Some(name) = sources
        .display_name
        .filter(|name| !name.trim().is_empty() && meaningful(name, profiles))
    {
        // The stored name is kept; only its display is split into words.
        return (clean(&split_camel(name)), FROM_NAME);
    }
    let mut topic = None;
    for (text, source) in [
        (sources.terminal_title, FROM_TERMINAL),
        (sources.conversation_title, FROM_CONVERSATION),
        (sources.agent_name, FROM_AGENT),
        (sources.tab_label, FROM_TAB),
    ] {
        if let Some(text) = text.filter(|text| meaningful(text, profiles)) {
            let words = humanize(text);
            let words = if matches!(source, FROM_AGENT | FROM_TAB) {
                case_words(&words)
            } else {
                words
            };
            topic = Some((clean(&tidy_ids(&words)), source));
            break;
        }
    }
    if topic.is_none() {
        topic = objective_clause(sources.objective).map(|clause| (clause, FROM_TASK));
    }
    // A role is a qualifier, never the whole title: "Workstream: <topic>",
    // else "Untitled workstream in <place>". A role that already names
    // something ("Yard orchestrator", "<project> orchestrator") stands.
    if let Some(role) = sources.role {
        let generic = GENERIC.contains(&role.trim().to_lowercase().as_str());
        if !generic {
            return (clean(role), FROM_ROLE);
        }
        if let Some((topic, _)) = topic {
            return (clean(&format!("{role}: {topic}")), FROM_ROLE);
        }
        let what = role.trim().to_lowercase();
        return sources
            .place
            .map(str::trim)
            .filter(|place| !place.is_empty())
            .map_or_else(
                || (format!("Untitled {what}"), UNTITLED),
                |place| (clean(&format!("Untitled {what} in {place}")), UNTITLED),
            );
    }
    if let Some(topic) = topic {
        return topic;
    }
    (untitled(sources.provider, sources.place), UNTITLED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles() -> HashSet<String> {
        ["generalist", "investigator", "reviewer"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn ids_profiles_and_providers_are_never_titles() {
        let profiles = profiles();
        for bad in [
            "yard-a1b2c3d-0123456789abcdef",
            "yard-0f1e2d3c4b5a69788796a5b4",
            "coord-0123456789abcdef0123456789",
            "Generalist",
            "Generalist-2",
            "Reviewer",
            "claude",
            "Codex",
            "Worker",
            "1",
            "Yard replacement [00000000-1111-4222-8333-444444444444]",
            "web-app orchestrator",
            "01a0b194-1111-7222-8333-444455556666",
            "term_0123abcd9",
            "Session 003caa5ac92d81bd",
            "",
            "ab",
        ] {
            assert!(!meaningful(bad, &profiles), "{bad}");
        }
        for good in [
            "Review the beta release",
            "docs-beta-review",
            "layoutnotes",
            "Upload retry fix",
            "FollowUpTasks",
            "PR-1234567890 review comments",
        ] {
            assert!(meaningful(good, &profiles), "{good}");
        }
    }

    #[test]
    fn task_prompts_become_short_clauses() {
        assert_eq!(
            objective_clause(
                "I want you to research and understand how the cache works, and then plan"
            )
            .as_deref(),
            Some("Research and understand how the cache works")
        );
        assert_eq!(
            objective_clause(
                "Can you investigate the flaky upload tests seen here, https://x.example/a"
            )
            .as_deref(),
            Some("Investigate the flaky upload tests seen here")
        );
        assert_eq!(
            objective_clause("You will review the work the orchestrator is doing. Be thorough.")
                .as_deref(),
            Some("Review the work the orchestrator is doing")
        );
        assert_eq!(
            objective_clause("Based on this design: https://x.example/d plan the key rotation rollout for the new CA").as_deref(),
            Some("Plan the key rotation rollout for the new CA")
        );
        assert_eq!(
            objective_clause(
                "I will provide you context but I want you to figure out the staging infra plan"
            )
            .as_deref(),
            Some("Figure out the staging infra plan")
        );
        assert_eq!(
            objective_clause(
                "Based on this thread can we investigate and plan how to get billing on SDK"
            )
            .as_deref(),
            Some("Investigate and plan how to get billing on SDK")
        );
        assert_eq!(objective_clause("Go."), None);
        assert_eq!(objective_clause(""), None);
    }

    #[test]
    fn the_chain_prefers_names_then_titles_then_task_then_fallback() {
        let profiles = profiles();
        let mut sources = TitleSources {
            display_name: Some("Generalist-2"),
            agent_name: Some("yard-a1b2c3d-0123456789abcdef"),
            tab_label: Some("Generalist"),
            objective: "",
            provider: Some("claude"),
            place: Some("web-app"),
            ..TitleSources::default()
        };
        assert_eq!(
            choose(&sources, &profiles),
            ("Untitled Claude chat in web-app".to_owned(), UNTITLED)
        );
        sources.objective = "Please fix the flaky upload test";
        assert_eq!(choose(&sources, &profiles).1, FROM_TASK);
        sources.tab_label = Some("Upload fixes");
        assert_eq!(
            choose(&sources, &profiles),
            ("Upload fixes".to_owned(), FROM_TAB)
        );
        sources.conversation_title = Some("Flaky upload test");
        assert_eq!(choose(&sources, &profiles).1, FROM_CONVERSATION);
        sources.display_name = Some("Payments");
        assert_eq!(
            choose(&sources, &profiles),
            ("Payments".to_owned(), FROM_NAME)
        );
        assert_eq!(untitled(None, None), "Untitled agent chat");
    }
}

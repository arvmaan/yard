//! What the agent's latest message *is*: a blocker only the user can clear,
//! a question, work still left, a completion, a bare acknowledgement, or
//! a finding. The board places quiet cards by it (a blocker or a question
//! waits on the user; a completion is probably done; "three CRs left" is
//! not) and words Status and Your move from it, so a card never says
//! "not finished" next to "nothing is left", or "probably finished" next
//! to "three CRs left".

/// What only the user can do to unblock the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Need {
    /// Expired or missing credentials ("AWS authentication failed").
    Credentials { aws: bool },
    /// Access the agent was refused ("permission denied", "403").
    Access,
    /// An approval the agent waits for.
    Approval,
    /// Someone else must act ("only `Ops` can use it").
    Contact(String),
    /// Input or a decision from the user.
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    Blocked(Need),
    Question,
    /// Work still open ("three CRs left"): the phrase that says so.
    Remaining(String),
    Done,
    /// Says nothing about the work ("your message came through").
    Ack,
    /// Anything else: a result or a warning, not a completion.
    Finding,
}

impl Reply {
    /// Waits on the user (a blocker or a question).
    pub(crate) const fn waits_on_user(&self) -> bool {
        matches!(self, Self::Blocked(_) | Self::Question)
    }
}

/// Acknowledgement-only sentences: they never become a Status snippet.
const ACKS: &[&str] = &[
    "your message came through",
    "message received",
    "got it",
    "understood",
    "thanks",
    "thank you",
    "noted",
    "will do",
    "on it",
    "sounds good",
    "no problem",
    "this task was already finished",
    "this task is already finished",
    "this was already done",
];

/// Whether a sentence only acknowledges (or repeats that it is done).
pub(crate) fn ack_sentence(sentence: &str) -> bool {
    let bare = bare(sentence);
    ACKS.iter()
        .any(|ack| bare == *ack || bare.starts_with(&format!("{ack} ")))
        && bare.split_whitespace().count() <= 12
}

fn bare(text: &str) -> String {
    text.trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
        .replace(['\u{2019}', '`', '*'], "'")
        .replace('\'', "")
}

/// Sentences of the message's opening (markdown table rows and empty lines
/// skipped), at most `max`.
pub(crate) fn sentences(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('|') {
            continue;
        }
        let mut rest = line;
        while !rest.trim().is_empty() && out.len() < max {
            let end = rest
                .char_indices()
                .find(|(at, ch)| {
                    matches!(ch, '.' | '!' | '?' | '\u{b7}')
                        && rest[at + ch.len_utf8()..]
                            .chars()
                            .next()
                            .is_none_or(char::is_whitespace)
                })
                .map_or(rest.len(), |(at, ch)| at + ch.len_utf8());
            let sentence = rest[..end].trim();
            if sentence.chars().any(char::is_alphanumeric) {
                out.push(sentence.to_owned());
            }
            rest = &rest[end..];
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

const BLOCKERS: &[&str] = &[
    "authentication failed",
    "auth failed",
    "credentials expired",
    "credentials have expired",
    "expired credentials",
    "token included in the request is invalid",
    "security token",
    "sso session",
    "login required",
    "not logged in",
    "permission denied",
    "access denied",
    "not authorized",
    "needs your approval",
    "need your approval",
    "waiting for your approval",
    "waiting for approval",
    "waiting on you",
    "waiting for you",
    "waiting for your",
    "waiting on your",
    "need your input",
    "needs your input",
    "need you to",
    "needs you to",
    "blocked on",
    "blocked by",
    "i'm blocked",
    "i am blocked",
    "can't proceed",
    "cannot proceed",
    "can't continue",
    "cannot continue",
];

fn need_of(lower: &str) -> Need {
    let any = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if any(&[
        "authentication",
        "credential",
        "security token",
        "sso",
        "login required",
        "not logged in",
        "auth failed",
    ]) {
        Need::Credentials {
            aws: lower.contains("aws") || lower.contains("security token"),
        }
    } else if any(&[
        "permission",
        "access denied",
        "not authorized",
        "unauthorized",
        "forbidden",
    ]) {
        Need::Access
    } else if lower.contains("approv") {
        Need::Approval
    } else {
        Need::Input
    }
}

/// "only `Ops` can …" names who must act: a decision for the user.
fn only_who(sentence: &str) -> Option<String> {
    let words = sentence.split_whitespace().collect::<Vec<_>>();
    let at = words
        .iter()
        .position(|word| word.eq_ignore_ascii_case("only"))?;
    let who = words.get(at + 1)?;
    let after = words.get(at + 2).map(|word| word.to_lowercase())?;
    let name = who.trim_matches(|ch: char| !ch.is_alphanumeric());
    let capital = name.chars().next().is_some_and(char::is_uppercase);
    (capital
        && name.len() >= 2
        && ["can", "may", "has", "have", "is", "owns", "controls"].contains(&after.as_str()))
    .then(|| name.to_owned())
}

/// Words that open a past-tense report of what the agent did.
const DID: &[&str] = &[
    "wrote", "made", "ran", "built", "sent", "put", "set", "rebuilt", "redid", "kept", "left",
];

fn reports_done(sentence: &str) -> bool {
    let lower = bare(sentence);
    let lower = lower
        .trim_start_matches("ive ")
        .trim_start_matches("i have ")
        .trim_start_matches("we have ")
        .trim_start_matches("weve ")
        .trim_start_matches("i ")
        .trim_start_matches("we ");
    if [
        "no required",
        "no further",
        "nothing left",
        "nothing else",
        "nothing remains",
        "no changes are needed",
        "no changes needed",
        "no more changes",
    ]
    .iter()
    .any(|lead| lower.starts_with(lead))
        || (lower.starts_with("no ") && (lower.contains(" left") || lower.contains(" remain")))
    {
        return true;
    }
    if [
        "already finished",
        "already done",
        "already complete",
        "all green",
        "all pass",
        "all tests pass",
        "is complete",
        "are complete",
        "is now complete",
        "is done",
        "is finished",
        "are all merged",
        "has merged",
        "have merged",
        "is merged",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        && !lower.contains("so far")
        && !lower.contains(" until ")
    {
        return true;
    }
    let first = lower.split_whitespace().next().unwrap_or_default();
    DID.contains(&first)
        || (first.len() > 4
            && first.ends_with("ed")
            && !["need", "needed", "proceed", "exceed", "succeed", "speed"].contains(&first))
}

/// "three CRs left", "2 steps remaining": the phrase, when the message says
/// work is still open.
fn remaining(sentence: &str) -> Option<String> {
    let lower = bare(sentence);
    if lower.starts_with("no ") || lower.starts_with("nothing") {
        return None;
    }
    let words = sentence
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .collect::<Vec<_>>();
    let at = words.iter().position(|word| {
        let word = word.to_lowercase();
        word == "left" || word == "remaining" || word == "remain" || word == "outstanding"
    })?;
    if at < 2 {
        return None;
    }
    let count = words[at - 2].to_lowercase();
    let number = count.parse::<u32>().is_ok()
        || [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "a",
            "some", "several", "few",
        ]
        .contains(&count.as_str());
    number.then(|| format!("{} {} left", words[at - 2].to_lowercase(), words[at - 1]))
}

/// Classify the agent's latest message from its opening sentences (and its
/// closing question, if any).
pub(crate) fn classify(text: Option<&str>) -> Option<Reply> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }
    let all = sentences(text, 8);
    // Headings ("## Summary") and lead-ins ("Done.") are not the message's
    // point; a bare "Done." still says it is complete.
    let words = |sentence: &str| {
        sentence
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count()
    };
    let done_lead = all.iter().take(2).any(|sentence| {
        words(sentence) < 3
            && ["done", "all done", "complete", "completed", "finished"]
                .contains(&bare(sentence).as_str())
    });
    let useful = all
        .iter()
        .filter(|sentence| {
            !ack_sentence(sentence)
                && !sentence.trim_start().starts_with('#')
                && words(sentence) >= 3
        })
        .collect::<Vec<_>>();
    let Some(lead) = useful.first() else {
        return Some(Reply::Ack);
    };
    let lower = lead.to_lowercase().replace('\u{2019}', "'");
    if BLOCKERS.iter().any(|phrase| lower.contains(phrase)) {
        return Some(Reply::Blocked(need_of(&lower)));
    }
    if let Some(who) = only_who(lead) {
        return Some(Reply::Blocked(Need::Contact(who)));
    }
    // A closing question (the message ends asking something).
    if text.trim_end().ends_with('?') || lead.trim_end().ends_with('?') {
        return Some(Reply::Question);
    }
    if let Some(left) = useful
        .iter()
        .take(2)
        .find_map(|sentence| remaining(sentence))
    {
        return Some(Reply::Remaining(left));
    }
    if done_lead || reports_done(lead) {
        return Some(Reply::Done);
    }
    Some(Reply::Finding)
}

/// The question the message ends with (its last sentence ending in "?"),
/// cut to at most 14 words, for "Asks: …".
pub(crate) fn question_text(text: Option<&str>) -> Option<String> {
    let asked = sentences(text?, 12)
        .into_iter()
        .rev()
        .find(|sentence| sentence.trim_end().ends_with('?'))?;
    let words = asked
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| matches!(ch, '*' | '`' | '_')))
        .filter(|word| !word.is_empty() && !word.contains("://"))
        .collect::<Vec<_>>();
    let short = if words.len() > 14 {
        format!(
            "{}\u{2026}",
            words[..14].join(" ").trim_end_matches([',', ';', ':'])
        )
    } else {
        words.join(" ")
    };
    let short = crate::redact::clean_text(&short, 160);
    (short.split_whitespace().count() >= 2 && !short.contains("[path]")).then_some(short)
}

/// Your move for a blocker: what the user does to unblock it.
pub(crate) fn unblock_move(need: &Need) -> String {
    match need {
        Need::Credentials { aws: true } => {
            "Refresh your AWS credentials, then tell it to continue.".to_owned()
        }
        Need::Credentials { aws: false } => "Sign in again, then tell it to continue.".to_owned(),
        Need::Access => "Grant the access it was refused, then tell it to continue.".to_owned(),
        Need::Approval => "Approve or decline what it asks, then tell it to continue.".to_owned(),
        Need::Contact(who) => format!("Decide who contacts {who}, then tell it how to go on."),
        Need::Input => "Give it what it is waiting for, then tell it to continue.".to_owned(),
    }
}

/// A Status lead for a blocker ("Blocked: AWS authentication failed").
pub(crate) const fn blocked_lead(need: &Need) -> &'static str {
    match need {
        Need::Contact(_) => "Needs a decision",
        _ => "Blocked",
    }
}

/// The first `CR-123…` or `CR 123…` id the message names, for a move that
/// names its object.
pub(crate) fn cr_id(text: Option<&str>) -> Option<String> {
    let text = text?;
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(found) = text[at..].find("CR") {
        let start = at + found;
        let mut index = start + 2;
        if bytes
            .get(index)
            .is_some_and(|byte| *byte == b'-' || *byte == b' ')
        {
            index += 1;
        }
        let digits = text[index..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        if before_ok && digits >= 6 {
            return Some(format!("CR-{}", &text[index..index + digits]));
        }
        at = start + 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_latest_messages() {
        let blocked = classify(Some(
            "AWS authentication failed \u{b7} refresh your AWS credentials and retry",
        ));
        assert_eq!(
            blocked,
            Some(Reply::Blocked(Need::Credentials { aws: true }))
        );
        assert_eq!(
            classify(Some("I am not logged in, so the deploy cannot start.")),
            Some(Reply::Blocked(Need::Credentials { aws: false }))
        );
        assert_eq!(
            classify(Some("Login required before I can fetch the release notes.")),
            Some(Reply::Blocked(Need::Credentials { aws: false }))
        );
        assert_eq!(
            classify(Some("Permission denied writing to the bucket.")),
            Some(Reply::Blocked(Need::Access))
        );
        assert_eq!(
            classify(Some(
                "The publishing root is a KMS signing key that only Ops can use. Its signature…"
            )),
            Some(Reply::Blocked(Need::Contact("Ops".to_owned())))
        );
        assert_eq!(
            classify(Some(
                "No required SDK changes are left. The client is merged."
            )),
            Some(Reply::Done)
        );
        assert_eq!(
            classify(Some("I've updated the design doc to version 195: link.")),
            Some(Reply::Done)
        );
        assert_eq!(
            classify(Some("I posted four draft replies on CR-123456789.")),
            Some(Reply::Done)
        );
        assert_eq!(
            classify(Some("You have three CRs left, and all three are drafts.")),
            Some(Reply::Remaining("three CRs left".to_owned()))
        );
        assert_eq!(
            classify(Some(
                "Gamma is still running two tasks on a September image."
            )),
            Some(Reply::Finding)
        );
        assert_eq!(
            classify(Some(
                "Your message came through as just \"|\". Which did you mean: 1 or 2?"
            )),
            Some(Reply::Question)
        );
        assert_eq!(classify(Some("Got it.")), Some(Reply::Ack));
        assert_eq!(
            classify(Some(
                "## Summary\nI've drafted the plan with 4 fixes.\n- step one"
            )),
            Some(Reply::Done)
        );
        assert_eq!(
            classify(Some("DONE. The facts file is written: 3 sections.")),
            Some(Reply::Done)
        );
        assert_eq!(
            question_text(Some("I traced the flake. Want me to fix it?")),
            Some("Want me to fix it?".to_owned())
        );
        assert_eq!(
            classify(Some("The checks are green so far: nothing changed.")),
            Some(Reply::Finding)
        );
        assert_eq!(classify(None), None);
    }

    #[test]
    fn moves_and_ids() {
        assert_eq!(
            unblock_move(&Need::Credentials { aws: true }),
            "Refresh your AWS credentials, then tell it to continue."
        );
        assert!(unblock_move(&Need::Contact("Ops".to_owned())).contains("Ops"));
        assert_eq!(
            cr_id(Some("posted replies on CR-123456789, to rev 1")),
            Some("CR-123456789".to_owned())
        );
        assert_eq!(cr_id(Some("the CR is ready")), None);
        assert!(ack_sentence("Your message came through as just \"|\"."));
        assert!(!ack_sentence("The fix is merged."));
    }
}

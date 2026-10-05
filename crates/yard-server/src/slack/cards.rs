//! Slack messages for inbound work: prompt cards with answer buttons,
//! collapsed cards, and outcome replies.
//!
//! Only the prompt itself (question, details, options — redacted, escaped,
//! capped at [`MAX_PROMPT_CHARS`]) is shown; standing-grant options are
//! listed as "not offered" and never get a button. Buttons carry only the
//! opaque action id as their value.

use serde_json::{Value, json};

use super::{
    actions::{ACTION_TTL, Refusal},
    blocks,
    message::{OutgoingMessage, clean, clean_prompt_line, since_token},
    prompt::{ChoicePrompt, MAX_PROMPT_CHARS, OptionRole, ParsedScreen, cap},
};

/// Slack caps button text at 75 characters.
const MAX_BUTTON_CHARS: usize = 60;
/// `action_id` prefix of Yard's prompt buttons.
pub const BUTTON_ACTION_PREFIX: &str = "yard_prompt_";

/// Button text: redacted, controls removed, capped; plain text (Slack does
/// not parse mentions there).
fn button_text(role: OptionRole, number: usize, label: &str) -> String {
    let text = match role {
        OptionRole::Allow => "Allow once".to_owned(),
        OptionRole::Deny => "Deny".to_owned(),
        OptionRole::Choice => {
            let label = clean_prompt_line(label)
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">");
            format!("{number}. {label}")
        }
    };
    if text.chars().count() > MAX_BUTTON_CHARS {
        let mut short = text.chars().take(MAX_BUTTON_CHARS - 1).collect::<String>();
        short.push('…');
        short
    } else {
        text
    }
}

fn buttons(choice: &ChoicePrompt, ids: &[String]) -> Value {
    let elements = choice
        .options
        .iter()
        .zip(ids)
        .enumerate()
        .map(|(position, (option, id))| {
            let mut button = json!({
                "type": "button",
                "action_id": format!("{BUTTON_ACTION_PREFIX}{position}"),
                "text": {
                    "type": "plain_text",
                    "text": button_text(option.role, option.index + 1, &option.label),
                    "emoji": false,
                },
                "value": id,
            });
            match option.role {
                OptionRole::Allow => button["style"] = json!("primary"),
                OptionRole::Deny => button["style"] = json!("danger"),
                OptionRole::Choice => {}
            }
            button
        })
        .collect::<Vec<_>>();
    json!({ "type": "actions", "block_id": "yard_prompt", "elements": elements })
}

/// What a prompt card shows besides the prompt.
#[derive(Debug, Clone, Copy)]
pub struct CardContext<'a> {
    /// "Open in Yard" target.
    pub ui_url: &'a str,
    /// The agent's Herdr pane, when known.
    pub pane: Option<&'a str>,
    pub at_unix_ms: u64,
}

fn links(ui_url: &str) -> Value {
    blocks::actions("yard_links", vec![blocks::open_in_yard(ui_url)])
}

fn footer(context: &CardContext<'_>, lead: &str) -> Value {
    let mut parts = vec![lead.to_owned()];
    if let Some(pane) = context.pane {
        parts.push(format!("pane `{}`", clean(pane, 40)));
    }
    parts.push(format!("updated {}", since_token(context.at_unix_ms)));
    blocks::context(&parts)
}

/// The card for what `title` (already cleaned) shows now. `ids` has one
/// action id per offered option of a choice prompt.
#[must_use]
pub fn prompt_card(
    title: &str,
    screen: &ParsedScreen,
    ids: &[String],
    context: &CardContext<'_>,
) -> OutgoingMessage {
    let minutes = ACTION_TTL.as_secs() / 60;
    let blocks = match screen {
        ParsedScreen::Choice(choice) => {
            let mut blocks = vec![
                blocks::header(&format!(":raised_hand: {title}")),
                blocks::section(&format!("*Waiting on a prompt*\n{}", choice.display())),
            ];
            if ids.len() == choice.options.len() && !ids.is_empty() {
                blocks.push(buttons(choice, ids));
            }
            blocks.push(links(context.ui_url));
            blocks.push(footer(
                context,
                &format!(
                    "Buttons work once and expire in {minutes} min · \"Always allow\" style options are never offered here"
                ),
            ));
            return OutgoingMessage {
                text: format!("{title} is waiting on a prompt in Yard."),
                blocks: blocks::finish(blocks),
            };
        }
        ParsedScreen::Question { question, .. } => {
            let question = cap(&clean_prompt_line(question), MAX_PROMPT_CHARS);
            return OutgoingMessage {
                text: format!("{title} asked a question."),
                blocks: blocks::finish(vec![
                    blocks::header(&format!(":speech_balloon: {title} asks")),
                    blocks::section(&format!(">{question}")),
                    links(context.ui_url),
                    footer(
                        context,
                        "Reply in this thread to answer; your reply is sent as \"From Slack (owner)\"",
                    ),
                ]),
            };
        }
        ParsedScreen::Unparsed { excerpt } => vec![
            blocks::header(&format!(":raised_hand: {title}")),
            blocks::section(&format!(
                "*Blocked*, but Yard could not read the prompt confidently. Open Yard to answer it.\n```{excerpt}```"
            )),
            links(context.ui_url),
            footer(
                context,
                "Nothing can be answered from Slack for this prompt",
            ),
        ],
    };
    OutgoingMessage {
        text: format!("{title} is blocked. Open Yard to answer it."),
        blocks: blocks::finish(blocks),
    }
}

/// The card after it was used: collapsed to one line, no buttons left.
#[must_use]
pub fn answered_card(title: &str, answer: &str, at_unix_ms: u64) -> OutgoingMessage {
    let answer = clean_prompt_line(answer);
    OutgoingMessage {
        text: format!("{title}: answered from Slack ({answer})."),
        blocks: blocks::finish(vec![
            blocks::section(&format!(
                ":white_check_mark: *Answered:* {answer} by you · {}",
                since_token(at_unix_ms)
            )),
            blocks::context(&[format!("*{title}*")]),
        ]),
    }
}

/// The card after a click that sent nothing: no buttons left, and why.
#[must_use]
pub fn not_sent_card(title: &str, refusal: &Refusal) -> OutgoingMessage {
    let reason = match refusal {
        Refusal::Unknown => "Yard no longer knows these buttons",
        Refusal::Expired => "the buttons expired",
        Refusal::AlreadyUsed => "it was already answered from Slack",
        Refusal::Gone(_) => "the agent is no longer running there",
        Refusal::TerminalChanged => "its terminal changed",
        Refusal::NoLongerBlocked => "the agent is no longer waiting on it",
        Refusal::PromptChanged => "the prompt changed",
        Refusal::NotOffered => "that option can't be sent from Slack",
        Refusal::Busy(_) => "the terminal is busy in Yard",
        Refusal::Unavailable(message) => message.trim_end_matches('.'),
        Refusal::Failed(_) => "sending failed",
    };
    OutgoingMessage {
        text: format!("{title}: not sent ({reason})."),
        blocks: blocks::finish(vec![
            blocks::section(&format!(":no_entry_sign: *Not sent:* {reason}.")),
            blocks::context(&[format!("*{title}*")]),
        ]),
    }
}

/// What to say when a click or reply did not reach the agent.
#[must_use]
pub fn refusal_text(title: &str, refusal: &Refusal) -> String {
    match refusal {
        Refusal::Unknown => {
            "Yard does not know that button (Yard may have restarted). Nothing was sent.".to_owned()
        }
        Refusal::Expired => {
            "That button expired. Nothing was sent; use `blocked` or Yard for the current prompt."
                .to_owned()
        }
        Refusal::AlreadyUsed => {
            "That prompt was already answered from Slack. Nothing else was sent.".to_owned()
        }
        Refusal::Gone(_) => format!("{title} is no longer running there. Nothing was sent."),
        Refusal::TerminalChanged => format!(
            "{title}'s terminal changed since this prompt was posted. Nothing was sent; open Yard."
        ),
        Refusal::NoLongerBlocked => {
            format!("{title} is no longer waiting on this prompt. Nothing was sent.")
        }
        Refusal::PromptChanged => {
            format!("That prompt changed. Nothing was sent. Here is what {title} shows now:")
        }
        Refusal::NotOffered => "That option can't be sent from Slack. Nothing was sent.".to_owned(),
        Refusal::Busy(_) => {
            format!("{title}'s terminal is open in Yard; answer it there. Nothing was sent.")
        }
        Refusal::Unavailable(message) => {
            format!("{title}: {message} Nothing was sent; open Yard.")
        }
        Refusal::Failed(_) => {
            format!("Yard could not reach {title}'s terminal. Nothing was confirmed; check Yard.")
        }
    }
}

/// A short, plain reply (no blocks).
#[must_use]
pub fn plain(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        text: text.to_owned(),
        blocks: json!([blocks::section(text)]),
    }
}

#[cfg(test)]
mod tests {
    use super::{CardContext, answered_card, not_sent_card, prompt_card};
    use crate::slack::actions::Refusal;
    use crate::slack::prompt::{
        parse_screen,
        tests::{CLAUDE_MENU, CLAUDE_PERMISSION, CLAUDE_QUESTION, WORKING},
    };

    const CONTEXT: CardContext<'static> = CardContext {
        ui_url: "https://yard.example.test/",
        pane: Some("wG:p1"),
        at_unix_ms: 1_695_900_000_000,
    };

    #[test]
    fn prompt_cards_offer_only_one_time_answers_with_opaque_values() {
        let ids = vec!["a".repeat(32), "b".repeat(32)];
        let card = prompt_card("Worker", &parse_screen(CLAUDE_PERMISSION), &ids, &CONTEXT);
        let blocks = card.blocks.to_string();
        let buttons = card.blocks[2]["elements"].as_array().unwrap();
        let labels = buttons
            .iter()
            .map(|button| button["text"]["text"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["Allow once", "Deny"]);
        assert_eq!(buttons[0]["value"], ids[0].as_str());
        assert_eq!(buttons[1]["style"], "danger");
        assert!(
            !blocks.contains("abc123") && !blocks.contains("/Users/"),
            "{blocks}"
        );
        assert!(blocks.contains("not offered in Slack"));
        let menu = prompt_card("Worker", &parse_screen(CLAUDE_MENU), &ids, &CONTEXT);
        let labels = menu.blocks[2]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|button| button["text"]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["1. Yard server", "2. Herdr plugin"]);
        // Without ids (RNG failure) there are no buttons at all.
        let bare = prompt_card("Worker", &parse_screen(CLAUDE_PERMISSION), &[], &CONTEXT);
        assert!(!bare.blocks.to_string().contains("\"value\""));
        assert!(!bare.blocks.to_string().contains("yard_prompt"));
        let question = prompt_card("Worker", &parse_screen(CLAUDE_QUESTION), &[], &CONTEXT);
        assert!(question.blocks.to_string().contains("Reply in this thread"));
        let unparsed = prompt_card("Worker", &parse_screen(WORKING), &[], &CONTEXT);
        let text = unparsed.blocks.to_string();
        assert!(
            text.contains("Open Yard") && !text.contains("xoxb-"),
            "{text}"
        );
        assert!(!text.contains("\"value\""));
        assert!(text.contains("https://yard.example.test/"));
        let answered = answered_card("Worker", "<!channel> ok", 1_695_900_000_000);
        assert!(answered.blocks.to_string().contains("&lt;!channel&gt;"));
        let line = answered.blocks[0]["text"]["text"].as_str().unwrap();
        assert!(
            line.starts_with(
                ":white_check_mark: *Answered:* &lt;!channel&gt; ok by you · <!date^1695900000^"
            ),
            "{line}"
        );
        assert!(
            !answered.blocks.to_string().contains("\"button\""),
            "collapsed"
        );
        let card = prompt_card("Worker", &parse_screen(CLAUDE_PERMISSION), &ids, &CONTEXT);
        assert_eq!(card.blocks[0]["text"]["text"], ":raised_hand: Worker");
        assert_eq!(
            card.blocks[3]["elements"][0]["url"],
            "https://yard.example.test/"
        );
        assert!(card.blocks[4].to_string().contains("pane `wG:p1`"));
        let refused = not_sent_card("Worker", &Refusal::PromptChanged);
        assert_eq!(refused.text, "Worker: not sent (the prompt changed).");
        assert!(!refused.blocks.to_string().contains("answered"));
    }
}

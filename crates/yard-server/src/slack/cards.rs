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
    message::{OutgoingMessage, clean_prompt_line},
    prompt::{ChoicePrompt, MAX_PROMPT_CHARS, OptionRole, ParsedScreen, cap},
};

/// Slack caps button text at 75 characters.
const MAX_BUTTON_CHARS: usize = 60;
/// `action_id` prefix of Yard's prompt buttons.
pub const BUTTON_ACTION_PREFIX: &str = "yard_prompt_";

fn section(text: &str) -> Value {
    json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } })
}

fn context(text: &str) -> Value {
    json!({ "type": "context", "elements": [{ "type": "mrkdwn", "text": text }] })
}

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

/// The card for what `title` (already cleaned) shows now. `ids` has one
/// action id per offered option of a choice prompt.
#[must_use]
pub fn prompt_card(title: &str, screen: &ParsedScreen, ids: &[String]) -> OutgoingMessage {
    let minutes = ACTION_TTL.as_secs() / 60;
    match screen {
        ParsedScreen::Choice(choice) => {
            let text = format!("{title} is waiting on a prompt in Yard.");
            let body = format!("*{title}* is waiting on a prompt:\n{}", choice.display());
            let mut blocks = vec![section(&body)];
            if ids.len() == choice.options.len() && !ids.is_empty() {
                blocks.push(buttons(choice, ids));
            }
            blocks.push(context(&format!(
                "Buttons work once and expire in {minutes} min. \"Always allow\" style options are never offered here."
            )));
            OutgoingMessage {
                text,
                blocks: Value::Array(blocks),
            }
        }
        ParsedScreen::Question { question, .. } => {
            let question = cap(&clean_prompt_line(question), MAX_PROMPT_CHARS);
            OutgoingMessage {
                text: format!("{title} asked a question."),
                blocks: json!([
                    section(&format!("*{title}* asks:\n>{question}")),
                    context(
                        "Reply in this thread to answer; your reply is sent as \"From Slack (owner)\"."
                    ),
                ]),
            }
        }
        ParsedScreen::Unparsed { excerpt } => OutgoingMessage {
            text: format!("{title} is blocked. Open Yard to answer it."),
            blocks: json!([section(&format!(
                "*{title}* is blocked, but Yard could not read the prompt confidently. Open Yard to answer it.\n```{excerpt}```"
            )),]),
        },
    }
}

/// The card after it was used: no buttons left.
#[must_use]
pub fn answered_card(title: &str, answer: &str) -> OutgoingMessage {
    let answer = clean_prompt_line(answer);
    OutgoingMessage {
        text: format!("{title}: answered from Slack ({answer})."),
        blocks: json!([section(&format!(
            "*{title}*: answered from Slack — {answer}."
        ))]),
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
        blocks: json!([section(&format!("*{title}*: not sent — {reason}."))]),
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
        blocks: json!([section(text)]),
    }
}

#[cfg(test)]
mod tests {
    use super::{answered_card, not_sent_card, prompt_card};
    use crate::slack::actions::Refusal;
    use crate::slack::prompt::{
        parse_screen,
        tests::{CLAUDE_MENU, CLAUDE_PERMISSION, CLAUDE_QUESTION, WORKING},
    };

    #[test]
    fn prompt_cards_offer_only_one_time_answers_with_opaque_values() {
        let ids = vec!["a".repeat(32), "b".repeat(32)];
        let card = prompt_card("Worker", &parse_screen(CLAUDE_PERMISSION), &ids);
        let blocks = card.blocks.to_string();
        let buttons = card.blocks[1]["elements"].as_array().unwrap();
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
        let menu = prompt_card("Worker", &parse_screen(CLAUDE_MENU), &ids);
        let labels = menu.blocks[1]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|button| button["text"]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["1. Yard server", "2. Herdr plugin"]);
        // Without ids (RNG failure) there are no buttons at all.
        let bare = prompt_card("Worker", &parse_screen(CLAUDE_PERMISSION), &[]);
        assert!(!bare.blocks.to_string().contains("\"button\""));
        let question = prompt_card("Worker", &parse_screen(CLAUDE_QUESTION), &[]);
        assert!(question.blocks.to_string().contains("Reply in this thread"));
        let unparsed = prompt_card("Worker", &parse_screen(WORKING), &[]);
        let text = unparsed.blocks.to_string();
        assert!(
            text.contains("Open Yard") && !text.contains("xoxb-"),
            "{text}"
        );
        assert!(!text.contains("\"button\""));
        let answered = answered_card("Worker", "<!channel> ok");
        assert!(answered.blocks.to_string().contains("&lt;!channel&gt;"));
        let refused = not_sent_card("Worker", &Refusal::PromptChanged);
        assert_eq!(refused.text, "Worker: not sent (the prompt changed).");
        assert!(!refused.blocks.to_string().contains("answered"));
    }
}

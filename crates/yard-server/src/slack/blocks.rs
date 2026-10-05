//! Block Kit primitives and Slack's size limits.
//!
//! Every builder here caps what it is given, so a card can never be
//! rejected for size: at most [`MAX_BLOCKS`] blocks per message,
//! [`MAX_TEXT`] characters per text object, [`MAX_FIELDS`] fields per
//! section, [`MAX_ELEMENTS`] elements per actions block. Text passed in is
//! already cleaned and escaped by the caller ([`super::message::clean`]);
//! plain-text objects (headers, buttons) are un-escaped first because Slack
//! shows them literally.

use serde_json::{Value, json};

use super::prompt::cap;

/// Slack rejects messages with more blocks.
pub const MAX_BLOCKS: usize = 50;
/// Section / context text objects.
pub const MAX_TEXT: usize = 3_000;
/// A section's `fields`.
pub const MAX_FIELDS: usize = 10;
/// One field's text.
const MAX_FIELD_TEXT: usize = 2_000;
/// Elements per actions block (Slack's limit).
pub const MAX_ELEMENTS: usize = 25;
/// Buttons per actions row that read well (Slack wraps the rest).
pub const ROW_BUTTONS: usize = 5;
/// Header text.
const MAX_HEADER: usize = 150;
/// Button text (Slack caps it at 75).
const MAX_BUTTON: usize = 60;
/// Elements of a context block.
const MAX_CONTEXT: usize = 10;
/// Button `url`.
const MAX_URL: usize = 3_000;
/// Where "Open in Yard" points unless `YARD_SLACK_UI_URL` says otherwise:
/// the owner opens it on their Mac through the port forward.
pub const DEFAULT_UI_URL: &str = "http://127.0.0.1:4317/";
/// The environment variable that overrides [`DEFAULT_UI_URL`].
pub const UI_URL_VAR: &str = "YARD_SLACK_UI_URL";
/// `action_id` of every "Open in Yard" link button (Slack still reports the
/// click; the inbound filter drops it without acting).
pub const OPEN_ACTION_ID: &str = "yard_open";

/// The public UI URL from `YARD_SLACK_UI_URL`: an `http(s)://` URL without
/// whitespace or credentials, else the default.
#[must_use]
pub fn ui_url(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return DEFAULT_UI_URL.to_owned();
    };
    let lower = raw.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let valid = rest.is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !authority.is_empty() && !authority.contains('@')
    }) && raw.len() <= MAX_URL
        && !raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '|' | '"'));
    if valid {
        raw.to_owned()
    } else {
        tracing::warn!("{UI_URL_VAR} is not an http(s) URL; using {DEFAULT_UI_URL}");
        DEFAULT_UI_URL.to_owned()
    }
}

/// Undo [`super::message::escape`] for plain-text objects.
fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let mut kept = text.chars().take(max - 1).collect::<String>();
        kept.push('…');
        kept
    } else {
        text.to_owned()
    }
}

/// A header (plain text, emoji codes rendered).
#[must_use]
pub fn header(text: &str) -> Value {
    json!({
        "type": "header",
        "text": { "type": "plain_text", "text": short(&unescaped(text), MAX_HEADER), "emoji": true },
    })
}

/// A mrkdwn section, capped.
#[must_use]
pub fn section(text: &str) -> Value {
    json!({ "type": "section", "text": { "type": "mrkdwn", "text": cap(text, MAX_TEXT - 20) } })
}

/// A section with a button on its right.
#[must_use]
pub fn section_with(text: &str, accessory: Value) -> Value {
    let mut block = section(text);
    block["accessory"] = accessory;
    block
}

/// A section of `*label*\nvalue` fields (at most [`MAX_FIELDS`]).
#[must_use]
pub fn fields(pairs: &[(&str, String)]) -> Value {
    let fields = pairs
        .iter()
        .take(MAX_FIELDS)
        .map(|(label, value)| {
            json!({ "type": "mrkdwn", "text": cap(&format!("*{label}*\n{value}"), MAX_FIELD_TEXT - 20) })
        })
        .collect::<Vec<_>>();
    json!({ "type": "section", "fields": fields })
}

/// A context line of mrkdwn parts.
#[must_use]
pub fn context(parts: &[String]) -> Value {
    let elements = parts
        .iter()
        .filter(|part| !part.is_empty())
        .take(MAX_CONTEXT)
        .map(|part| json!({ "type": "mrkdwn", "text": cap(part, MAX_TEXT - 20) }))
        .collect::<Vec<_>>();
    json!({ "type": "context", "elements": elements })
}

#[must_use]
pub fn divider() -> Value {
    json!({ "type": "divider" })
}

/// Button look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Default,
    Primary,
    Danger,
}

/// A button whose only payload is `value` (an opaque action id).
#[must_use]
pub fn button(text: &str, action_id: &str, value: &str, style: Style) -> Value {
    let mut button = json!({
        "type": "button",
        "action_id": action_id,
        "text": { "type": "plain_text", "text": short(&unescaped(text), MAX_BUTTON), "emoji": true },
        "value": value,
    });
    match style {
        Style::Primary => button["style"] = json!("primary"),
        Style::Danger => button["style"] = json!("danger"),
        Style::Default => {}
    }
    button
}

/// The "Open in Yard" link button.
#[must_use]
pub fn open_in_yard(url: &str) -> Value {
    json!({
        "type": "button",
        "action_id": OPEN_ACTION_ID,
        "text": { "type": "plain_text", "text": "Open in Yard", "emoji": true },
        "url": url,
    })
}

/// An actions block (at most [`MAX_ELEMENTS`] elements).
#[must_use]
pub fn actions(block_id: &str, elements: Vec<Value>) -> Value {
    let elements = elements.into_iter().take(MAX_ELEMENTS).collect::<Vec<_>>();
    json!({ "type": "actions", "block_id": block_id, "elements": elements })
}

/// The finished `blocks` array: past [`MAX_BLOCKS`] the tail is replaced by
/// a "+N more — open Yard" context.
#[must_use]
pub fn finish(mut blocks: Vec<Value>) -> Value {
    if blocks.len() > MAX_BLOCKS {
        let hidden = blocks.len() - (MAX_BLOCKS - 1);
        blocks.truncate(MAX_BLOCKS - 1);
        blocks.push(context(&[format!("+{hidden} more — open Yard.")]));
    }
    Value::Array(blocks)
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_UI_URL, MAX_BLOCKS, MAX_ELEMENTS, MAX_FIELDS, Style, actions, button, context,
        fields, finish, header, open_in_yard, section, ui_url,
    };

    #[test]
    fn builders_respect_slack_limits() {
        let long = "x".repeat(10_000);
        let text = section(&long)["text"]["text"].as_str().unwrap().to_owned();
        assert!(text.chars().count() <= 3_000 && text.ends_with("(truncated)"));
        let header = header(&format!("&lt;!channel&gt; {long}"));
        let header = header["text"]["text"].as_str().unwrap();
        assert!(header.starts_with("<!channel>") && header.chars().count() <= 150);
        let pairs = (0..15).map(|_| ("Label", long.clone())).collect::<Vec<_>>();
        let fields = fields(&pairs);
        let fields = fields["fields"].as_array().unwrap();
        assert_eq!(fields.len(), MAX_FIELDS);
        assert!(fields[0]["text"].as_str().unwrap().chars().count() <= 2_000);
        let many = (0..40)
            .map(|n| button(&long, &format!("b{n}"), "v", Style::Default))
            .collect();
        let row = actions("row", many);
        let row = row["elements"].as_array().unwrap();
        assert_eq!(row.len(), MAX_ELEMENTS);
        assert!(row[0]["text"]["text"].as_str().unwrap().chars().count() <= 75);
        let blocks = finish((0..70).map(|_| context(&["line".to_owned()])).collect());
        let blocks = blocks.as_array().unwrap();
        assert_eq!(blocks.len(), MAX_BLOCKS);
        assert!(
            blocks[MAX_BLOCKS - 1]
                .to_string()
                .contains("+21 more — open Yard")
        );
        let link = open_in_yard(DEFAULT_UI_URL);
        assert_eq!(link["url"], DEFAULT_UI_URL);
        assert!(link.get("value").is_none());
        assert_eq!(button("Deny", "a", "v", Style::Danger)["style"], "danger");
    }

    #[test]
    fn the_ui_url_is_an_http_url_or_the_default() {
        assert_eq!(ui_url(None), DEFAULT_UI_URL);
        assert_eq!(ui_url(Some("  ")), DEFAULT_UI_URL);
        assert_eq!(
            ui_url(Some("https://yard.example.dev/app")),
            "https://yard.example.dev/app"
        );
        assert_eq!(
            ui_url(Some("http://localhost:8080/")),
            "http://localhost:8080/"
        );
        for bad in [
            "javascript:alert(1)",
            "ftp://host/",
            "https://user:pw@host/",
            "https://host/a b",
            "https://",
            "https://host/<!channel>",
        ] {
            assert_eq!(ui_url(Some(bad)), DEFAULT_UI_URL, "{bad}");
        }
    }
}

//! Socket Mode envelopes and the owner-only inbound filter.
//!
//! Every envelope is acknowledged before it is looked at ([`super::socket`]).
//! An event or action is then accepted ONLY when all of these hold:
//! team and enterprise equal the bot's (`auth.test`), the app id equals the
//! one Slack reported in `hello`, the channel is the owner's DM with the bot,
//! the user is `YARD_SLACK_OWNER_USER_ID`, it is not a bot message, our own
//! echo, an edit, a deletion or any other `subtype`, it was not seen before
//! (dedupe by event id / action id + time), and it is fresh (within
//! [`MAX_AGE`], at most [`MAX_SKEW`] in the future). Everything else is
//! dropped with a reason that is logged at DEBUG and never answered.

use std::{collections::HashMap, time::Duration};

use serde_json::Value;

/// Older events and clicks are dropped (replays, long reconnect backlogs).
pub const MAX_AGE: Duration = Duration::from_secs(5 * 60);
/// Clock skew tolerated for timestamps in the future.
pub const MAX_SKEW: Duration = Duration::from_secs(60);
/// Owner messages longer than this are refused, not truncated.
pub const MAX_MESSAGE_CHARS: usize = 4_000;
const DEDUPE_TTL_MS: u64 = 15 * 60 * 1000;
const DEDUPE_CAPACITY: usize = 2_048;

/// One Socket Mode frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    /// Present on everything that must be acknowledged.
    pub envelope_id: Option<String>,
    pub kind: EnvelopeKind,
    pub payload: Value,
    pub retry_attempt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeKind {
    Hello {
        num_connections: u64,
        app_id: Option<String>,
    },
    Disconnect {
        reason: String,
    },
    EventsApi,
    Interactive,
    Other(String),
}

/// Parse one text frame; `None` when it is not a JSON object with `type`.
#[must_use]
pub fn parse_envelope(text: &str) -> Option<Envelope> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    let kind = object.get("type")?.as_str()?;
    let text_field = |name: &str| object.get(name).and_then(Value::as_str).map(str::to_owned);
    let kind = match kind {
        "hello" => EnvelopeKind::Hello {
            num_connections: object
                .get("num_connections")
                .and_then(Value::as_u64)
                .unwrap_or(1),
            app_id: value
                .pointer("/connection_info/app_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "disconnect" => EnvelopeKind::Disconnect {
            reason: text_field("reason").unwrap_or_default(),
        },
        "events_api" => EnvelopeKind::EventsApi,
        "interactive" => EnvelopeKind::Interactive,
        other => EnvelopeKind::Other(other.to_owned()),
    };
    Some(Envelope {
        envelope_id: text_field("envelope_id").filter(|id| !id.is_empty() && id.len() <= 256),
        kind,
        payload: object.get("payload").cloned().unwrap_or(Value::Null),
        retry_attempt: object
            .get("retry_attempt")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

/// Who may talk to Yard, learned from `auth.test`, `conversations.open`
/// and `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundContext {
    pub team_id: String,
    pub enterprise_id: Option<String>,
    pub owner_user_id: String,
    pub dm_channel: String,
    pub bot_user_id: Option<String>,
    pub bot_id: Option<String>,
    /// From `hello.connection_info.app_id`; `None` until the hello.
    pub app_id: Option<String>,
}

/// A DM the owner typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerMessage {
    pub text: String,
    pub ts: String,
    pub thread_ts: Option<String>,
}

/// A button the owner clicked on one of Yard's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerAction {
    /// The opaque, server-generated action id carried as the button value.
    pub action: String,
    /// The button's `action_id` (only tells prompt buttons from
    /// navigation buttons; never trusted for what to do).
    pub action_id: String,
    pub action_ts: String,
    /// The message the button is on, and its thread.
    pub message_ts: String,
    pub thread_ts: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    Message(OwnerMessage),
    Action(OwnerAction),
}

/// Applies the owner-only rules and remembers what it has seen.
#[derive(Debug, Default)]
pub struct InboundFilter {
    context: Option<InboundContext>,
    /// From `hello`; kept across bot-session reconnects.
    app_id: Option<String>,
    seen: HashMap<String, u64>,
}

type Verdict = Result<Inbound, &'static str>;

impl InboundFilter {
    #[must_use]
    pub fn new(context: InboundContext) -> Self {
        Self {
            app_id: context.app_id.clone(),
            context: Some(context),
            seen: HashMap::new(),
        }
    }

    /// Replace the workspace/DM context (after a reconnect of the bot
    /// session), keeping the dedupe memory and the app id from `hello`.
    pub fn set_context(&mut self, context: InboundContext) {
        if context.app_id.is_some() {
            self.app_id.clone_from(&context.app_id);
        }
        self.context = Some(context);
    }

    /// Record the app id from `hello`.
    pub fn set_app_id(&mut self, app_id: Option<String>) {
        self.app_id = app_id;
    }

    /// Decide on one acknowledged envelope. `Err` carries the drop reason.
    ///
    /// # Errors
    ///
    /// Returns the reason the envelope is not accepted.
    pub fn accept(&mut self, envelope: &Envelope, now_unix_ms: u64) -> Verdict {
        let mut context = self.context.clone().ok_or("not_connected")?;
        context.app_id.clone_from(&self.app_id);
        let context = &context;
        let (inbound, key) = match envelope.kind {
            EnvelopeKind::EventsApi => message_event(context, &envelope.payload, now_unix_ms)?,
            EnvelopeKind::Interactive => block_action(context, &envelope.payload, now_unix_ms)?,
            _ => return Err("unsupported_envelope"),
        };
        self.seen
            .retain(|_, at| now_unix_ms.saturating_sub(*at) < DEDUPE_TTL_MS);
        if self.seen.contains_key(&key) {
            return Err("duplicate");
        }
        if self.seen.len() >= DEDUPE_CAPACITY
            && let Some(oldest) = self
                .seen
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone())
        {
            self.seen.remove(&oldest);
        }
        self.seen.insert(key, now_unix_ms);
        Ok(inbound)
    }
}

impl InboundFilter {
    /// For an owner DM dropped only for its age (`stale` /
    /// `from_the_future`, checked after the team, app, channel and owner
    /// checks passed): the message ts to reply under, at most once per
    /// event id. Nothing is ever acted on.
    pub fn late_owner_message(
        &mut self,
        envelope: &Envelope,
        reason: &str,
        now_unix_ms: u64,
    ) -> Option<String> {
        if !matches!(reason, "stale" | "from_the_future")
            || envelope.kind != EnvelopeKind::EventsApi
        {
            return None;
        }
        let event_id = text(&envelope.payload, "/event_id").filter(|id| !id.is_empty())?;
        let ts = text(&envelope.payload, "/event/ts")?.to_owned();
        let key = format!("late:{event_id}");
        self.seen
            .retain(|_, at| now_unix_ms.saturating_sub(*at) < DEDUPE_TTL_MS);
        if self.seen.contains_key(&key) || self.seen.len() >= DEDUPE_CAPACITY {
            return None;
        }
        self.seen.insert(key, now_unix_ms);
        Some(ts)
    }
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// Team, enterprise and app must all be ours.
fn same_workspace(
    context: &InboundContext,
    team: Option<&str>,
    enterprise: Option<&str>,
    app: Option<&str>,
) -> Result<(), &'static str> {
    if team != Some(context.team_id.as_str()) {
        return Err("foreign_team");
    }
    if enterprise.filter(|id| !id.is_empty()) != context.enterprise_id.as_deref() {
        return Err("foreign_enterprise");
    }
    match (&context.app_id, app) {
        (Some(expected), Some(app)) if expected == app => Ok(()),
        (None, _) => Err("no_hello_yet"),
        _ => Err("foreign_app"),
    }
}

/// `1700000000.000100` (Slack ts) or `1700000000` → Unix ms.
fn slack_time_ms(value: &Value) -> Option<u64> {
    let seconds = match value {
        Value::String(text) => text.parse::<f64>().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    (seconds.is_finite() && seconds > 0.0).then(|| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = (seconds * 1000.0) as u64;
        ms
    })
}

fn fresh(at_ms: Option<u64>, now_unix_ms: u64) -> Result<(), &'static str> {
    let at_ms = at_ms.ok_or("missing_timestamp")?;
    let max_age = u64::try_from(MAX_AGE.as_millis()).unwrap_or(u64::MAX);
    let max_skew = u64::try_from(MAX_SKEW.as_millis()).unwrap_or(u64::MAX);
    if now_unix_ms.saturating_sub(at_ms) > max_age {
        return Err("stale");
    }
    if at_ms.saturating_sub(now_unix_ms) > max_skew {
        return Err("from_the_future");
    }
    Ok(())
}

/// `events_api` → `event_callback` → `message` in the owner's DM.
fn message_event(
    context: &InboundContext,
    payload: &Value,
    now_unix_ms: u64,
) -> Result<(Inbound, String), &'static str> {
    if text(payload, "/type") != Some("event_callback") {
        return Err("not_an_event_callback");
    }
    same_workspace(
        context,
        text(payload, "/team_id"),
        text(payload, "/enterprise_id"),
        text(payload, "/api_app_id"),
    )?;
    let event_id = text(payload, "/event_id")
        .filter(|id| !id.is_empty())
        .ok_or("missing_event_id")?;
    let event = payload.get("event").ok_or("missing_event")?;
    if text(event, "/type") != Some("message") {
        return Err("not_a_message");
    }
    // Edits, deletions, joins, bot posts, file shares, thread broadcasts…
    if event.get("subtype").is_some() || event.get("edited").is_some() {
        return Err("subtype");
    }
    if event.get("bot_id").is_some() || event.get("bot_profile").is_some() {
        return Err("bot_message");
    }
    if event.get("hidden").and_then(Value::as_bool) == Some(true) {
        return Err("hidden");
    }
    if text(event, "/channel_type") != Some("im")
        || text(event, "/channel") != Some(context.dm_channel.as_str())
    {
        return Err("not_the_owner_dm");
    }
    let user = text(event, "/user").ok_or("missing_user")?;
    if context.bot_user_id.as_deref() == Some(user) {
        return Err("own_echo");
    }
    if user != context.owner_user_id {
        return Err("not_the_owner");
    }
    let ts = text(event, "/ts").ok_or("missing_ts")?;
    fresh(
        payload.get("event_time").and_then(slack_time_ms),
        now_unix_ms,
    )?;
    fresh(slack_time_ms(&Value::String(ts.to_owned())), now_unix_ms)?;
    let body = text(event, "/text").unwrap_or_default().trim();
    if body.is_empty() {
        return Err("empty");
    }
    if body.chars().count() > MAX_MESSAGE_CHARS {
        return Err("too_long");
    }
    let body = slack_plain_text(body);
    let body = body.trim();
    if body.is_empty() {
        return Err("empty");
    }
    let message = OwnerMessage {
        text: body.to_owned(),
        ts: ts.to_owned(),
        thread_ts: text(event, "/thread_ts")
            .filter(|thread| *thread != ts)
            .map(str::to_owned),
    };
    Ok((Inbound::Message(message), format!("event:{event_id}")))
}

/// The owner's text as typed: Slack's `&amp;` / `&lt;` / `&gt;` decoded
/// and links (`<url>`, `<url|label>`) unwrapped. Mentions and channel
/// references (`<@U…>`, `<!here>`, `<#C…|name>`) stay in Slack's form.
#[must_use]
pub fn slack_plain_text(raw: &str) -> String {
    fn decode(text: &str) -> String {
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode(&rest[..open]));
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            out.push_str(&decode(&rest[open..]));
            return out;
        };
        let inner = &after[..close];
        if inner.starts_with(['@', '!', '#']) {
            out.push('<');
            out.push_str(inner);
            out.push('>');
        } else {
            let shown = inner.split_once('|').map_or(inner, |(_, label)| label);
            out.push_str(&decode(shown));
        }
        rest = &after[close + 1..];
    }
    out.push_str(&decode(rest));
    out
}

/// `interactive` → `block_actions`: one button on a message in the DM.
fn block_action(
    context: &InboundContext,
    payload: &Value,
    now_unix_ms: u64,
) -> Result<(Inbound, String), &'static str> {
    if text(payload, "/type") != Some("block_actions") {
        return Err("not_a_block_action");
    }
    same_workspace(
        context,
        text(payload, "/team/id"),
        text(payload, "/enterprise/id"),
        text(payload, "/api_app_id"),
    )?;
    if text(payload, "/user/id") != Some(context.owner_user_id.as_str()) {
        return Err("not_the_owner");
    }
    let channel = text(payload, "/channel/id").or_else(|| text(payload, "/container/channel_id"));
    if channel != Some(context.dm_channel.as_str())
        || text(payload, "/container/channel_id")
            .is_some_and(|container| container != context.dm_channel)
    {
        return Err("not_the_owner_dm");
    }
    if text(payload, "/container/type") != Some("message") {
        return Err("not_on_a_message");
    }
    let actions = payload
        .get("actions")
        .and_then(Value::as_array)
        .ok_or("missing_actions")?;
    let [action] = actions.as_slice() else {
        return Err("not_one_action");
    };
    if text(action, "/type") != Some("button") {
        return Err("not_a_button");
    }
    // "Open in Yard" only opens a URL in the browser; nothing to do here.
    if action.get("url").is_some()
        || text(action, "/action_id") == Some(super::blocks::OPEN_ACTION_ID)
    {
        return Err("link_button");
    }
    let value = text(action, "/value").ok_or("missing_value")?;
    if !is_action_token(value) {
        return Err("malformed_action");
    }
    let action_ts = text(action, "/action_ts").ok_or("missing_timestamp")?;
    fresh(
        slack_time_ms(&Value::String(action_ts.to_owned())),
        now_unix_ms,
    )?;
    let message_ts = text(payload, "/container/message_ts")
        .or_else(|| text(payload, "/message/ts"))
        .ok_or("missing_message")?;
    let key = format!("action:{value}:{action_ts}");
    Ok((
        Inbound::Action(OwnerAction {
            action: value.to_owned(),
            action_id: text(action, "/action_id").unwrap_or_default().to_owned(),
            action_ts: action_ts.to_owned(),
            message_ts: message_ts.to_owned(),
            thread_ts: text(payload, "/message/thread_ts").map(str::to_owned),
        }),
        key,
    ))
}

/// Yard's opaque action ids: 32 lowercase hex characters.
#[must_use]
pub fn is_action_token(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::{
        EnvelopeKind, Inbound, InboundContext, InboundFilter, OwnerAction, OwnerMessage,
        parse_envelope,
    };

    pub(crate) const NOW_S: u64 = 1_800_000_000;
    pub(crate) const ACTION: &str = "0123456789abcdef0123456789abcdef";

    pub(crate) fn context() -> InboundContext {
        InboundContext {
            team_id: "T01SANDBOX".to_owned(),
            enterprise_id: Some("E01SANDBOX0".to_owned()),
            owner_user_id: "U01OWNER".to_owned(),
            dm_channel: "D01DM".to_owned(),
            bot_user_id: Some("UBOT".to_owned()),
            bot_id: Some("B01BOT".to_owned()),
            app_id: Some("A01APP".to_owned()),
        }
    }

    /// An `events_api` envelope for a DM from the owner at `at_s`.
    pub(crate) fn message(envelope: &str, event_id: &str, text: &str, at_s: u64) -> Value {
        json!({
            "envelope_id": envelope, "type": "events_api", "accepts_response_payload": false,
            "retry_attempt": 0,
            "payload": {
                "type": "event_callback", "team_id": "T01SANDBOX", "enterprise_id": "E01SANDBOX0",
                "api_app_id": "A01APP", "event_id": event_id, "event_time": at_s,
                "event": {
                    "type": "message", "channel": "D01DM", "channel_type": "im",
                    "user": "U01OWNER", "text": text, "ts": format!("{at_s}.000100"),
                }
            }
        })
    }

    /// An `interactive` envelope for one button click at `at_s`.
    pub(crate) fn click(envelope: &str, action: &str, at_s: u64) -> Value {
        json!({
            "envelope_id": envelope, "type": "interactive", "accepts_response_payload": true,
            "payload": {
                "type": "block_actions", "api_app_id": "A01APP",
                "team": { "id": "T01SANDBOX" }, "enterprise": { "id": "E01SANDBOX0" },
                "user": { "id": "U01OWNER" }, "channel": { "id": "D01DM" },
                "container": { "type": "message", "message_ts": "1799999990.000200", "channel_id": "D01DM" },
                "message": { "ts": "1799999990.000200", "thread_ts": "1799999900.000100" },
                "actions": [{
                    "type": "button", "action_id": "yard_prompt_0", "value": action,
                    "action_ts": format!("{at_s}.123456"),
                }]
            }
        })
    }

    fn accept(filter: &mut InboundFilter, value: &Value) -> Result<Inbound, &'static str> {
        let envelope = parse_envelope(&value.to_string()).expect("envelope parses");
        filter.accept(&envelope, NOW_S * 1000)
    }

    /// Set (or add) the field at `pointer`.
    pub(crate) fn with(mut value: Value, pointer: &str, replacement: Value) -> Value {
        let (parent, key) = pointer.rsplit_once('/').expect(pointer);
        let parent = value.pointer_mut(parent).expect(pointer);
        match parent {
            Value::Array(items) => items[key.parse::<usize>().expect(pointer)] = replacement,
            Value::Object(object) => {
                object.insert(key.to_owned(), replacement);
            }
            _ => panic!("{pointer}"),
        }
        value
    }

    fn without(mut value: Value, parent: &str, key: &str) -> Value {
        value
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .expect(parent)
            .remove(key);
        value
    }

    #[test]
    fn parses_hello_disconnect_and_payload_envelopes() {
        let hello = parse_envelope(
            r#"{"type":"hello","num_connections":2,"connection_info":{"app_id":"A01APP"}}"#,
        )
        .unwrap();
        assert_eq!(
            hello.kind,
            EnvelopeKind::Hello {
                num_connections: 2,
                app_id: Some("A01APP".to_owned())
            }
        );
        assert_eq!(hello.envelope_id, None);
        let disconnect =
            parse_envelope(r#"{"type":"disconnect","reason":"refresh_requested"}"#).unwrap();
        assert_eq!(
            disconnect.kind,
            EnvelopeKind::Disconnect {
                reason: "refresh_requested".to_owned()
            }
        );
        let event = parse_envelope(&message("env-1", "Ev1", "status", NOW_S).to_string()).unwrap();
        assert_eq!(event.kind, EnvelopeKind::EventsApi);
        assert_eq!(event.envelope_id.as_deref(), Some("env-1"));
        assert!(parse_envelope("not json").is_none());
        assert!(parse_envelope("[1]").is_none());
    }

    #[test]
    fn late_owner_messages_get_one_notice_and_strangers_none() {
        let mut filter = InboundFilter::new(context());
        let old = message("env-1", "Ev9", "blocked", NOW_S - 400);
        let envelope = parse_envelope(&old.to_string()).unwrap();
        let reason = filter.accept(&envelope, NOW_S * 1000).unwrap_err();
        assert_eq!(reason, "stale");
        let ts = format!("{}.000100", NOW_S - 400);
        assert_eq!(
            filter.late_owner_message(&envelope, reason, NOW_S * 1000),
            Some(ts)
        );
        assert_eq!(
            filter.late_owner_message(&envelope, reason, NOW_S * 1000),
            None,
            "a Slack retry of the same event gets no second notice"
        );
        let stranger = with(old, "/payload/event/user", json!("U0STRANGER"));
        let stranger = with(stranger, "/payload/event_id", json!("Ev10"));
        let envelope = parse_envelope(&stranger.to_string()).unwrap();
        let reason = filter.accept(&envelope, NOW_S * 1000).unwrap_err();
        assert_eq!(reason, "not_the_owner");
        assert_eq!(
            filter.late_owner_message(&envelope, reason, NOW_S * 1000),
            None
        );
    }

    #[test]
    fn owner_text_is_decoded_the_way_it_was_typed() {
        use super::slack_plain_text;
        for (raw, typed) in [
            ("R&amp;D: what&#39;s left?", "R&D: what&#39;s left?"),
            (
                "run `a &amp;&amp; b &gt; out.txt`",
                "run `a && b > out.txt`",
            ),
            ("x &amp;lt; y", "x &lt; y"),
            (
                "see <https://example.com/a?b=1&amp;c=2>",
                "see https://example.com/a?b=1&c=2",
            ),
            (
                "see <http://example.com|example.com> now",
                "see example.com now",
            ),
            (
                "Checkout <!channel>: hi <@U01X> in <#C01|general>",
                "Checkout <!channel>: hi <@U01X> in <#C01|general>",
            ),
            ("unclosed &lt;tag", "unclosed <tag"),
        ] {
            assert_eq!(slack_plain_text(raw), typed, "{raw}");
        }
        let mut filter = InboundFilter::new(context());
        let event = message("env-1", "Ev1", "R&amp;D: ship &lt;it&gt;", NOW_S - 10);
        let Ok(Inbound::Message(message)) = accept(&mut filter, &event) else {
            panic!("owner message");
        };
        assert_eq!(message.text, "R&D: ship <it>");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn accepts_only_fresh_unseen_owner_messages_in_the_dm() {
        let mut filter = InboundFilter::new(context());
        let good = message("env-1", "Ev1", "  status  ", NOW_S - 10);
        assert_eq!(
            accept(&mut filter, &good),
            Ok(Inbound::Message(OwnerMessage {
                text: "status".to_owned(),
                ts: format!("{}.000100", NOW_S - 10),
                thread_ts: None,
            }))
        );
        assert_eq!(
            accept(&mut filter, &good),
            Err("duplicate"),
            "replayed event id"
        );
        let retried = with(good.clone(), "/envelope_id", json!("env-2"));
        assert_eq!(
            accept(&mut filter, &retried),
            Err("duplicate"),
            "Slack retry"
        );
        let base = |id: &str| message("env-x", id, "hi", NOW_S - 10);
        for (value, reason) in [
            (
                with(base("E2"), "/payload/team_id", json!("T0OTHER")),
                "foreign_team",
            ),
            (
                with(base("E3"), "/payload/enterprise_id", json!("E0OTHER")),
                "foreign_enterprise",
            ),
            (
                without(base("E4"), "/payload", "enterprise_id"),
                "foreign_enterprise",
            ),
            (
                with(base("E5"), "/payload/api_app_id", json!("A0OTHER")),
                "foreign_app",
            ),
            (
                with(base("E6"), "/payload/event/channel", json!("D0OTHER")),
                "not_the_owner_dm",
            ),
            (
                with(base("E7"), "/payload/event/channel_type", json!("channel")),
                "not_the_owner_dm",
            ),
            (
                with(base("E8"), "/payload/event/user", json!("U0SOMEONE")),
                "not_the_owner",
            ),
            (
                with(base("E9"), "/payload/event/user", json!("UBOT")),
                "own_echo",
            ),
            (
                with(
                    base("E10"),
                    "/payload/event/subtype",
                    json!("message_changed"),
                ),
                "subtype",
            ),
            (
                with(base("E11"), "/payload/event/subtype", json!("bot_message")),
                "subtype",
            ),
            (
                with(base("E12"), "/payload/event/bot_id", json!("B01BOT")),
                "bot_message",
            ),
            (
                with(
                    base("E13"),
                    "/payload/event/edited",
                    json!({"user": "U01OWNER"}),
                ),
                "subtype",
            ),
            (message("env-x", "E14", "old", NOW_S - 301), "stale"),
            (
                message("env-x", "E15", "future", NOW_S + 120),
                "from_the_future",
            ),
            (
                with(
                    base("E16"),
                    "/payload/event/ts",
                    json!(format!("{}.1", NOW_S - 400)),
                ),
                "stale",
            ),
            (message("env-x", "E17", "   ", NOW_S), "empty"),
            (
                message("env-x", "E18", &"x".repeat(4_001), NOW_S),
                "too_long",
            ),
            (
                with(base("E19"), "/payload/type", json!("url_verification")),
                "not_an_event_callback",
            ),
        ] {
            assert_eq!(accept(&mut filter, &value), Err(reason), "{value}");
        }
        let threaded = with(
            message("env-y", "E20", "yes please", NOW_S),
            "/payload/event/thread_ts",
            json!("1799999900.000100"),
        );
        let Ok(Inbound::Message(reply)) = accept(&mut filter, &threaded) else {
            panic!("thread reply accepted");
        };
        assert_eq!(reply.thread_ts.as_deref(), Some("1799999900.000100"));
    }

    #[test]
    fn accepts_only_owner_button_clicks_on_dm_messages() {
        let mut filter = InboundFilter::new(context());
        let good = click("env-1", ACTION, NOW_S - 5);
        assert_eq!(
            accept(&mut filter, &good),
            Ok(Inbound::Action(OwnerAction {
                action: ACTION.to_owned(),
                action_id: "yard_prompt_0".to_owned(),
                action_ts: format!("{}.123456", NOW_S - 5),
                message_ts: "1799999990.000200".to_owned(),
                thread_ts: Some("1799999900.000100".to_owned()),
            }))
        );
        assert_eq!(accept(&mut filter, &good), Err("duplicate"));
        let base = || click("env-x", "fedcba9876543210fedcba9876543210", NOW_S);
        for (value, reason) in [
            (
                with(base(), "/payload/team/id", json!("T0OTHER")),
                "foreign_team",
            ),
            (
                with(base(), "/payload/enterprise", Value::Null),
                "foreign_enterprise",
            ),
            (
                with(base(), "/payload/user/id", json!("U0SOMEONE")),
                "not_the_owner",
            ),
            (
                with(base(), "/payload/channel/id", json!("C0PUBLIC")),
                "not_the_owner_dm",
            ),
            (
                with(base(), "/payload/container/channel_id", json!("C0PUBLIC")),
                "not_the_owner_dm",
            ),
            (
                with(base(), "/payload/container/type", json!("view")),
                "not_on_a_message",
            ),
            (
                with(base(), "/payload/actions/0/type", json!("static_select")),
                "not_a_button",
            ),
            (
                with(base(), "/payload/actions/0/value", json!("worker-1:allow")),
                "malformed_action",
            ),
            (
                with(base(), "/payload/actions", json!([])),
                "not_one_action",
            ),
            (
                // Two buttons in one payload: neither is taken.
                with(
                    base(),
                    "/payload/actions",
                    json!([
                        base()["payload"]["actions"][0].clone(),
                        base()["payload"]["actions"][0].clone()
                    ]),
                ),
                "not_one_action",
            ),
            (
                click("env-x", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", NOW_S - 400),
                "stale",
            ),
            (
                with(base(), "/payload/type", json!("view_submission")),
                "not_a_block_action",
            ),
        ] {
            assert_eq!(accept(&mut filter, &value), Err(reason), "{value}");
        }
        let mut unhelloed = context();
        unhelloed.app_id = None;
        let mut filter = InboundFilter::new(unhelloed);
        assert_eq!(accept(&mut filter, &base()), Err("no_hello_yet"));
        filter.set_app_id(Some("A01APP".to_owned()));
        assert!(accept(&mut filter, &base()).is_ok());
        assert_eq!(
            InboundFilter::default()
                .accept(&parse_envelope(&base().to_string()).unwrap(), NOW_S * 1000),
            Err("not_connected")
        );
    }
}

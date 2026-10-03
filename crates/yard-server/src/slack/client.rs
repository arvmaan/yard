//! Minimal Slack Web API client: `auth.test`, `conversations.open`,
//! `chat.postMessage`, `chat.update` (bot token) and `apps.connections.open`
//! (app-level token, Socket Mode). Tokens travel in a sensitive
//! `Authorization` header, never in a URL, a log line or an error. Responses
//! are size-bounded.

use std::time::Duration;

use reqwest::{
    StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER},
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{
    message::OutgoingMessage,
    secret::{AppToken, BotToken},
};

const SLACK_API_BASE: &str = "https://slack.com/api/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(30);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackIdentity {
    pub team_id: String,
    pub team: String,
    pub enterprise_id: Option<String>,
    pub bot_user_id: Option<String>,
    /// The bot's `B…` id; messages it posts carry it as `bot_id`.
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedMessage {
    pub channel: String,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlackError {
    #[error("Slack rate limited Yard; retrying after {} s", .0.as_secs())]
    RateLimited(Duration),
    /// The token was rejected; Yard drops it and re-reads the secret.
    #[error("Slack rejected the bot token ({0}); Yard will re-read the secret")]
    InvalidAuth(String),
    #[error("Slack returned `{0}`")]
    Api(String),
    #[error("Slack could not be reached: {0}")]
    Transport(String),
    #[error("Slack returned an unreadable response")]
    Decode,
}

impl SlackError {
    fn from_code(code: &str) -> Self {
        match code {
            "invalid_auth" | "not_authed" | "token_revoked" | "token_expired"
            | "account_inactive" => Self::InvalidAuth(code.to_owned()),
            _ => Self::Api(code.to_owned()),
        }
    }
}

#[derive(Clone)]
pub struct SlackApi {
    http: reqwest::Client,
    base: String,
}

impl SlackApi {
    /// The production client for `https://slack.com/api/`.
    ///
    /// # Panics
    ///
    /// Panics only if the TLS stack cannot initialise, which is a build
    /// defect (the rustls ring provider is compiled in).
    #[must_use]
    pub fn production() -> Self {
        Self::build(SLACK_API_BASE.to_owned(), false)
    }

    /// A client for a local mock server (tests and debug-build rehearsals).
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn with_base_url(base: impl Into<String>) -> Self {
        Self::build(base.into(), true)
    }

    fn build(base: String, local: bool) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("yard/", env!("CARGO_PKG_VERSION")));
        if local {
            builder = builder.no_proxy();
        } else {
            builder = builder.https_only(true);
        }
        Self {
            http: builder.build().expect("TLS client configuration is valid"),
            base,
        }
    }

    /// `auth.test`: learn the team and enterprise the token belongs to.
    ///
    /// # Errors
    ///
    /// Returns [`SlackError`] on transport, auth or API failures.
    pub async fn auth_test(&self, token: &BotToken) -> Result<SlackIdentity, SlackError> {
        let body = self.call(token, "auth.test", &json!({})).await?;
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(SlackIdentity {
            team_id: text("team_id").ok_or(SlackError::Decode)?,
            team: text("team").unwrap_or_default(),
            enterprise_id: text("enterprise_id").filter(|id| !id.is_empty()),
            bot_user_id: text("user_id"),
            bot_id: text("bot_id").filter(|id| !id.is_empty()),
        })
    }

    /// `conversations.open` with the owner: the bot↔owner DM channel id.
    ///
    /// # Errors
    ///
    /// Returns [`SlackError`] on transport, auth or API failures.
    pub async fn open_dm(&self, token: &BotToken, user_id: &str) -> Result<String, SlackError> {
        let body = self
            .call(token, "conversations.open", &json!({ "users": user_id }))
            .await?;
        body.pointer("/channel/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(SlackError::Decode)
    }

    /// `chat.postMessage`, optionally as a broadcast reply in a thread.
    ///
    /// # Errors
    ///
    /// Returns [`SlackError`] on transport, auth or API failures.
    pub async fn post_message(
        &self,
        token: &BotToken,
        channel: &str,
        message: &OutgoingMessage,
        thread_ts: Option<&str>,
    ) -> Result<PostedMessage, SlackError> {
        let mut body = json!({
            "channel": channel,
            "text": message.text,
            "blocks": message.blocks,
            "unfurl_links": false,
            "unfurl_media": false,
        });
        if let Some(thread_ts) = thread_ts {
            body["thread_ts"] = json!(thread_ts);
            body["reply_broadcast"] = json!(true);
        }
        self.post_body(token, channel, &body).await
    }

    /// `chat.postMessage` as a quiet reply in `thread_ts` (no broadcast):
    /// answers to the owner's own Slack messages and button clicks.
    ///
    /// # Errors
    ///
    /// Returns [`SlackError`] on transport, auth or API failures.
    pub async fn post_thread_reply(
        &self,
        token: &BotToken,
        channel: &str,
        message: &OutgoingMessage,
        thread_ts: &str,
    ) -> Result<PostedMessage, SlackError> {
        let body = json!({
            "channel": channel,
            "text": message.text,
            "blocks": message.blocks,
            "thread_ts": thread_ts,
            "unfurl_links": false,
            "unfurl_media": false,
        });
        self.post_body(token, channel, &body).await
    }

    /// `chat.update`: replace a message Yard posted (collapse an answered
    /// prompt card). Edits do not notify, so outcomes are also posted as a
    /// new reply.
    ///
    /// # Errors
    ///
    /// Returns [`SlackError`] on transport, auth or API failures.
    pub async fn update_message(
        &self,
        token: &BotToken,
        channel: &str,
        ts: &str,
        message: &OutgoingMessage,
    ) -> Result<(), SlackError> {
        let body = json!({
            "channel": channel,
            "ts": ts,
            "text": message.text,
            "blocks": message.blocks,
        });
        self.call(token, "chat.update", &body).await.map(|_| ())
    }

    /// `apps.connections.open` with the app-level token: a one-time
    /// WebSocket URL for Socket Mode.
    ///
    /// # Errors
    ///
    /// Returns [`SlackError`] on transport, auth or API failures.
    pub async fn open_socket_url(&self, token: &AppToken) -> Result<String, SlackError> {
        let body = self
            .call_with_secret(token.expose(), "apps.connections.open", &json!({}))
            .await?;
        body.get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(SlackError::Decode)
    }

    async fn post_body(
        &self,
        token: &BotToken,
        channel: &str,
        body: &Value,
    ) -> Result<PostedMessage, SlackError> {
        let response = self.call(token, "chat.postMessage", body).await?;
        Ok(PostedMessage {
            channel: response
                .get("channel")
                .and_then(Value::as_str)
                .unwrap_or(channel)
                .to_owned(),
            ts: response
                .get("ts")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(SlackError::Decode)?,
        })
    }

    async fn call(
        &self,
        token: &BotToken,
        method: &str,
        body: &Value,
    ) -> Result<Value, SlackError> {
        self.call_with_secret(token.expose(), method, body).await
    }

    async fn call_with_secret(
        &self,
        secret: &str,
        method: &str,
        body: &Value,
    ) -> Result<Value, SlackError> {
        // Sized up front so building it never reallocates and leaves an
        // unzeroized copy behind. `HeaderValue` keeps its own copy, which
        // cannot be zeroized; it is marked sensitive and dropped per request.
        let mut bearer = Zeroizing::new(String::with_capacity(7 + secret.len()));
        bearer.push_str("Bearer ");
        bearer.push_str(secret);
        let mut authorization = HeaderValue::from_str(&bearer)
            .map_err(|_| SlackError::InvalidAuth("malformed_token".to_owned()))?;
        authorization.set_sensitive(true);
        let payload = serde_json::to_vec(body).map_err(|_| SlackError::Decode)?;
        let mut response = self
            .http
            .post(format!("{}{method}", self.base))
            .header(AUTHORIZATION, authorization)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .body(payload)
            .send()
            .await
            .map_err(|error| SlackError::Transport(transport_reason(&error)))?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map_or(DEFAULT_RETRY_AFTER, Duration::from_secs)
                .min(MAX_RETRY_AFTER);
            return Err(SlackError::RateLimited(retry_after));
        }
        if !response.status().is_success() {
            return Err(SlackError::Transport(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| SlackError::Transport(transport_reason(&error)))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(SlackError::Decode);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| SlackError::Decode)?;
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(value)
        } else {
            let code = value
                .get("error")
                .and_then(Value::as_str)
                .filter(|code| {
                    code.len() <= 64
                        && code.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit()
                        })
                })
                .unwrap_or("unknown_error");
            Err(SlackError::from_code(code))
        }
    }
}

/// A short, token-free reason for a transport failure.
fn transport_reason(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "timed out".to_owned()
    } else if error.is_connect() {
        "connection failed".to_owned()
    } else if error.is_body() || error.is_decode() {
        "response interrupted".to_owned()
    } else {
        "request failed".to_owned()
    }
}

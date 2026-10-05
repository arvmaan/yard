//! Slack DM notifications (phase 1) and inbound Socket Mode (phase 2).
//!
//! One owner, one DM. Outbound: a bot token with `chat:write` and
//! `im:write`. Inbound ([`hub`], off unless `YARD_SLACK_INBOUND=on`): an
//! app-level token opens a Socket Mode WebSocket ([`socket`]); only the
//! owner's fresh DMs and button clicks pass [`inbound`], and answers reach
//! agents only through Yard's guarded paths ([`actions`], [`console`]);
//! status commands answer from durable state ([`commands`]) and questions go
//! to an orchestrator with its status report relayed back ([`relay`]). The notifier
//! runs inside yard-server as a supervised background task that never
//! returns. It reads durable state every [`TICK`], derives attention with
//! [`detector`], rate-limits and digests with [`outbox`], and posts through
//! [`client`]. The token comes from Secrets Manager via the AWS CLI
//! ([`secret`]) and is held only in memory.

pub mod actions;
pub mod ask;
pub mod audit;
pub mod blocks;
pub mod cards;
pub mod client;
pub mod commands;
pub mod console;
pub mod controls;
pub mod detector;
pub mod digest;
pub mod held;
pub mod inbound;
pub mod message;
pub mod outbox;
pub mod policy;
pub mod presence;
pub mod prompt;
pub mod quiet;
pub mod relay;
pub mod secret;
pub(crate) mod socket;
pub mod threads;
pub mod views;

mod hub;

#[cfg(test)]
mod quiet_tests;
#[cfg(test)]
mod socket_tests;
#[cfg(test)]
pub(crate) mod tests;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tracing::{debug, info, warn};
use yard_store::YardStore;

use crate::config::{SlackConfig, SlackSettings};
use client::{SlackApi, SlackError, SlackIdentity};
use detector::{Detector, DetectorTiming};
use message::{OutgoingMessage, batch_message, test_message};
use outbox::{Outbox, ThreadKey};
use policy::{Policy, QuietParts};
use presence::ViewerPresence;
use secret::{BotToken, SecretError, SecretSource};
use threads::ThreadStore;

/// How often durable state is diffed.
pub const TICK: Duration = Duration::from_secs(2);
/// Slack allows about one message per second per channel.
const CHANNEL_MIN_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_RETRY_BASE: Duration = Duration::from_secs(30);
const CONNECT_RETRY_CAP: Duration = Duration::from_secs(10 * 60);
/// A failed post (other than 429) waits this long, doubling per consecutive
/// failure up to [`DELIVERY_RETRY_CAP`].
const DELIVERY_RETRY_BASE: Duration = Duration::from_secs(30);
const DELIVERY_RETRY_CAP: Duration = Duration::from_secs(10 * 60);
/// `chat.postMessage` errors that retrying the same post cannot fix: the
/// session is dropped so the next attempt reconnects (auth.test and
/// conversations.open) and the status reads `misconfigured`.
const PERMANENT_POST_ERRORS: &[&str] = &[
    "channel_not_found",
    "not_in_channel",
    "is_archived",
    "missing_scope",
    "not_allowed_token_type",
    "cannot_dm_bot",
    "user_not_found",
    "user_disabled",
    "restricted_action",
    "team_access_not_granted",
    "no_permission",
    "ekm_access_denied",
];
const MISCONFIGURED_RETRY: Duration = Duration::from_secs(10 * 60);
const STORE_RETRY_BASE: Duration = Duration::from_secs(5);
const STORE_RETRY_CAP: Duration = Duration::from_secs(60);
const PANIC_BACKOFF: Duration = Duration::from_secs(30);
const TEST_COOLDOWN: Duration = Duration::from_secs(5);
const TEST_TIMEOUT: Duration = Duration::from_secs(45);
/// A repeated, unchanged failure is logged at WARN at most this often.
const WARN_REPEAT_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackStatusKind {
    Off,
    Misconfigured,
    /// Enabled; the first connection attempt has not finished yet.
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackTeam {
    pub id: String,
    pub name: String,
    pub enterprise_id: Option<String>,
}

/// `GET /api/v1/integrations/slack`. Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackIntegrationStatus {
    pub enabled: bool,
    pub status: SlackStatusKind,
    pub team: Option<SlackTeam>,
    pub last_error: Option<String>,
    /// Unix milliseconds of the last message Slack accepted.
    pub last_sent_at: Option<u64>,
    /// `true` when only a change to the `YARD_SLACK_*` environment and a
    /// restart can help (off, or invalid settings). `false` for problems Yard
    /// found at runtime (secret contents, enterprise, Slack errors): fixing
    /// the secret needs no restart and "Send test message" retries now.
    pub restart_required: bool,
    /// Inbound Socket Mode (phase 2).
    pub inbound: hub::InboundStatusView,
    /// Quiet hours, held notifications and mute (enabled only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet: Option<policy::QuietStatusView>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlackTestError {
    #[error("Slack notifications are off; set YARD_SLACK_NOTIFICATIONS=on and restart Yard")]
    Off,
    #[error("Slack notifications are misconfigured: {0}")]
    Misconfigured(String),
    #[error("A test message was sent moments ago; wait a few seconds")]
    RateLimited,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone)]
pub struct SlackNotifier {
    inner: Arc<Inner>,
}

struct Inner {
    config: SlackConfig,
    store: Option<Arc<dyn YardStore>>,
    presence: ViewerPresence,
    status: Mutex<StatusState>,
    detector: Mutex<Detector>,
    outbox: Mutex<Outbox>,
    policy: Mutex<Policy>,
    delivery: tokio::sync::Mutex<Delivery>,
    logs: Mutex<LogThrottle>,
    store_failures: Mutex<u32>,
    delivery_failures: Mutex<u32>,
    inbound: hub::InboundState,
}

#[derive(Debug, Default)]
struct StatusState {
    status: Option<SlackStatusKind>,
    team: Option<SlackTeam>,
    last_error: Option<String>,
    last_sent_at: Option<u64>,
    last_test_at: Option<Instant>,
}

/// Everything that talks to Slack; one holder at a time.
struct Delivery {
    api: SlackApi,
    secrets: Option<SecretSource>,
    owner_user_id: String,
    enterprise_id: Option<String>,
    session: Option<Session>,
    threads: ThreadStore,
    last_post_at: Option<Instant>,
    connect_failures: u32,
    connect_not_before: Option<Instant>,
}

struct Session {
    token: BotToken,
    identity: SlackIdentity,
    channel: String,
}

#[derive(Debug)]
enum ConnectError {
    /// Backing off after an earlier failure; nothing new to report.
    Waiting,
    Secret(SecretError),
    Slack(SlackError),
    EnterpriseMismatch(String),
}

impl ConnectError {
    fn message(&self) -> String {
        match self {
            Self::Waiting => "Waiting to retry the Slack connection".to_owned(),
            Self::Secret(error) => format!("Reading the bot token failed: {error}"),
            Self::Slack(error) => error.to_string(),
            Self::EnterpriseMismatch(message) => message.clone(),
        }
    }

    const fn misconfigured(&self) -> bool {
        match self {
            Self::Secret(error) => error.is_misconfiguration(),
            Self::EnterpriseMismatch(_) => true,
            Self::Waiting | Self::Slack(_) => false,
        }
    }
}

/// Test seams: a local Slack API, a fake `aws`, fast settle windows.
pub(crate) struct NotifierParts {
    pub api: SlackApi,
    pub aws_binary: std::ffi::OsString,
    pub thread_file: Option<PathBuf>,
    pub timing: DetectorTiming,
    pub min_interval: Duration,
    pub inbound: hub::InboundParts,
    /// Quiet hours, digests and mute ([`policy`]).
    pub quiet: QuietParts,
}

impl SlackNotifier {
    /// A notifier that never sends (notifications off, or no store).
    #[must_use]
    pub fn off() -> Self {
        Self::build(
            SlackConfig::Off,
            None,
            None,
            NotifierParts::production(None),
        )
    }

    /// The production notifier for `config`.
    #[must_use]
    pub fn new(config: SlackConfig, store: Arc<dyn YardStore>, thread_file: PathBuf) -> Self {
        let audit_file = audit::audit_file_path(&thread_file);
        let mut parts = NotifierParts::production(Some(thread_file));
        parts.inbound.audit_file = Some(audit_file);
        Self::build(config, Some(store), None, parts)
    }

    #[cfg(test)]
    pub(crate) fn with_parts(
        config: SlackConfig,
        store: Arc<dyn YardStore>,
        parts: NotifierParts,
    ) -> Self {
        Self::build(config, Some(store), None, parts)
    }

    fn build(
        config: SlackConfig,
        store: Option<Arc<dyn YardStore>>,
        presence: Option<ViewerPresence>,
        parts: NotifierParts,
    ) -> Self {
        let settings = match &config {
            SlackConfig::Enabled(settings) => Some(settings.clone()),
            SlackConfig::Off | SlackConfig::Misconfigured(_) => None,
        };
        let secrets = settings.as_ref().map(|settings| {
            let mut source = SecretSource::from_settings(settings);
            source.aws_binary.clone_from(&parts.aws_binary);
            source
        });
        let inbound = hub::InboundState::new(settings.as_ref(), &parts);
        let mut quiet = parts.quiet;
        if settings.is_none() {
            // Nothing is sent, so nothing is held on disk.
            quiet.held_file = None;
            quiet.preferences_file = None;
        }
        let delivery = Delivery {
            api: parts.api,
            secrets,
            owner_user_id: settings
                .as_ref()
                .map(|settings| settings.owner_user_id.clone())
                .unwrap_or_default(),
            enterprise_id: settings
                .as_ref()
                .and_then(|settings| settings.enterprise_id.clone()),
            session: None,
            threads: parts
                .thread_file
                .filter(|_| settings.is_some())
                .map_or_else(ThreadStore::default, ThreadStore::load),
            last_post_at: None,
            connect_failures: 0,
            connect_not_before: None,
        };
        Self {
            inner: Arc::new(Inner {
                config,
                store,
                presence: presence.unwrap_or_default(),
                status: Mutex::default(),
                detector: Mutex::new(Detector::new(parts.timing, unix_ms())),
                outbox: Mutex::new(Outbox::with_interval(parts.min_interval)),
                policy: Mutex::new(Policy::new(quiet)),
                delivery: tokio::sync::Mutex::new(delivery),
                logs: Mutex::default(),
                store_failures: Mutex::new(0),
                delivery_failures: Mutex::new(0),
                inbound,
            }),
        }
    }

    /// The registry the HTTP layer updates when a browser opens an agent.
    #[must_use]
    pub fn presence(&self) -> ViewerPresence {
        self.inner.presence.clone()
    }

    fn settings(&self) -> Option<&SlackSettings> {
        match &self.inner.config {
            SlackConfig::Enabled(settings) if self.inner.store.is_some() => Some(settings),
            _ => None,
        }
    }

    #[must_use]
    pub fn status(&self) -> SlackIntegrationStatus {
        match &self.inner.config {
            SlackConfig::Off => SlackIntegrationStatus {
                enabled: false,
                status: SlackStatusKind::Off,
                team: None,
                last_error: None,
                last_sent_at: None,
                restart_required: true,
                inbound: hub::InboundStatusView::off(),
                quiet: None,
            },
            SlackConfig::Misconfigured(reason) => SlackIntegrationStatus {
                enabled: true,
                status: SlackStatusKind::Misconfigured,
                team: None,
                last_error: Some(reason.clone()),
                last_sent_at: None,
                restart_required: true,
                inbound: hub::InboundStatusView::off(),
                quiet: None,
            },
            SlackConfig::Enabled(_) => {
                let state = lock(&self.inner.status);
                SlackIntegrationStatus {
                    enabled: true,
                    status: state.status.unwrap_or(SlackStatusKind::Connecting),
                    team: state.team.clone(),
                    last_error: state.last_error.clone(),
                    last_sent_at: state.last_sent_at,
                    restart_required: false,
                    inbound: self.inbound_status(),
                    quiet: Some(lock(&self.inner.policy).status()),
                }
            }
        }
    }

    /// Supervised loop. It never returns: a returning background task stops
    /// Yard. Each tick runs as its own task so a panic is logged and the loop
    /// continues after a pause.
    pub async fn run(self) {
        let Some(settings) = self.settings() else {
            if let SlackConfig::Misconfigured(reason) = &self.inner.config {
                warn!(%reason, "Slack notifications are misconfigured; nothing will be sent");
            }
            std::future::pending::<()>().await;
            return;
        };
        info!(
            owner = %settings.owner_user_id,
            secret = %settings.secret_id,
            "Slack notifications enabled"
        );
        let inbound = self.clone();
        tokio::spawn(async move {
            loop {
                let notifier = inbound.clone();
                if let Err(error) = tokio::spawn(notifier.run_inbound()).await {
                    warn!(%error, "Inbound Slack failed; restarting after a pause");
                }
                tokio::time::sleep(PANIC_BACKOFF).await;
            }
        });
        loop {
            let notifier = self.clone();
            let delay = match tokio::spawn(async move { notifier.tick().await }).await {
                Ok(delay) => delay,
                Err(error) => {
                    warn!(%error, "Slack notifier tick failed; continuing after a pause");
                    PANIC_BACKOFF
                }
            };
            tokio::time::sleep(delay).await;
        }
    }

    /// One pass: read durable state, detect, and deliver at most one message.
    /// Returns how long to wait before the next pass.
    pub(crate) async fn tick(&self) -> Duration {
        let Some(store) = self.inner.store.clone() else {
            return PANIC_BACKOFF;
        };
        let now = Instant::now();
        let now_unix_ms = unix_ms();
        let after = lock(&self.inner.detector).commands_query_after();
        match store.attention_records(after).await {
            Ok(records) => {
                *lock(&self.inner.store_failures) = 0;
                let presence = &self.inner.presence;
                let observation =
                    lock(&self.inner.detector).observe(&records, now, now_unix_ms, |target| {
                        presence.is_viewing(target, now)
                    });
                if observation.suppressed > 0 {
                    debug!(
                        suppressed = observation.suppressed,
                        "Slack notification suppressed while the agent is open in Yard"
                    );
                }
                self.admit(observation.events).await;
            }
            Err(error) => {
                let failures = {
                    let mut failures = lock(&self.inner.store_failures);
                    *failures = failures.saturating_add(1);
                    *failures
                };
                self.log_failure("store", &format!("Reading attention state failed: {error}"));
                return backoff(STORE_RETRY_BASE, STORE_RETRY_CAP, failures);
            }
        }

        let mut delivery = self.inner.delivery.lock().await;
        match delivery.ensure_session(Instant::now()).await {
            // A fresh session clears an earlier connect error; an existing
            // one leaves a delivery error visible until a message succeeds.
            Ok(true) => self.record_connected(&delivery),
            Ok(false) => {}
            Err(error) => {
                self.record_connect_error(&error);
                return TICK;
            }
        }
        {
            // Quiet hours or a mute began while events waited for the
            // global interval: they wait for the digest instead.
            let mut policy = lock(&self.inner.policy);
            if policy.holding(policy.now()) {
                let (queued, dropped) = lock(&self.inner.outbox).take_all();
                if !queued.is_empty() || dropped > 0 {
                    policy.hold_all(queued, dropped);
                }
            }
        }
        let batch = {
            let detector = lock(&self.inner.detector);
            let presence = &self.inner.presence;
            lock(&self.inner.outbox).next_batch(Instant::now(), |event| {
                detector.still_true(event)
                    && !event
                        .view_target
                        .as_ref()
                        .is_some_and(|target| presence.is_viewing(target, Instant::now()))
            })
        };
        let Some(batch) = batch else {
            self.deliver_digest(&mut delivery).await;
            return TICK;
        };
        let message = batch_message(
            &batch,
            message::CardOptions {
                ui_url: &self.inner.inbound.ui_url,
                inbound: self.inbound_enabled(),
            },
        );
        match delivery
            .post(Some(&batch.thread), &message, unix_ms())
            .await
        {
            Ok(root) => {
                *lock(&self.inner.delivery_failures) = 0;
                lock(&self.inner.outbox).sent(Instant::now());
                self.record_sent();
                drop(delivery);
                self.follow_with_prompt_cards(&batch, root);
            }
            Err(error) => {
                let failures = {
                    let mut failures = lock(&self.inner.delivery_failures);
                    *failures = failures.saturating_add(1);
                    *failures
                };
                let not_before = Instant::now() + delivery_retry_delay(&error, failures);
                lock(&self.inner.outbox).retry(batch, not_before);
                self.record_delivery_error(&error);
            }
        }
        TICK
    }

    /// Send one test DM now (the Settings "Send test message" button).
    ///
    /// # Errors
    ///
    /// Returns [`SlackTestError`] when notifications are off or
    /// misconfigured, a test was just sent, or Slack delivery fails.
    pub async fn send_test(&self) -> Result<SlackIntegrationStatus, SlackTestError> {
        match &self.inner.config {
            SlackConfig::Off => return Err(SlackTestError::Off),
            SlackConfig::Misconfigured(reason) => {
                return Err(SlackTestError::Misconfigured(reason.clone()));
            }
            SlackConfig::Enabled(_) if self.inner.store.is_none() => {
                return Err(SlackTestError::Off);
            }
            SlackConfig::Enabled(_) => {}
        }
        {
            let mut status = lock(&self.inner.status);
            if status
                .last_test_at
                .is_some_and(|last| last.elapsed() < TEST_COOLDOWN)
            {
                return Err(SlackTestError::RateLimited);
            }
            status.last_test_at = Some(Instant::now());
        }
        let attempt = async {
            let mut delivery = self.inner.delivery.lock().await;
            // The owner asked explicitly, so skip any connect backoff.
            delivery.connect_not_before = None;
            match delivery.ensure_session(Instant::now()).await {
                Ok(true) => self.record_connected(&delivery),
                Ok(false) => {}
                Err(error) => {
                    self.record_connect_error(&error);
                    return Err(SlackTestError::Failed(error.message()));
                }
            }
            match delivery
                .post(None, &test_message(unix_ms()), unix_ms())
                .await
            {
                Ok(_) => {
                    self.record_sent();
                    Ok(self.status())
                }
                Err(error) => {
                    self.record_delivery_error(&error);
                    Err(SlackTestError::Failed(error.to_string()))
                }
            }
        };
        tokio::time::timeout(TEST_TIMEOUT, attempt)
            .await
            .unwrap_or_else(|_| {
                Err(SlackTestError::Failed(
                    "Slack did not answer in time; try again".to_owned(),
                ))
            })
    }

    fn record_connected(&self, delivery: &Delivery) {
        let Some(session) = &delivery.session else {
            return;
        };
        let mut state = lock(&self.inner.status);
        let recovered = state.status != Some(SlackStatusKind::Connected);
        state.status = Some(SlackStatusKind::Connected);
        state.last_error = None;
        state.team = Some(SlackTeam {
            id: session.identity.team_id.clone(),
            name: session.identity.team.clone(),
            enterprise_id: session.identity.enterprise_id.clone(),
        });
        drop(state);
        if recovered {
            info!(team = %session.identity.team_id, "Slack notifier connected");
            lock(&self.inner.logs).recovered("connect");
        }
    }

    fn record_connect_error(&self, error: &ConnectError) {
        if matches!(error, ConnectError::Waiting) {
            return;
        }
        let message = error.message();
        {
            let mut state = lock(&self.inner.status);
            state.status = Some(if error.misconfigured() {
                SlackStatusKind::Misconfigured
            } else {
                SlackStatusKind::Error
            });
            state.last_error = Some(message.clone());
        }
        self.log_failure("connect", &message);
    }

    fn record_delivery_error(&self, error: &SlackError) {
        let message = error.to_string();
        {
            let mut state = lock(&self.inner.status);
            state.status = Some(if is_permanent_post_error(error) {
                SlackStatusKind::Misconfigured
            } else {
                SlackStatusKind::Error
            });
            state.last_error = Some(message.clone());
        }
        self.log_failure("deliver", &message);
    }

    fn record_sent(&self) {
        let mut state = lock(&self.inner.status);
        state.status = Some(SlackStatusKind::Connected);
        state.last_error = None;
        state.last_sent_at = Some(unix_ms());
    }

    fn log_failure(&self, key: &'static str, message: &str) {
        let decision = lock(&self.inner.logs).observe(key, message, Instant::now());
        if let Some(suppressed) = decision {
            warn!(
                error = %message,
                suppressed_repeats = suppressed,
                "Slack notifier problem; it keeps retrying"
            );
        } else {
            debug!(error = %message, "Slack notifier problem repeated");
        }
    }
}

/// Debug builds only: point the Web API and Socket Mode at a fake Slack on
/// loopback for isolated rehearsals. Release builds never read it.
#[cfg(debug_assertions)]
const TEST_ENDPOINT_VAR: &str = "YARD_SLACK_TEST_ENDPOINT";

/// The Web API base for `http://127.0.0.1:<port>` (optional trailing `/`);
/// any other host, scheme, path or port is refused.
#[cfg(debug_assertions)]
fn loopback_endpoint(value: &str) -> Option<String> {
    let rest = value.strip_prefix("http://127.0.0.1:")?;
    let port = rest.strip_suffix('/').unwrap_or(rest);
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    Some(format!("http://127.0.0.1:{port}/api/"))
}

impl NotifierParts {
    fn production(thread_file: Option<PathBuf>) -> Self {
        let (relay, relay_warning) =
            relay::RelayTiming::from_env(std::env::var(relay::ASK_TIMEOUT_VAR).ok().as_deref());
        if let Some(warning) = relay_warning {
            warn!("{warning}");
        }
        let (config, warnings) = quiet::QuietConfig::from_lookup(|name| std::env::var(name).ok());
        for warning in warnings {
            warn!("{warning}");
        }
        let quiet = QuietParts {
            config,
            noise: policy::NoiseConfig::default(),
            clock: policy::system_clock(),
            held_file: thread_file
                .as_deref()
                .map(|path| held::sibling(path, held::HELD_FILE)),
            preferences_file: thread_file
                .as_deref()
                .map(|path| held::sibling(path, held::PREFERENCES_FILE)),
        };
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut parts = Self {
            api: SlackApi::production(),
            aws_binary: "aws".into(),
            thread_file,
            timing: DetectorTiming::default(),
            min_interval: outbox::GLOBAL_MIN_INTERVAL,
            inbound: hub::InboundParts {
                ui_url: blocks::ui_url(std::env::var(blocks::UI_URL_VAR).ok().as_deref()),
                relay,
                ..hub::InboundParts::default()
            },
            quiet,
        };
        #[cfg(debug_assertions)]
        if let Some(value) = std::env::var_os(TEST_ENDPOINT_VAR) {
            if let Some(base) = value.to_str().and_then(loopback_endpoint) {
                warn!(
                    endpoint = %base,
                    "{TEST_ENDPOINT_VAR} is set: Slack traffic goes to a local fake Slack (debug build rehearsal only)"
                );
                parts.api = SlackApi::with_base_url(base);
                parts.inbound.url_policy = socket::UrlPolicy::Loopback;
            } else {
                warn!(
                    "{TEST_ENDPOINT_VAR} is not http://127.0.0.1:<port>; ignored, using slack.com"
                );
            }
        }
        parts
    }
}

impl Delivery {
    /// Connect if needed; `Ok(true)` means a new session was just opened.
    async fn ensure_session(&mut self, now: Instant) -> Result<bool, ConnectError> {
        if self.session.is_some() {
            return Ok(false);
        }
        if self
            .connect_not_before
            .is_some_and(|not_before| now < not_before)
        {
            return Err(ConnectError::Waiting);
        }
        match self.connect().await {
            Ok(session) => {
                self.session = Some(session);
                self.connect_failures = 0;
                self.connect_not_before = None;
                Ok(true)
            }
            Err(error) => {
                self.connect_failures = self.connect_failures.saturating_add(1);
                let delay = match &error {
                    error if error.misconfigured() => MISCONFIGURED_RETRY,
                    ConnectError::Slack(SlackError::RateLimited(retry_after)) => *retry_after,
                    _ => backoff(CONNECT_RETRY_BASE, CONNECT_RETRY_CAP, self.connect_failures),
                };
                self.connect_not_before = Some(now + delay);
                Err(error)
            }
        }
    }

    async fn connect(&self) -> Result<Session, ConnectError> {
        let secrets = self.secrets.as_ref().ok_or(ConnectError::Waiting)?;
        let token = secrets.fetch().await.map_err(ConnectError::Secret)?;
        let identity = self
            .api
            .auth_test(&token)
            .await
            .map_err(ConnectError::Slack)?;
        if let Some(expected) = &self.enterprise_id
            && identity.enterprise_id.as_deref() != Some(expected.as_str())
        {
            return Err(ConnectError::EnterpriseMismatch(format!(
                "The bot token belongs to enterprise {}, not YARD_SLACK_ENTERPRISE_ID {expected}; nothing will be sent",
                identity.enterprise_id.as_deref().unwrap_or("(none)")
            )));
        }
        let channel = self
            .api
            .open_dm(&token, &self.owner_user_id)
            .await
            .map_err(ConnectError::Slack)?;
        Ok(Session {
            token,
            identity,
            channel,
        })
    }

    /// Post into `thread` (or top level); returns the thread's root ts. A
    /// rejected token drops the session
    /// so the next pass re-reads the secret, after a backoff; a permanent
    /// error ([`PERMANENT_POST_ERRORS`]) drops it too, so the next pass
    /// reconnects instead of re-posting into the same failure.
    async fn post(
        &mut self,
        thread: Option<&ThreadKey>,
        message: &OutgoingMessage,
        now_unix_ms: u64,
    ) -> Result<String, SlackError> {
        if let Some(last) = self.last_post_at {
            let wait = CHANNEL_MIN_INTERVAL.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let Some(session) = &self.session else {
            return Err(SlackError::Transport("not connected".to_owned()));
        };
        let team_id = session.identity.team_id.clone();
        let channel = session.channel.clone();
        let key = thread.map(ThreadKey::storage_key);
        let thread_ts = key
            .as_deref()
            .and_then(|key| self.threads.get(&team_id, &channel, key))
            .map(str::to_owned);
        let mut root = thread_ts.clone();
        let mut result = self
            .api
            .post_message(&session.token, &channel, message, thread_ts.as_deref())
            .await;
        if let (Some(key), Some(_), Err(SlackError::Api(code))) = (&key, &thread_ts, &result)
            && matches!(code.as_str(), "thread_not_found" | "message_not_found")
        {
            // The root message is gone; start a new thread.
            root = None;
            self.threads.forget(&team_id, &channel, key);
            result = self
                .api
                .post_message(&session.token, &channel, message, None)
                .await;
            if let Ok(posted) = &result {
                self.threads
                    .put(&team_id, &channel, key, &posted.ts, now_unix_ms);
            }
        } else if let (Some(key), None, Ok(posted)) = (&key, &thread_ts, &result) {
            self.threads
                .put(&team_id, &channel, key, &posted.ts, now_unix_ms);
        }
        self.last_post_at = Some(Instant::now());
        match result {
            Ok(posted) => Ok(root.unwrap_or(posted.ts)),
            Err(error) => {
                if matches!(error, SlackError::InvalidAuth(_)) || is_permanent_post_error(&error) {
                    self.session = None;
                    self.connect_failures = self.connect_failures.saturating_add(1);
                    self.connect_not_before = Some(
                        Instant::now()
                            + backoff(CONNECT_RETRY_BASE, CONNECT_RETRY_CAP, self.connect_failures),
                    );
                }
                Err(error)
            }
        }
    }
}

/// WARN on the first failure, when the message changes, and then at most
/// every [`WARN_REPEAT_INTERVAL`]; the rest go to DEBUG.
#[derive(Default)]
struct LogThrottle {
    entries: HashMap<&'static str, (String, Instant, u32)>,
}

impl LogThrottle {
    fn observe(&mut self, key: &'static str, message: &str, now: Instant) -> Option<u32> {
        if let Some((last, warned_at, suppressed)) = self.entries.get_mut(key) {
            if last == message && now.saturating_duration_since(*warned_at) < WARN_REPEAT_INTERVAL {
                *suppressed = suppressed.saturating_add(1);
                return None;
            }
            let count = *suppressed;
            message.clone_into(last);
            *warned_at = now;
            *suppressed = 0;
            return Some(count);
        }
        self.entries.insert(key, (message.to_owned(), now, 0));
        Some(0)
    }

    fn recovered(&mut self, key: &'static str) {
        self.entries.remove(key);
    }
}

/// How long a failed post waits: Slack's `Retry-After` for 429, otherwise
/// 30 s doubling per consecutive failure up to 10 min.
fn delivery_retry_delay(error: &SlackError, consecutive_failures: u32) -> Duration {
    match error {
        SlackError::RateLimited(retry_after) => *retry_after,
        _ => backoff(
            DELIVERY_RETRY_BASE,
            DELIVERY_RETRY_CAP,
            consecutive_failures,
        ),
    }
}

fn is_permanent_post_error(error: &SlackError) -> bool {
    matches!(error, SlackError::Api(code) if PERMANENT_POST_ERRORS.contains(&code.as_str()))
}

fn backoff(base: Duration, cap: Duration, failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    base.saturating_mul(1 << exponent).min(cap)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

//! Slack Socket Mode transport: an outbound WebSocket, no inbound endpoint.
//!
//! `apps.connections.open` (app-level token) returns a one-time `wss://` URL
//! on `*.slack.com`; Yard connects with rustls (ring, compiled-in Mozilla
//! roots) and reads frames. Every envelope with an `envelope_id` is
//! acknowledged the moment it is read — before it is parsed further or
//! queued — so the 3 s ack deadline never depends on the work it triggers.
//! Work goes to a bounded queue; when it is full the envelope is dropped
//! (already acked, so Slack does not retry it) and counted.
//!
//! `disconnect` envelopes (`refresh_requested`, `warning`, …) end the
//! session and a new URL is opened at once; transport errors, idle sockets
//! and failed opens back off (2 s doubling to 5 min). `link_disabled` means
//! Socket Mode is off for the app: retried every 10 min.

use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::{mpsc, watch},
    time::timeout,
};
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, pki_types::ServerName},
};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{
        Message,
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

use super::inbound::{Envelope, EnvelopeKind, parse_envelope};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// No frame at all (Slack pings regularly) for this long → reconnect.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub(crate) const RETRY_BASE: Duration = Duration::from_secs(2);
pub(crate) const RETRY_CAP: Duration = Duration::from_secs(5 * 60);
pub(crate) const LINK_DISABLED_RETRY: Duration = Duration::from_secs(10 * 60);

/// Any byte stream a WebSocket can run over (TLS in production).
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub(crate) type Socket = WebSocketStream<Box<dyn Io>>;

/// Which socket URLs Yard will dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UrlPolicy {
    /// `wss://` on `slack.com` or a subdomain, port 443.
    Slack,
    /// Tests and debug-build rehearsals only: plain `ws://127.0.0.1:<port>`.
    #[cfg(any(test, debug_assertions))]
    Loopback,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SocketError {
    #[error("Slack returned a Socket Mode URL Yard will not dial")]
    UntrustedUrl,
    #[error("Socket Mode connection failed: {0}")]
    Connect(String),
}

/// `(tls, host, port)` for an allowed URL.
fn target(url: &str, policy: UrlPolicy) -> Result<(bool, String, u16), SocketError> {
    let (tls, rest) = match policy {
        UrlPolicy::Slack => (true, url.strip_prefix("wss://")),
        #[cfg(any(test, debug_assertions))]
        UrlPolicy::Loopback => (false, url.strip_prefix("ws://")),
    };
    let rest = rest.ok_or(SocketError::UntrustedUrl)?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') || authority.is_empty() {
        return Err(SocketError::UntrustedUrl);
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>().map_err(|_| SocketError::UntrustedUrl)?,
        ),
        None if tls => (authority, 443),
        None => return Err(SocketError::UntrustedUrl),
    };
    let host = host.to_ascii_lowercase();
    let allowed = match policy {
        UrlPolicy::Slack => {
            port == 443
                && (host == "slack.com" || host.ends_with(".slack.com"))
                && host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        }
        #[cfg(any(test, debug_assertions))]
        UrlPolicy::Loopback => host == "127.0.0.1",
    };
    if !allowed {
        return Err(SocketError::UntrustedUrl);
    }
    Ok((tls, host, port))
}

fn tls_config() -> Result<Arc<ClientConfig>, SocketError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| SocketError::Connect(error.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Dial the socket URL. The URL carries a one-time ticket: it is never
/// logged or put in an error.
pub(crate) async fn connect(url: &str, policy: UrlPolicy) -> Result<Socket, SocketError> {
    let (tls, host, port) = target(url, policy)?;
    let attempt = async {
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|error| SocketError::Connect(error.kind().to_string()))?;
        let stream: Box<dyn Io> = if tls {
            let name = ServerName::try_from(host.clone()).map_err(|_| SocketError::UntrustedUrl)?;
            let stream = TlsConnector::from(tls_config()?)
                .connect(name, tcp)
                .await
                .map_err(|error| SocketError::Connect(format!("TLS: {}", error.kind())))?;
            Box::new(stream)
        } else {
            Box::new(tcp)
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_BYTES))
            .max_frame_size(Some(MAX_FRAME_BYTES));
        let (socket, _) = client_async_with_config(url, stream, Some(config))
            .await
            .map_err(|error| SocketError::Connect(handshake_error(&error)))?;
        Ok(socket)
    };
    timeout(CONNECT_TIMEOUT, attempt)
        .await
        .unwrap_or_else(|_| Err(SocketError::Connect("timed out".to_owned())))
}

/// A handshake error without the URL (it holds the ticket).
fn handshake_error(error: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Http(response) => format!("HTTP {}", response.status()),
        Error::Io(error) => error.kind().to_string(),
        _ => "handshake failed".to_owned(),
    }
}

/// Why one connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionEnd {
    /// Slack asked us to reconnect (`refresh_requested`, `warning`, …).
    Disconnect(String),
    Closed,
    Idle,
    Error(String),
    /// Yard is shutting down: the socket gets a close frame, no reconnect.
    Shutdown,
}

impl SessionEnd {
    /// Reconnect at once (a planned refresh) rather than after a backoff.
    pub(crate) fn immediate(&self) -> bool {
        matches!(self, Self::Disconnect(reason) if reason != "link_disabled")
    }
}

/// What the reader hands to the worker.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SocketEvent {
    Hello {
        num_connections: u64,
        app_id: Option<String>,
    },
    Envelope(Envelope),
}

/// Counters for one connection (tests and status).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionStats {
    pub acked: u64,
    pub dropped_queue_full: u64,
}

/// Read one connection until it ends: ack each envelope first, then queue
/// it for the worker (never waiting on the worker).
pub(crate) async fn run_session(
    socket: &mut Socket,
    queue: &mpsc::Sender<SocketEvent>,
    idle_timeout: Duration,
    stats: &mut SessionStats,
    closing: &mut watch::Receiver<bool>,
) -> SessionEnd {
    loop {
        let next = tokio::select! {
            biased;
            _ = closing.wait_for(|closing| *closing) => return SessionEnd::Shutdown,
            next = timeout(idle_timeout, socket.next()) => next,
        };
        let frame = match next {
            Err(_) => return SessionEnd::Idle,
            Ok(None) => return SessionEnd::Closed,
            Ok(Some(Err(error))) => return SessionEnd::Error(transport_error(&error)),
            Ok(Some(Ok(frame))) => frame,
        };
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => return SessionEnd::Closed,
            // tungstenite queues the pong; flushing sends it.
            Message::Ping(_) => {
                if let Err(error) = socket.flush().await {
                    return SessionEnd::Error(transport_error(&error));
                }
                continue;
            }
            _ => continue,
        };
        let Some(envelope) = parse_envelope(text.as_str()) else {
            continue;
        };
        if let Some(envelope_id) = &envelope.envelope_id {
            let ack = json!({ "envelope_id": envelope_id }).to_string();
            match timeout(ACK_TIMEOUT, socket.send(Message::Text(ack.into()))).await {
                Ok(Ok(())) => stats.acked += 1,
                Ok(Err(error)) => return SessionEnd::Error(transport_error(&error)),
                Err(_) => return SessionEnd::Error("ack timed out".to_owned()),
            }
        }
        let event = match &envelope.kind {
            EnvelopeKind::Hello {
                num_connections,
                app_id,
            } => SocketEvent::Hello {
                num_connections: *num_connections,
                app_id: app_id.clone(),
            },
            EnvelopeKind::Disconnect { reason } => return SessionEnd::Disconnect(reason.clone()),
            EnvelopeKind::EventsApi | EnvelopeKind::Interactive => SocketEvent::Envelope(envelope),
            EnvelopeKind::Other(_) => continue,
        };
        if queue.try_send(event).is_err() {
            stats.dropped_queue_full += 1;
        }
    }
}

fn transport_error(error: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::ConnectionClosed | Error::AlreadyClosed => "closed".to_owned(),
        Error::Io(error) => error.kind().to_string(),
        Error::Capacity(_) => "frame too large".to_owned(),
        Error::Protocol(error) => format!("protocol: {error}"),
        _ => "transport error".to_owned(),
    }
}

/// The close frame Yard sends when a session ends: `1000 normal` with a
/// reason on shutdown, so Slack drops the connection at once instead of
/// counting it in `num_connections` until it times out.
pub(crate) fn close_frame(end: &SessionEnd) -> Option<CloseFrame> {
    matches!(end, SessionEnd::Shutdown).then(|| CloseFrame {
        code: CloseCode::Normal,
        reason: "yard shutting down".into(),
    })
}

/// The wait before the next connection attempt.
pub(crate) fn retry_delay(end: Option<&SessionEnd>, failures: u32) -> Duration {
    match end {
        Some(end) if end.immediate() => Duration::ZERO,
        Some(SessionEnd::Disconnect(_)) => LINK_DISABLED_RETRY,
        _ => super::backoff(RETRY_BASE, RETRY_CAP, failures.max(1)),
    }
}

/// A session that stayed up this long was healthy: the backoff restarts.
pub(crate) const HEALTHY_SESSION: Duration = Duration::from_secs(60);

/// Consecutive failures after a session that ended with `end` after
/// `lasted`. A planned refresh resets them; a healthy session counts as the
/// first failure only, so one drop after a long quiet hour retries fast.
pub(crate) fn failures_after(failures: u32, end: &SessionEnd, lasted: Duration) -> u32 {
    if end.immediate() {
        0
    } else if lasted >= HEALTHY_SESSION {
        1
    } else {
        failures.saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        HEALTHY_SESSION, LINK_DISABLED_RETRY, RETRY_BASE, RETRY_CAP, SessionEnd, SocketError,
        UrlPolicy, failures_after, retry_delay, target,
    };

    #[test]
    fn a_healthy_session_restarts_the_backoff() {
        let short = Duration::from_secs(1);
        assert_eq!(failures_after(8, &SessionEnd::Closed, short), 9);
        assert_eq!(failures_after(8, &SessionEnd::Idle, HEALTHY_SESSION), 1);
        assert_eq!(
            retry_delay(
                Some(&SessionEnd::Closed),
                failures_after(8, &SessionEnd::Closed, HEALTHY_SESSION)
            ),
            retry_delay(Some(&SessionEnd::Closed), 1)
        );
        assert!(retry_delay(Some(&SessionEnd::Closed), 1) < Duration::from_secs(5));
        assert_eq!(
            failures_after(
                8,
                &SessionEnd::Disconnect("refresh_requested".into()),
                short
            ),
            0
        );
    }

    #[test]
    fn dials_only_wss_on_slack_com_port_443() {
        assert_eq!(
            target(
                "wss://wss-primary.slack.com/link/?ticket=x&app_id=A1",
                UrlPolicy::Slack
            ),
            Ok((true, "wss-primary.slack.com".to_owned(), 443))
        );
        assert!(target("wss://slack.com:443/link", UrlPolicy::Slack).is_ok());
        for url in [
            "ws://wss-primary.slack.com/link",
            "https://wss-primary.slack.com/link",
            "wss://wss-primary.slack.com.evil.test/link",
            "wss://evilslack.com/link",
            "wss://wss-primary.slack.com:8443/link",
            "wss://user@wss-primary.slack.com/link",
            "wss://127.0.0.1/link",
            "wss:///link",
        ] {
            assert_eq!(
                target(url, UrlPolicy::Slack),
                Err(SocketError::UntrustedUrl),
                "{url}"
            );
        }
        assert!(target("ws://127.0.0.1:9/link", UrlPolicy::Loopback).is_ok());
        assert!(target("ws://example.com:9/link", UrlPolicy::Loopback).is_err());
    }

    #[test]
    fn refresh_is_immediate_and_failures_back_off() {
        let refresh = SessionEnd::Disconnect("refresh_requested".to_owned());
        assert_eq!(retry_delay(Some(&refresh), 5), Duration::ZERO);
        let warning = SessionEnd::Disconnect("warning".to_owned());
        assert_eq!(retry_delay(Some(&warning), 1), Duration::ZERO);
        let disabled = SessionEnd::Disconnect("link_disabled".to_owned());
        assert_eq!(retry_delay(Some(&disabled), 1), LINK_DISABLED_RETRY);
        assert_eq!(retry_delay(Some(&SessionEnd::Closed), 1), RETRY_BASE);
        assert_eq!(retry_delay(None, 2), RETRY_BASE * 2);
        assert_eq!(retry_delay(Some(&SessionEnd::Idle), 30), RETRY_CAP);
    }
}

use std::{io, path::PathBuf, process::ExitStatus};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HerdrError {
    #[error("failed to execute Herdr session discovery: {0}")]
    DiscoveryIo(#[source] io::Error),
    #[error("Herdr session discovery timed out")]
    DiscoveryTimeout,
    #[error("Herdr session discovery {stream} exceeded {limit} bytes")]
    DiscoveryResponseTooLarge { stream: &'static str, limit: usize },
    #[error("Herdr session discovery failed with {status}: {stderr}")]
    DiscoveryFailed { status: ExitStatus, stderr: String },
    #[error("Herdr session discovery returned invalid JSON: {0}")]
    DiscoveryDecode(#[source] serde_json::Error),
    #[error("Herdr session '{0}' was not found")]
    SessionNotFound(String),
    #[error("Herdr session '{0}' is not running")]
    SessionNotRunning(String),
    #[error("failed to start the Herdr session server: {0}")]
    SessionStartIo(#[source] io::Error),
    #[error("Herdr session server exited during startup with {status}")]
    SessionStartFailed { status: ExitStatus },
    #[error("Herdr session '{0}' did not become ready before the startup timeout")]
    SessionStartTimeout(String),
    #[error("failed to connect to Herdr socket {path}: {source}")]
    SocketConnect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write Herdr request: {0}")]
    SocketWrite(#[source] io::Error),
    #[error("Herdr socket request timed out")]
    SocketTimeout,
    #[error("Herdr closed the socket before returning a response")]
    EmptyResponse,
    #[error("Herdr returned a response larger than {0} bytes")]
    ResponseTooLarge(usize),
    #[error("Herdr returned a response without a newline terminator")]
    UnterminatedResponse,
    #[error("Herdr returned invalid snapshot JSON: {0}")]
    SnapshotDecode(#[source] serde_json::Error),
    #[error("Herdr returned invalid command JSON: {0}")]
    CommandDecode(#[source] serde_json::Error),
    #[error("Herdr response ID '{actual}' did not match request ID '{expected}'")]
    ResponseIdMismatch { expected: String, actual: String },
    #[error("Herdr API returned {code}: {message}")]
    Api { code: String, message: String },
    #[error("Herdr response did not contain a session snapshot")]
    MissingSnapshot,
    #[error("Herdr response did not contain a command result")]
    MissingCommandResult,
    #[error("Herdr returned unexpected result type '{actual}', expected '{expected}'")]
    UnexpectedResult {
        expected: &'static str,
        actual: String,
    },
    #[error(
        "Herdr protocol {actual} is incompatible; Yard supports protocols {minimum} through {maximum}"
    )]
    ProtocolMismatch {
        minimum: u32,
        maximum: u32,
        actual: u32,
    },
    #[error("invalid Herdr topology: {0}")]
    InvalidTopology(String),
}

impl HerdrError {
    /// Whether this error proves Herdr never acted on the request: nothing
    /// was sent (discovery failed, the session is gone, the socket would not
    /// connect) or Herdr answered with a structured refusal. Any other
    /// failure (a timeout, a read or write error, a malformed or unexpected
    /// reply) can come after Herdr received and carried out the request, so
    /// the outcome is unknown.
    #[must_use]
    pub fn request_not_delivered(&self) -> bool {
        matches!(
            self,
            Self::DiscoveryIo(_)
                | Self::DiscoveryTimeout
                | Self::DiscoveryResponseTooLarge { .. }
                | Self::DiscoveryFailed { .. }
                | Self::DiscoveryDecode(_)
                | Self::SessionNotFound(_)
                | Self::SessionNotRunning(_)
                | Self::SocketConnect { .. }
                | Self::Api { .. }
        )
    }
}

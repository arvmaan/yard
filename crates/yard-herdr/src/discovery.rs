use std::{io, path::PathBuf, process::Stdio};

use serde::Deserialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};
use yard_domain::RuntimeSession;

use crate::{HerdrConfig, HerdrError};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrSession {
    pub name: String,
    #[serde(rename = "default")]
    pub is_default: bool,
    pub running: bool,
    pub socket_path: PathBuf,
}

impl HerdrSession {
    pub(crate) fn into_summary(self) -> RuntimeSession {
        RuntimeSession {
            name: self.name,
            is_default: self.is_default,
            running: self.running,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SessionList {
    sessions: Vec<HerdrSession>,
}

pub(crate) async fn discover_sessions(
    config: &HerdrConfig,
) -> Result<Vec<HerdrSession>, HerdrError> {
    let mut command = Command::new(&config.binary);
    command
        .args(["session", "list", "--json"])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(HerdrError::DiscoveryIo)?;
    let stdout = child
        .stdout
        .take()
        .expect("piped Herdr stdout must be present");
    let stderr = child
        .stderr
        .take()
        .expect("piped Herdr stderr must be present");

    let operation = async move {
        let (stdout, stderr, status) = tokio::try_join!(
            read_bounded(stdout, config.max_discovery_bytes, "stdout"),
            read_bounded(stderr, 64 * 1024, "stderr"),
            async { child.wait().await.map_err(DiscoveryRunError::Io) }
        )?;
        Ok::<_, DiscoveryRunError>((status, stdout, stderr))
    };
    let (status, stdout, stderr) = timeout(config.request_timeout, operation)
        .await
        .map_err(|_| HerdrError::DiscoveryTimeout)?
        .map_err(DiscoveryRunError::into_herdr)?;

    if !status.success() {
        return Err(HerdrError::DiscoveryFailed {
            status,
            stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }

    parse_sessions(&stdout)
}

fn parse_sessions(bytes: &[u8]) -> Result<Vec<HerdrSession>, HerdrError> {
    serde_json::from_slice::<SessionList>(bytes)
        .map(|list| list.sessions)
        .map_err(HerdrError::DiscoveryDecode)
}

enum DiscoveryRunError {
    Io(io::Error),
    TooLarge { stream: &'static str, limit: usize },
}

impl DiscoveryRunError {
    fn into_herdr(self) -> HerdrError {
        match self {
            Self::Io(error) => HerdrError::DiscoveryIo(error),
            Self::TooLarge { stream, limit } => {
                HerdrError::DiscoveryResponseTooLarge { stream, limit }
            }
        }
    }
}

async fn read_bounded(
    reader: impl AsyncRead + Unpin,
    limit: usize,
    stream: &'static str,
) -> Result<Vec<u8>, DiscoveryRunError> {
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut reader = reader.take(take_limit);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(DiscoveryRunError::Io)?;
    if bytes.len() > limit {
        return Err(DiscoveryRunError::TooLarge { stream, limit });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::parse_sessions;

    #[test]
    fn parses_scrubbed_session_inventory() {
        let sessions =
            parse_sessions(include_bytes!("../tests/fixtures/v0.8.0/sessions.json")).unwrap();

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "default");
        assert!(sessions[0].is_default);
        assert!(sessions[0].running);
        assert!(!sessions[1].running);
    }
}

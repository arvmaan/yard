use std::path::Path;

use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::timeout,
};

use crate::{
    HerdrConfig, HerdrError,
    wire::{ApiResponse, SessionSnapshot},
};

const SNAPSHOT_REQUEST_ID: &str = "yard:session:snapshot";

pub(crate) async fn request_snapshot(
    config: &HerdrConfig,
    socket_path: &Path,
) -> Result<SessionSnapshot, HerdrError> {
    timeout(
        config.request_timeout,
        request_snapshot_inner(config, socket_path),
    )
    .await
    .map_err(|_| HerdrError::SocketTimeout)?
}

async fn request_snapshot_inner(
    config: &HerdrConfig,
    socket_path: &Path,
) -> Result<SessionSnapshot, HerdrError> {
    let stream =
        UnixStream::connect(socket_path)
            .await
            .map_err(|source| HerdrError::SocketConnect {
                path: socket_path.to_path_buf(),
                source,
            })?;
    let (reader, mut writer) = stream.into_split();
    let mut request = serde_json::to_vec(&json!({
        "id": SNAPSHOT_REQUEST_ID,
        "method": "session.snapshot",
        "params": {}
    }))
    .expect("static snapshot request must serialize");
    request.push(b'\n');

    writer
        .write_all(&request)
        .await
        .map_err(HerdrError::SocketWrite)?;
    writer.flush().await.map_err(HerdrError::SocketWrite)?;

    let limit = u64::try_from(config.max_response_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut reader = BufReader::new(reader).take(limit);
    let mut response = Vec::new();
    reader
        .read_until(b'\n', &mut response)
        .await
        .map_err(HerdrError::SocketWrite)?;

    if response.is_empty() {
        return Err(HerdrError::EmptyResponse);
    }
    if response.len() > config.max_response_bytes {
        return Err(HerdrError::ResponseTooLarge(config.max_response_bytes));
    }
    if response.last() != Some(&b'\n') {
        return Err(HerdrError::UnterminatedResponse);
    }

    decode_snapshot(&response)
}

pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<SessionSnapshot, HerdrError> {
    let response: ApiResponse =
        serde_json::from_slice(bytes).map_err(HerdrError::SnapshotDecode)?;

    if response.id != SNAPSHOT_REQUEST_ID {
        return Err(HerdrError::ResponseIdMismatch {
            expected: SNAPSHOT_REQUEST_ID.to_owned(),
            actual: response.id,
        });
    }
    if let Some(error) = response.error {
        return Err(HerdrError::Api {
            code: error.code,
            message: error.message,
        });
    }
    let result = response.result.ok_or(HerdrError::MissingSnapshot)?;
    if result.kind != "session_snapshot" {
        return Err(HerdrError::MissingSnapshot);
    }
    Ok(result.snapshot)
}

pub(crate) async fn request_command(
    config: &HerdrConfig,
    socket_path: &Path,
    request_id: &str,
    method: &str,
    params: serde_json::Value,
    request_timeout: std::time::Duration,
) -> Result<serde_json::Value, HerdrError> {
    timeout(
        request_timeout,
        request_command_inner(config, socket_path, request_id, method, params),
    )
    .await
    .map_err(|_| HerdrError::SocketTimeout)?
}

async fn request_command_inner(
    config: &HerdrConfig,
    socket_path: &Path,
    request_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, HerdrError> {
    let stream =
        UnixStream::connect(socket_path)
            .await
            .map_err(|source| HerdrError::SocketConnect {
                path: socket_path.to_path_buf(),
                source,
            })?;
    let (reader, mut writer) = stream.into_split();
    let mut request = serde_json::to_vec(&json!({
        "id": request_id,
        "method": method,
        "params": params
    }))
    .expect("serializable Herdr command request");
    request.push(b'\n');
    writer
        .write_all(&request)
        .await
        .map_err(HerdrError::SocketWrite)?;
    writer.flush().await.map_err(HerdrError::SocketWrite)?;

    let limit = u64::try_from(config.max_response_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut reader = BufReader::new(reader).take(limit);
    let mut response = Vec::new();
    reader
        .read_until(b'\n', &mut response)
        .await
        .map_err(HerdrError::SocketWrite)?;
    validate_response_frame(config, &response)?;
    decode_command_response(&response, request_id)
}

fn validate_response_frame(config: &HerdrConfig, response: &[u8]) -> Result<(), HerdrError> {
    if response.is_empty() {
        return Err(HerdrError::EmptyResponse);
    }
    if response.len() > config.max_response_bytes {
        return Err(HerdrError::ResponseTooLarge(config.max_response_bytes));
    }
    if response.last() != Some(&b'\n') {
        return Err(HerdrError::UnterminatedResponse);
    }
    Ok(())
}

fn decode_command_response(
    bytes: &[u8],
    expected_id: &str,
) -> Result<serde_json::Value, HerdrError> {
    let response: serde_json::Value =
        serde_json::from_slice(bytes).map_err(HerdrError::CommandDecode)?;
    let actual_id = response
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or(HerdrError::MissingCommandResult)?;
    if actual_id != expected_id {
        return Err(HerdrError::ResponseIdMismatch {
            expected: expected_id.to_owned(),
            actual: actual_id.to_owned(),
        });
    }
    if let Some(error) = response.get("error") {
        return Err(HerdrError::Api {
            code: error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Herdr command failed")
                .to_owned(),
        });
    }
    response
        .get("result")
        .cloned()
        .ok_or(HerdrError::MissingCommandResult)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    use super::{SNAPSHOT_REQUEST_ID, decode_snapshot, request_command, request_snapshot};
    use crate::HerdrConfig;

    #[test]
    fn decodes_scrubbed_snapshot() {
        let snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json")).unwrap();

        assert_eq!(snapshot.protocol, 19);
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.agents.len(), 1);
    }

    #[test]
    fn decodes_herdr_0_8_2_protocol_20_snapshot() {
        let snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.2/snapshot.json")).unwrap();

        assert_eq!(snapshot.version, "0.8.2");
        assert_eq!(snapshot.protocol, 20);
        assert_eq!(snapshot.workspaces[0].workspace_id, "w20");
        assert_eq!(snapshot.tabs[0].tab_id, "w20:t1");
        assert_eq!(snapshot.panes[0].terminal_id, "term_protocol20");
        assert_eq!(snapshot.agents[0].name.as_deref(), Some("yard-protocol20"));
    }

    #[tokio::test]
    async fn frames_snapshot_request_as_ndjson() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let fixture: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json"))
                .unwrap();
        let mut fixture = serde_json::to_vec(&fixture).unwrap();
        fixture.push(b'\n');

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["id"], SNAPSHOT_REQUEST_ID);
            assert_eq!(request["method"], "session.snapshot");
            writer.write_all(&fixture).await.unwrap();
        });

        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };
        let snapshot = request_snapshot(&config, &socket_path).await.unwrap();
        server.await.unwrap();

        assert_eq!(snapshot.version, "0.8.0");
    }

    #[tokio::test]
    async fn frames_and_correlates_mutating_command() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["id"], "yard:command-1:tab");
            assert_eq!(request["method"], "tab.create");
            assert_eq!(request["params"]["workspace_id"], "w1");
            writer
                .write_all(
                    b"{\"id\":\"yard:command-1:tab\",\"result\":{\"type\":\"tab_created\"}}\n",
                )
                .await
                .unwrap();
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let result = request_command(
            &config,
            &socket_path,
            "yard:command-1:tab",
            "tab.create",
            serde_json::json!({"workspace_id": "w1"}),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(result["type"], "tab_created");
    }
}

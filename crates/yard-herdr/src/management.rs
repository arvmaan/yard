use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::{HerdrConfig, HerdrError, discovery::discover_sessions, socket::request_command};

pub const PANE_MANAGEMENT_ENDPOINT: &str = "pane_management_lease_v1";

#[derive(Clone, PartialEq, Eq)]
pub struct LeaseToken(String);

impl LeaseToken {
    #[must_use]
    pub fn from_secret(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LeaseToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LeaseToken([REDACTED])")
    }
}

impl fmt::Display for LeaseToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Serialize for LeaseToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneManagementCapability {
    pub supported: bool,
    pub endpoint: Option<String>,
}

impl PaneManagementCapability {
    #[must_use]
    pub fn unsupported() -> Self {
        Self {
            supported: false,
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquirePaneLeaseRequest {
    pub request_id: String,
    pub acquisition_request_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewPaneLeaseRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub token: LeaseToken,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePaneLeaseRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub token: LeaseToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLeaseStatusRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseLeasedPaneRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub token: LeaseToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLease {
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub token: LeaseToken,
    pub expires_at_unix_ms: u64,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLeaseOperationResult {
    pub expires_at_unix_ms: Option<u64>,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneLeaseStatus {
    Available,
    Owned,
    Conflict,
    Expired,
}

#[derive(Debug, Error)]
pub enum PaneManagementRpcError {
    #[error("Herdr does not support pane management leases")]
    Unsupported,
    #[error("the pane management lease is held by another owner")]
    Conflict,
    #[error("the pane management lease was not found")]
    NotFound,
    #[error("the pane management lease expired")]
    Expired,
    #[error("the pane management lease owner did not match")]
    OwnerMismatch,
    #[error("the pane management lease token did not match")]
    TokenMismatch,
    #[error("the pane instance changed")]
    PaneInstanceMismatch,
    #[error("the pane management request ID was replayed with different input")]
    ReplayConflict,
    #[error("Herdr returned an invalid pane management response: {0}")]
    Decode(#[source] serde_json::Error),
    #[error(transparent)]
    Runtime(#[from] HerdrError),
}

#[derive(Debug, Default, Deserialize)]
struct CapabilityFlags {
    #[serde(default)]
    pane_management_lease: bool,
    #[serde(default)]
    pane_management_lease_v1: bool,
}

#[derive(Debug, Default, Deserialize)]
struct Endpoints {
    #[serde(default)]
    pane_management_lease: Option<String>,
    #[serde(default)]
    pane_management_lease_v1: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Pong {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    capabilities: CapabilityFlags,
    #[serde(default)]
    endpoints: Endpoints,
}

#[derive(Debug, Deserialize)]
struct LeaseWire {
    #[serde(rename = "type")]
    kind: String,
    pane_id: String,
    pane_instance_id: String,
    owner_id: String,
    token: String,
    #[serde(deserialize_with = "deserialize_u64")]
    expires_at_unix_ms: u64,
    #[serde(default)]
    replayed: bool,
}

#[derive(Debug, Deserialize)]
struct OperationWire {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "deserialize_optional_u64")]
    expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    replayed: bool,
}

#[derive(Debug, Deserialize)]
struct StatusWire {
    #[serde(rename = "type")]
    kind: String,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum PaneManagementRequest<'a> {
    Acquire {
        acquisition_request_id: &'a str,
        pane_id: &'a str,
        pane_instance_id: &'a str,
        owner_id: &'a str,
        #[serde(serialize_with = "serialize_u64")]
        ttl_ms: u64,
    },
    Renew {
        pane_id: &'a str,
        pane_instance_id: &'a str,
        owner_id: &'a str,
        token: &'a str,
        #[serde(serialize_with = "serialize_u64")]
        ttl_ms: u64,
    },
    Release {
        pane_id: &'a str,
        pane_instance_id: &'a str,
        owner_id: &'a str,
        token: &'a str,
    },
    Status {
        pane_id: &'a str,
        pane_instance_id: &'a str,
        owner_id: &'a str,
    },
    CloseIfLeased {
        pane_id: &'a str,
        pane_instance_id: &'a str,
        owner_id: &'a str,
        token: &'a str,
    },
}

pub(crate) async fn capability(
    config: &HerdrConfig,
    session: &str,
) -> Result<PaneManagementCapability, PaneManagementRpcError> {
    let socket = running_socket(config, session).await?;
    let result = request_command(
        config,
        &socket,
        "yard:pane-management:capability",
        "ping",
        json!({}),
        config.request_timeout,
    )
    .await?;
    decode_capability(result)
}

pub(crate) async fn acquire(
    config: &HerdrConfig,
    request: AcquirePaneLeaseRequest,
) -> Result<PaneLease, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        &PaneManagementRequest::Acquire {
            acquisition_request_id: &request.acquisition_request_id,
            pane_id: &request.pane_id,
            pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            ttl_ms: request.ttl_ms,
        },
    )
    .await?;
    decode_lease(result)
}

pub(crate) async fn renew(
    config: &HerdrConfig,
    request: RenewPaneLeaseRequest,
) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        &PaneManagementRequest::Renew {
            pane_id: &request.pane_id,
            pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            token: request.token.expose_secret(),
            ttl_ms: request.ttl_ms,
        },
    )
    .await?;
    decode_operation(result, "pane_management_lease_renewed")
}

pub(crate) async fn release(
    config: &HerdrConfig,
    request: ReleasePaneLeaseRequest,
) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        &PaneManagementRequest::Release {
            pane_id: &request.pane_id,
            pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            token: request.token.expose_secret(),
        },
    )
    .await?;
    decode_operation(result, "pane_management_lease_released")
}

pub(crate) async fn status(
    config: &HerdrConfig,
    request: PaneLeaseStatusRequest,
) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        &PaneManagementRequest::Status {
            pane_id: &request.pane_id,
            pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
        },
    )
    .await?;
    decode_status(result)
}

pub(crate) async fn close_if_leased(
    config: &HerdrConfig,
    request: CloseLeasedPaneRequest,
) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        &PaneManagementRequest::CloseIfLeased {
            pane_id: &request.pane_id,
            pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            token: request.token.expose_secret(),
        },
    )
    .await?;
    decode_operation(result, "pane_closed_if_leased")
}

async fn rpc(
    config: &HerdrConfig,
    session: &str,
    request_id: &str,
    request: &PaneManagementRequest<'_>,
) -> Result<serde_json::Value, PaneManagementRpcError> {
    let socket = running_socket(config, session).await?;
    rpc_at_socket(config, &socket, request_id, request).await
}

async fn rpc_at_socket(
    config: &HerdrConfig,
    socket: &std::path::Path,
    request_id: &str,
    request: &PaneManagementRequest<'_>,
) -> Result<serde_json::Value, PaneManagementRpcError> {
    request_command(
        config,
        socket,
        request_id,
        PANE_MANAGEMENT_ENDPOINT,
        serde_json::to_value(request).map_err(PaneManagementRpcError::Decode)?,
        config.request_timeout,
    )
    .await
    .map_err(map_runtime_error)
}

async fn running_socket(
    config: &HerdrConfig,
    session: &str,
) -> Result<std::path::PathBuf, PaneManagementRpcError> {
    discover_sessions(config)
        .await?
        .into_iter()
        .find(|candidate| candidate.name == session && candidate.running)
        .map(|candidate| candidate.socket_path)
        .ok_or_else(|| HerdrError::SessionNotRunning(session.to_owned()).into())
}

fn decode_capability(
    value: serde_json::Value,
) -> Result<PaneManagementCapability, PaneManagementRpcError> {
    let pong: Pong = serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    let advertised =
        pong.capabilities.pane_management_lease || pong.capabilities.pane_management_lease_v1;
    let endpoint = [
        pong.endpoints.pane_management_lease,
        pong.endpoints.pane_management_lease_v1,
    ]
    .into_iter()
    .flatten()
    .find(|endpoint| endpoint == PANE_MANAGEMENT_ENDPOINT);
    if pong.kind != "pong" || !advertised || endpoint.is_none() {
        return Ok(PaneManagementCapability::unsupported());
    }
    Ok(PaneManagementCapability {
        supported: true,
        endpoint,
    })
}

fn decode_lease(value: serde_json::Value) -> Result<PaneLease, PaneManagementRpcError> {
    let lease: LeaseWire = serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    if lease.kind != "pane_management_lease_acquired" {
        return Err(unexpected_result(&lease.kind));
    }
    Ok(PaneLease {
        pane_id: lease.pane_id,
        pane_instance_id: lease.pane_instance_id,
        owner_id: lease.owner_id,
        token: LeaseToken::from_secret(lease.token),
        expires_at_unix_ms: lease.expires_at_unix_ms,
        replayed: lease.replayed,
    })
}

fn decode_operation(
    value: serde_json::Value,
    expected: &'static str,
) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let result: OperationWire =
        serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    if result.kind != expected {
        return Err(unexpected_result(&result.kind));
    }
    Ok(PaneLeaseOperationResult {
        expires_at_unix_ms: result.expires_at_unix_ms,
        replayed: result.replayed,
    })
}

fn decode_status(value: serde_json::Value) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
    let status: StatusWire =
        serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    if status.kind != "pane_management_lease_status" {
        return Err(unexpected_result(&status.kind));
    }
    match status.status.as_str() {
        "available" => Ok(PaneLeaseStatus::Available),
        "owned" => Ok(PaneLeaseStatus::Owned),
        "conflict" => Ok(PaneLeaseStatus::Conflict),
        "expired" => Ok(PaneLeaseStatus::Expired),
        _ => Err(unexpected_result(&status.status)),
    }
}

fn map_runtime_error(error: HerdrError) -> PaneManagementRpcError {
    let HerdrError::Api { code, .. } = &error else {
        return PaneManagementRpcError::Runtime(error);
    };
    match code.as_str() {
        "method_not_found" | "pane_management_lease_unsupported" => {
            PaneManagementRpcError::Unsupported
        }
        "pane_management_lease_conflict" => PaneManagementRpcError::Conflict,
        "pane_management_lease_not_found" => PaneManagementRpcError::NotFound,
        "pane_management_lease_expired" => PaneManagementRpcError::Expired,
        "pane_management_lease_owner_mismatch" => PaneManagementRpcError::OwnerMismatch,
        "pane_management_lease_token_mismatch" => PaneManagementRpcError::TokenMismatch,
        "pane_instance_mismatch" => PaneManagementRpcError::PaneInstanceMismatch,
        "idempotency_conflict" => PaneManagementRpcError::ReplayConflict,
        _ => PaneManagementRpcError::Runtime(error),
    }
}

fn unexpected_result(actual: &str) -> PaneManagementRpcError {
    PaneManagementRpcError::Runtime(HerdrError::UnexpectedResult {
        expected: "pane_management_lease_v1 result",
        actual: actual.to_owned(),
    })
}

fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(value) => value.parse().map_err(serde::de::Error::custom),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        _ => Err(serde::de::Error::custom("expected an integer string")),
    }
}

fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => {
            value.parse().map(Some).map_err(serde::de::Error::custom)
        }
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        Some(_) => Err(serde::de::Error::custom("expected an integer string")),
    }
}

fn serialize_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde::Deserialize;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    use super::*;

    #[derive(Deserialize)]
    struct RpcFixtures {
        calls: Vec<RpcCall>,
        errors: Vec<RpcError>,
    }

    #[derive(Deserialize)]
    struct RpcCall {
        request: serde_json::Value,
        result: serde_json::Value,
    }

    #[derive(Deserialize)]
    struct RpcError {
        code: String,
    }

    fn fixture(name: &str) -> serde_json::Value {
        let bytes = match name {
            "supported" => {
                include_bytes!("../tests/fixtures/pane-management-lease-v1/supported.json")
                    .as_slice()
            }
            "unsupported" => {
                include_bytes!("../tests/fixtures/pane-management-lease-v1/unsupported-0.9.0.json")
                    .as_slice()
            }
            "rpc" => {
                include_bytes!("../tests/fixtures/pane-management-lease-v1/rpc.json").as_slice()
            }
            _ => unreachable!(),
        };
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn negotiates_only_the_exact_capability_and_endpoint() {
        let supported = fixture("supported")["ping_result"].clone();
        assert!(decode_capability(supported.clone()).unwrap().supported);
        let mut legacy_only = supported.clone();
        legacy_only["capabilities"]["pane_management_lease_v1"] = json!(false);
        legacy_only["endpoints"]["pane_management_lease_v1"] = serde_json::Value::Null;
        assert!(decode_capability(legacy_only).unwrap().supported);
        let mut versioned_only = supported;
        versioned_only["capabilities"]["pane_management_lease"] = json!(false);
        versioned_only["endpoints"]["pane_management_lease"] = serde_json::Value::Null;
        assert!(decode_capability(versioned_only).unwrap().supported);
        assert!(
            !decode_capability(fixture("unsupported")["ping_result"].clone())
                .unwrap()
                .supported
        );
        let mut spoofed = fixture("supported")["ping_result"].clone();
        spoofed["endpoints"]["pane_management_lease"] = json!("pane_management_lease_v2");
        spoofed["endpoints"]["pane_management_lease_v1"] = json!("pane_management_lease_v2");
        assert!(!decode_capability(spoofed).unwrap().supported);
    }

    #[test]
    fn decodes_replay_and_status_contract() {
        let fixture: RpcFixtures =
            serde_json::from_value(fixture("rpc")).expect("typed RPC fixture");
        let lease = decode_lease(fixture.calls[0].result.clone()).unwrap();
        assert!(lease.replayed);
        assert_eq!(lease.expires_at_unix_ms, 1_790_000_060_000);
        assert_eq!(
            decode_status(fixture.calls[3].result.clone()).unwrap(),
            PaneLeaseStatus::Conflict
        );
    }

    #[test]
    fn maps_all_typed_contract_errors() {
        let fixture: RpcFixtures =
            serde_json::from_value(fixture("rpc")).expect("typed RPC fixture");
        let expected = [
            PaneManagementRpcError::Conflict,
            PaneManagementRpcError::NotFound,
            PaneManagementRpcError::Expired,
            PaneManagementRpcError::OwnerMismatch,
            PaneManagementRpcError::TokenMismatch,
            PaneManagementRpcError::PaneInstanceMismatch,
            PaneManagementRpcError::ReplayConflict,
        ];
        for (wire, expected) in fixture.errors.iter().zip(expected) {
            let mapped = map_runtime_error(HerdrError::Api {
                code: wire.code.clone(),
                message: "redacted fixture error".to_owned(),
            });
            assert_eq!(
                std::mem::discriminant(&mapped),
                std::mem::discriminant(&expected)
            );
        }
    }

    #[test]
    fn lease_token_redacts_debug_display_and_serialization() {
        let token = LeaseToken::from_secret("lease-secret-value".to_owned());
        assert_eq!(format!("{token:?}"), "LeaseToken([REDACTED])");
        assert_eq!(token.to_string(), "[REDACTED]");
        assert_eq!(serde_json::to_string(&token).unwrap(), "\"[REDACTED]\"");
        assert!(!format!("{token:?}{token}").contains(token.expose_secret()));
    }

    #[tokio::test]
    async fn frames_all_typed_rpcs_for_exact_replay() {
        let fixture: RpcFixtures =
            serde_json::from_value(fixture("rpc")).expect("typed RPC fixture");
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let calls = fixture
            .calls
            .iter()
            .flat_map(|call| [call.request.clone(), call.request.clone()])
            .zip(
                fixture
                    .calls
                    .iter()
                    .flat_map(|call| [call.result.clone(), call.result.clone()]),
            )
            .collect::<Vec<_>>();
        let server = tokio::spawn(async move {
            for (expected, result) in calls {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request, expected);
                let response = json!({"id": request["id"], "result": result});
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let token = LeaseToken::from_secret("fixture-secret-never-expose".to_owned());
        let requests = [
            PaneManagementRequest::Acquire {
                acquisition_request_id: "yard:manage:batch:pane",
                pane_id: "pane-1",
                pane_instance_id: "pane-instance-1",
                owner_id: "yard:installation-1",
                ttl_ms: 60_000,
            },
            PaneManagementRequest::Renew {
                pane_id: "pane-1",
                pane_instance_id: "pane-instance-1",
                owner_id: "yard:installation-1",
                token: token.expose_secret(),
                ttl_ms: 60_000,
            },
            PaneManagementRequest::Release {
                pane_id: "pane-1",
                pane_instance_id: "pane-instance-1",
                owner_id: "yard:installation-1",
                token: token.expose_secret(),
            },
            PaneManagementRequest::Status {
                pane_id: "pane-1",
                pane_instance_id: "pane-instance-1",
                owner_id: "yard:installation-1",
            },
            PaneManagementRequest::CloseIfLeased {
                pane_id: "pane-1",
                pane_instance_id: "pane-instance-1",
                owner_id: "yard:installation-1",
                token: token.expose_secret(),
            },
        ];
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };
        for (request, call) in requests.iter().zip(&fixture.calls) {
            let id = call.request["id"].as_str().unwrap();
            let first = rpc_at_socket(&config, &socket_path, id, request)
                .await
                .unwrap();
            let replay = rpc_at_socket(&config, &socket_path, id, request)
                .await
                .unwrap();
            assert_eq!(first, replay);
        }
        server.await.unwrap();
    }
}

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::{HerdrConfig, HerdrError, discovery::discover_sessions, socket::request_command};

pub const PANE_MANAGEMENT_CAPABILITY_V1: &str = "pane_management_lease_v1";

const ACQUIRE_METHOD: &str = "pane.management_lease.acquire";
const RENEW_METHOD: &str = "pane.management_lease.renew";
const RELEASE_METHOD: &str = "pane.management_lease.release";
const STATUS_METHOD: &str = "pane.management_lease.status";
const CLOSE_METHOD: &str = "pane.close_if_management_leased";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLeaseOperationResult {
    pub expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneLeaseStatus {
    Available,
    Leased {
        pane_id: String,
        pane_instance_id: String,
        owner_id: String,
        expires_at_unix_ms: u64,
    },
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
    #[error("the pane management lease duration was invalid")]
    InvalidDuration,
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

#[derive(Debug, Deserialize)]
struct Pong {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    capabilities: CapabilityFlags,
}

#[derive(Deserialize)]
struct LeaseResultWire {
    #[serde(rename = "type")]
    kind: String,
    lease: LeaseWire,
}

#[derive(Deserialize)]
struct LeaseWire {
    pane_id: String,
    pane_instance_id: String,
    owner_id: String,
    token: String,
    #[serde(deserialize_with = "deserialize_u64")]
    expires_at_unix_ms: u64,
}

#[derive(Debug, Deserialize)]
struct OkWire {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct StatusResultWire {
    #[serde(rename = "type")]
    kind: String,
    lease: Option<StatusLeaseWire>,
}

#[derive(Debug, Deserialize)]
struct StatusLeaseWire {
    pane_id: String,
    pane_instance_id: String,
    owner_id: String,
    #[serde(deserialize_with = "deserialize_u64")]
    expires_at_unix_ms: u64,
}

#[derive(Serialize)]
struct AcquireParams<'a> {
    pane_id: &'a str,
    expected_pane_instance_id: &'a str,
    owner_id: &'a str,
    lease_duration_ms: u64,
}

#[derive(Serialize)]
struct RenewParams<'a> {
    pane_id: &'a str,
    expected_pane_instance_id: &'a str,
    owner_id: &'a str,
    token: &'a str,
    lease_duration_ms: u64,
}

#[derive(Serialize)]
struct AuthenticatedParams<'a> {
    pane_id: &'a str,
    expected_pane_instance_id: &'a str,
    owner_id: &'a str,
    token: &'a str,
}

#[derive(Serialize)]
struct StatusParams<'a> {
    pane_id: &'a str,
    expected_pane_instance_id: &'a str,
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
        ACQUIRE_METHOD,
        &AcquireParams {
            pane_id: &request.pane_id,
            expected_pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            lease_duration_ms: request.ttl_ms,
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
        RENEW_METHOD,
        &RenewParams {
            pane_id: &request.pane_id,
            expected_pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            token: request.token.expose_secret(),
            lease_duration_ms: request.ttl_ms,
        },
    )
    .await?;
    decode_renewed_lease(result)
}

pub(crate) async fn release(
    config: &HerdrConfig,
    request: ReleasePaneLeaseRequest,
) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        RELEASE_METHOD,
        &AuthenticatedParams {
            pane_id: &request.pane_id,
            expected_pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            token: request.token.expose_secret(),
        },
    )
    .await?;
    decode_ok(result)
}

pub(crate) async fn status(
    config: &HerdrConfig,
    request: PaneLeaseStatusRequest,
) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
    let result = rpc(
        config,
        &request.session,
        &request.request_id,
        STATUS_METHOD,
        &StatusParams {
            pane_id: &request.pane_id,
            expected_pane_instance_id: &request.pane_instance_id,
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
        CLOSE_METHOD,
        &AuthenticatedParams {
            pane_id: &request.pane_id,
            expected_pane_instance_id: &request.pane_instance_id,
            owner_id: &request.owner_id,
            token: request.token.expose_secret(),
        },
    )
    .await?;
    decode_ok(result)
}

async fn rpc<T: Serialize>(
    config: &HerdrConfig,
    session: &str,
    request_id: &str,
    method: &str,
    params: &T,
) -> Result<serde_json::Value, PaneManagementRpcError> {
    let socket = running_socket(config, session).await?;
    rpc_at_socket(config, &socket, request_id, method, params).await
}

async fn rpc_at_socket<T: Serialize>(
    config: &HerdrConfig,
    socket: &std::path::Path,
    request_id: &str,
    method: &str,
    params: &T,
) -> Result<serde_json::Value, PaneManagementRpcError> {
    request_command(
        config,
        socket,
        request_id,
        method,
        serde_json::to_value(params).map_err(PaneManagementRpcError::Decode)?,
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
    if pong.kind != "pong" || !advertised {
        return Ok(PaneManagementCapability::unsupported());
    }
    Ok(PaneManagementCapability {
        supported: true,
        endpoint: Some(PANE_MANAGEMENT_CAPABILITY_V1.to_owned()),
    })
}

fn decode_lease(value: serde_json::Value) -> Result<PaneLease, PaneManagementRpcError> {
    let result: LeaseResultWire =
        serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    if result.kind != "pane_management_lease" {
        return Err(unexpected_result(&result.kind));
    }
    let lease = result.lease;
    Ok(PaneLease {
        pane_id: lease.pane_id,
        pane_instance_id: lease.pane_instance_id,
        owner_id: lease.owner_id,
        token: LeaseToken::from_secret(lease.token),
        expires_at_unix_ms: lease.expires_at_unix_ms,
    })
}

fn decode_renewed_lease(
    value: serde_json::Value,
) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let lease = decode_lease(value)?;
    Ok(PaneLeaseOperationResult {
        expires_at_unix_ms: Some(lease.expires_at_unix_ms),
    })
}

fn decode_ok(value: serde_json::Value) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
    let result: OkWire = serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    if result.kind != "ok" {
        return Err(unexpected_result(&result.kind));
    }
    Ok(PaneLeaseOperationResult {
        expires_at_unix_ms: None,
    })
}

fn decode_status(value: serde_json::Value) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
    let result: StatusResultWire =
        serde_json::from_value(value).map_err(PaneManagementRpcError::Decode)?;
    if result.kind != "pane_management_lease_status" {
        return Err(unexpected_result(&result.kind));
    }
    match result.lease {
        None => Ok(PaneLeaseStatus::Available),
        Some(lease) => Ok(PaneLeaseStatus::Leased {
            pane_id: lease.pane_id,
            pane_instance_id: lease.pane_instance_id,
            owner_id: lease.owner_id,
            expires_at_unix_ms: lease.expires_at_unix_ms,
        }),
    }
}

fn map_runtime_error(error: HerdrError) -> PaneManagementRpcError {
    let HerdrError::Api { code, .. } = &error else {
        return PaneManagementRpcError::Runtime(error);
    };
    match code.as_str() {
        "method_not_found" | "METHOD_NOT_FOUND" | "PANE_MANAGEMENT_LEASE_UNSUPPORTED" => {
            PaneManagementRpcError::Unsupported
        }
        "LEASE_DURATION_INVALID" => PaneManagementRpcError::InvalidDuration,
        "LEASE_CONFLICT" => PaneManagementRpcError::Conflict,
        "LEASE_NOT_FOUND" | "PANE_NOT_FOUND" => PaneManagementRpcError::NotFound,
        "LEASE_EXPIRED" => PaneManagementRpcError::Expired,
        "LEASE_OWNER_MISMATCH" => PaneManagementRpcError::OwnerMismatch,
        "LEASE_TOKEN_INVALID" => PaneManagementRpcError::TokenMismatch,
        "PANE_INSTANCE_MISMATCH" => PaneManagementRpcError::PaneInstanceMismatch,
        "REQUEST_REPLAY_MISMATCH" => PaneManagementRpcError::ReplayConflict,
        _ => PaneManagementRpcError::Runtime(error),
    }
}

fn unexpected_result(actual: &str) -> PaneManagementRpcError {
    PaneManagementRpcError::Runtime(HerdrError::UnexpectedResult {
        expected: "pane management result",
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
    fn negotiates_either_capability_alias() {
        let supported = fixture("supported")["ping_result"].clone();
        assert!(decode_capability(supported.clone()).unwrap().supported);
        let mut legacy_only = supported.clone();
        legacy_only["capabilities"]["pane_management_lease_v1"] = json!(false);
        assert!(decode_capability(legacy_only).unwrap().supported);
        let mut versioned_only = supported;
        versioned_only["capabilities"]["pane_management_lease"] = json!(false);
        assert!(decode_capability(versioned_only).unwrap().supported);
        assert!(
            !decode_capability(fixture("unsupported")["ping_result"].clone())
                .unwrap()
                .supported
        );
    }

    #[test]
    fn decodes_lease_and_status_contract() {
        let fixture: RpcFixtures =
            serde_json::from_value(fixture("rpc")).expect("typed RPC fixture");
        let lease = decode_lease(fixture.calls[0].result.clone()).unwrap();
        assert_eq!(lease.expires_at_unix_ms, 1_790_000_060_000);
        assert!(matches!(
            decode_status(fixture.calls[3].result.clone()).unwrap(),
            PaneLeaseStatus::Leased { owner_id, .. } if owner_id == "other-owner"
        ));
    }

    #[test]
    fn maps_all_typed_contract_errors() {
        let fixture: RpcFixtures =
            serde_json::from_value(fixture("rpc")).expect("typed RPC fixture");
        let expected = [
            PaneManagementRpcError::InvalidDuration,
            PaneManagementRpcError::Conflict,
            PaneManagementRpcError::NotFound,
            PaneManagementRpcError::Expired,
            PaneManagementRpcError::OwnerMismatch,
            PaneManagementRpcError::TokenMismatch,
            PaneManagementRpcError::PaneInstanceMismatch,
            PaneManagementRpcError::ReplayConflict,
            PaneManagementRpcError::NotFound,
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
            (
                ACQUIRE_METHOD,
                serde_json::to_value(AcquireParams {
                    pane_id: "pane-1",
                    expected_pane_instance_id: "pane-instance-1",
                    owner_id: "yard:installation-1",
                    lease_duration_ms: 60_000,
                })
                .unwrap(),
            ),
            (
                RENEW_METHOD,
                serde_json::to_value(RenewParams {
                    pane_id: "pane-1",
                    expected_pane_instance_id: "pane-instance-1",
                    owner_id: "yard:installation-1",
                    token: token.expose_secret(),
                    lease_duration_ms: 60_000,
                })
                .unwrap(),
            ),
            (
                RELEASE_METHOD,
                serde_json::to_value(AuthenticatedParams {
                    pane_id: "pane-1",
                    expected_pane_instance_id: "pane-instance-1",
                    owner_id: "yard:installation-1",
                    token: token.expose_secret(),
                })
                .unwrap(),
            ),
            (
                STATUS_METHOD,
                serde_json::to_value(StatusParams {
                    pane_id: "pane-1",
                    expected_pane_instance_id: "pane-instance-1",
                })
                .unwrap(),
            ),
            (
                CLOSE_METHOD,
                serde_json::to_value(AuthenticatedParams {
                    pane_id: "pane-1",
                    expected_pane_instance_id: "pane-instance-1",
                    owner_id: "yard:installation-1",
                    token: token.expose_secret(),
                })
                .unwrap(),
            ),
        ];
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };
        for ((method, params), call) in requests.iter().zip(&fixture.calls) {
            let id = call.request["id"].as_str().unwrap();
            let first = rpc_at_socket(&config, &socket_path, id, method, params)
                .await
                .unwrap();
            let replay = rpc_at_socket(&config, &socket_path, id, method, params)
                .await
                .unwrap();
            assert_eq!(first, replay);
        }
        server.await.unwrap();
    }
}

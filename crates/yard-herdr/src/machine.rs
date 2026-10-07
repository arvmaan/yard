use std::{collections::HashSet, io, process::Stdio, time::SystemTime};

use serde::Deserialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};
use yard_domain::{
    EndpointRuntimeInventory, RuntimeEndpoint, RuntimeEndpointCapabilities,
    RuntimeEndpointConnectionState, RuntimeEndpointRef, RuntimeEndpointSession, RuntimeEndpoints,
};

use crate::{HerdrConfig, HerdrError, normalize, wire::ApiResponse};

const MAX_MACHINES: usize = 64;
const MAX_MACHINE_OUTPUT_BYTES: usize = 1024 * 1024;
const MACHINE_ID_BYTES: usize = 32;
const MAX_MACHINE_LABEL_BYTES: usize = 128;
const MAX_SESSION_BYTES: usize = 128;
const MAX_INVENTORY_RECORDS: usize = 10_000;
const MAX_RUNTIME_ID_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub(crate) struct SavedMachine {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) session: String,
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineListRow {
    id: String,
    label: String,
    #[serde(rename = "target")]
    _target: serde::de::IgnoredAny,
    session: String,
    enabled: bool,
    #[allow(dead_code)]
    selected: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineStatusRow {
    id: String,
    label: String,
    status: String,
    error: Option<String>,
}

pub(crate) async fn endpoints(config: &HerdrConfig) -> Result<RuntimeEndpoints, HerdrError> {
    let local_sessions = crate::discovery::discover_sessions(config)
        .await
        .unwrap_or_default();
    let machines = list_saved_machines(config).await?;
    let mut endpoints = vec![RuntimeEndpoint {
        endpoint: RuntimeEndpointRef::Local,
        label: "Local".to_owned(),
        enabled: true,
        connection_state: if local_sessions.iter().any(|session| session.running) {
            RuntimeEndpointConnectionState::Reachable
        } else {
            RuntimeEndpointConnectionState::Unreachable
        },
        capabilities: RuntimeEndpointCapabilities {
            inventory_read: true,
            mutations: true,
            terminal_streaming: true,
        },
        sessions: local_sessions
            .into_iter()
            .map(|session| RuntimeEndpointSession {
                name: session.name,
                is_default: session.is_default,
                observed_running: Some(session.running),
            })
            .collect(),
    }];
    let statuses = status_saved_machines(config).await.unwrap_or_default();
    for machine in machines {
        let (connection_state, inventory_read) = if machine.enabled {
            let state = statuses
                .iter()
                .find(|status| status.id == machine.id)
                .map_or(RuntimeEndpointConnectionState::Unknown, connection_state);
            (state, state == RuntimeEndpointConnectionState::Reachable)
        } else {
            (RuntimeEndpointConnectionState::Disabled, false)
        };
        endpoints.push(RuntimeEndpoint {
            endpoint: RuntimeEndpointRef::Machine {
                machine_id: machine.id,
            },
            label: machine.label,
            enabled: machine.enabled,
            connection_state,
            capabilities: RuntimeEndpointCapabilities {
                inventory_read,
                mutations: false,
                terminal_streaming: false,
            },
            sessions: vec![RuntimeEndpointSession {
                is_default: machine.session == "default",
                name: machine.session,
                observed_running: inventory_read.then_some(true),
            }],
        });
    }
    Ok(RuntimeEndpoints {
        adapter: "herdr".to_owned(),
        endpoints,
    })
}

pub(crate) async fn inventory(
    config: &HerdrConfig,
    machine_id: &str,
) -> Result<EndpointRuntimeInventory, HerdrError> {
    validate_machine_id(machine_id)?;
    let machines = list_saved_machines(config).await?;
    let machine = machines
        .iter()
        .find(|machine| machine.id == machine_id)
        .ok_or_else(|| HerdrError::MachineNotFound(machine_id.to_owned()))?;
    if !machine.enabled {
        return Err(HerdrError::MachineDisabled(machine_id.to_owned()));
    }
    let session = machine.session.clone();
    let stdout = run_bounded(
        config,
        ["--machine", machine.id.as_str(), "api", "snapshot"],
        config.max_response_bytes,
        HerdrCommand::Forward,
    )
    .await?;
    let response: ApiResponse =
        serde_json::from_slice(&stdout).map_err(|_| sanitized_machine_inventory_error())?;
    if response.error.is_some() {
        return Err(HerdrError::MachineForwardFailed);
    }
    let result = response.result.ok_or(HerdrError::MissingSnapshot)?;
    if result.kind != "session_snapshot" {
        return Err(HerdrError::MissingSnapshot);
    }
    let snapshot = result.snapshot;
    let observed_at_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        });
    let normalized = normalize::normalize(snapshot, &session, observed_at_unix_ms, config)
        .map_err(|_| sanitized_machine_inventory_error())?;
    validate_inventory_bounds(&normalized)?;
    Ok(EndpointRuntimeInventory {
        endpoint: RuntimeEndpointRef::Machine {
            machine_id: machine.id.clone(),
        },
        inventory: normalized,
    })
}

pub(crate) async fn list_saved_machines(
    config: &HerdrConfig,
) -> Result<Vec<SavedMachine>, HerdrError> {
    let stdout = run_bounded(
        config,
        ["machine", "list", "--json"],
        MAX_MACHINE_OUTPUT_BYTES,
        HerdrCommand::Catalog,
    )
    .await?;
    let rows: Vec<MachineListRow> =
        serde_json::from_slice(&stdout).map_err(HerdrError::MachineCatalogDecode)?;
    if rows.len() > MAX_MACHINES {
        return Err(HerdrError::MachineCatalogInvalid(format!(
            "catalog contains more than {MAX_MACHINES} machines"
        )));
    }
    let mut machine_ids = HashSet::with_capacity(rows.len());
    if rows.iter().any(|row| !machine_ids.insert(row.id.as_str())) {
        return Err(HerdrError::MachineCatalogInvalid(
            "catalog contains duplicate machine IDs".to_owned(),
        ));
    }
    rows.into_iter()
        .map(|row| {
            validate_machine_id(&row.id)?;
            validate_text("machine label", &row.label, MAX_MACHINE_LABEL_BYTES)?;
            validate_text("machine session", &row.session, MAX_SESSION_BYTES)?;
            Ok(SavedMachine {
                id: row.id,
                label: row.label,
                session: row.session,
                enabled: row.enabled,
            })
        })
        .collect()
}

async fn status_saved_machines(config: &HerdrConfig) -> Result<Vec<MachineStatusRow>, HerdrError> {
    let stdout = run_bounded(
        config,
        ["machine", "status", "--json"],
        MAX_MACHINE_OUTPUT_BYTES,
        HerdrCommand::Status,
    )
    .await?;
    let rows: Vec<MachineStatusRow> =
        serde_json::from_slice(&stdout).map_err(HerdrError::MachineStatusDecode)?;
    if rows.len() > MAX_MACHINES {
        return Err(HerdrError::MachineStatusAmbiguous("catalog".to_owned()));
    }
    let mut ids = std::collections::HashSet::new();
    for row in &rows {
        validate_machine_id(&row.id)?;
        validate_text("machine label", &row.label, MAX_MACHINE_LABEL_BYTES)?;
        if !ids.insert(row.id.as_str()) {
            return Err(HerdrError::MachineStatusAmbiguous(row.id.clone()));
        }
        // Herdr stderr/error text may contain SSH details. Deliberately discard it.
        let _ = row.error.as_deref();
    }
    Ok(rows)
}

fn connection_state(status: &MachineStatusRow) -> RuntimeEndpointConnectionState {
    match status.status.as_str() {
        "reachable" => RuntimeEndpointConnectionState::Reachable,
        "disabled" => RuntimeEndpointConnectionState::Disabled,
        "auth required" => RuntimeEndpointConnectionState::AuthenticationRequired,
        "error"
            if status.error.as_deref().is_some_and(|error| {
                let error = error.to_ascii_lowercase();
                error.contains("incompatible") || error.contains("update")
            }) =>
        {
            RuntimeEndpointConnectionState::Incompatible
        }
        "error" => RuntimeEndpointConnectionState::Unreachable,
        _ => RuntimeEndpointConnectionState::Unknown,
    }
}

fn sanitized_machine_inventory_error() -> HerdrError {
    HerdrError::MachineInventoryInvalid("forwarded snapshot failed validation".to_owned())
}

fn validate_inventory_bounds(inventory: &yard_domain::RuntimeInventory) -> Result<(), HerdrError> {
    let counts = [
        inventory.workspaces.len(),
        inventory.tabs.len(),
        inventory.panes.len(),
        inventory.workers.len(),
        inventory.child_agents.len(),
    ];
    if counts
        .into_iter()
        .any(|count| count > MAX_INVENTORY_RECORDS)
    {
        return Err(HerdrError::MachineInventoryInvalid(format!(
            "inventory collection exceeds {MAX_INVENTORY_RECORDS} records"
        )));
    }
    let ids = inventory
        .workspaces
        .iter()
        .map(|value| value.runtime_id.as_str())
        .chain(
            inventory
                .tabs
                .iter()
                .flat_map(|value| [value.runtime_id.as_str(), value.workspace_id.as_str()]),
        )
        .chain(inventory.panes.iter().flat_map(|value| {
            [
                value.runtime_id.as_str(),
                value.terminal_id.as_str(),
                value.workspace_id.as_str(),
                value.tab_id.as_str(),
            ]
        }))
        .chain(inventory.workers.iter().flat_map(|value| {
            [
                value.runtime_id.as_str(),
                value.terminal_id.as_str(),
                value.workspace_id.as_str(),
                value.tab_id.as_str(),
                value.pane_id.as_str(),
            ]
        }));
    if ids.into_iter().any(|id| {
        id.is_empty() || id.len() > MAX_RUNTIME_ID_BYTES || id.chars().any(char::is_control)
    }) {
        return Err(HerdrError::MachineInventoryInvalid(format!(
            "runtime identity must be 1 through {MAX_RUNTIME_ID_BYTES} bytes without control characters"
        )));
    }
    Ok(())
}

fn validate_machine_id(machine_id: &str) -> Result<(), HerdrError> {
    if machine_id.len() != MACHINE_ID_BYTES
        || !machine_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(HerdrError::InvalidMachineId);
    }
    Ok(())
}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), HerdrError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(HerdrError::MachineCatalogInvalid(format!(
            "{field} must be 1 through {maximum} bytes without control characters"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum HerdrCommand {
    Catalog,
    Status,
    Forward,
}

async fn run_bounded<const N: usize>(
    config: &HerdrConfig,
    args: [&str; N],
    output_limit: usize,
    kind: HerdrCommand,
) -> Result<Vec<u8>, HerdrError> {
    let mut command = Command::new(&config.binary);
    #[cfg(test)]
    if let Some(script) = &config.test_script {
        command.arg(script);
    }
    command
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(HerdrError::MachineCommandIo)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let operation = async move {
        let (stdout, _stderr, status) = tokio::try_join!(
            read_bounded(stdout, output_limit),
            read_bounded(stderr, 64 * 1024),
            async { child.wait().await.map_err(CommandRunError::Io) }
        )?;
        Ok::<_, CommandRunError>((stdout, status))
    };
    let (stdout, status) = timeout(config.request_timeout, operation)
        .await
        .map_err(|_| match kind {
            HerdrCommand::Catalog => HerdrError::MachineCatalogTimeout,
            HerdrCommand::Status => HerdrError::MachineStatusTimeout,
            HerdrCommand::Forward => HerdrError::MachineForwardTimeout,
        })?
        .map_err(CommandRunError::into_herdr)?;
    if !status.success() && !matches!(kind, HerdrCommand::Status) {
        return Err(match kind {
            HerdrCommand::Catalog => HerdrError::MachineCatalogFailed,
            HerdrCommand::Status => unreachable!("status failures retain structured stdout"),
            HerdrCommand::Forward => HerdrError::MachineForwardFailed,
        });
    }
    Ok(stdout)
}

enum CommandRunError {
    Io(io::Error),
    TooLarge(usize),
}

impl CommandRunError {
    fn into_herdr(self) -> HerdrError {
        match self {
            Self::Io(error) => HerdrError::MachineCommandIo(error),
            Self::TooLarge(limit) => HerdrError::MachineOutputTooLarge(limit),
        }
    }
}

async fn read_bounded(
    reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, CommandRunError> {
    let mut reader = reader.take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1));
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(CommandRunError::Io)?;
    if bytes.len() > limit {
        return Err(CommandRunError::TooLarge(limit));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use super::{inventory, list_saved_machines};
    use crate::{HerdrConfig, HerdrError};
    use yard_domain::RuntimeEndpointRef;

    fn script(body: &str) -> (tempfile::TempDir, HerdrConfig) {
        let temp = tempfile::tempdir().unwrap();
        let script_path = temp.path().join("script.sh");
        fs::write(&script_path, format!("set -eu\n{body}\n")).unwrap();
        let canonical_path = std::path::PathBuf::from("/bin/sh");
        (
            temp,
            HerdrConfig {
                binary: canonical_path.into_os_string(),
                request_timeout: Duration::from_millis(500),
                max_discovery_bytes: 1024 * 1024,
                max_response_bytes: 1024 * 1024,
                test_script: Some(script_path.into_os_string()),
                ..HerdrConfig::default()
            },
        )
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn catalog() -> String {
        format!(
            r#"[{{"id":"{ID}","label":"Build box","target":"secret-host","session":"default","enabled":true,"selected":false}}]"#
        )
    }

    #[tokio::test]
    async fn catalog_keeps_opaque_identity_across_label_rename() {
        let body = format!(r"printf '%s' '{}'", catalog());
        let (_temp, config) = script(&body);
        let first = list_saved_machines(&config).await.unwrap();
        let renamed_catalog = catalog().replace("Build box", "Renamed machine");
        let renamed_body = format!(r"printf '%s' '{renamed_catalog}'");
        let (_renamed_temp, renamed_config) = script(&renamed_body);
        let renamed = list_saved_machines(&renamed_config).await.unwrap();

        assert_eq!(first[0].id, renamed[0].id);
        assert_ne!(first[0].label, renamed[0].label);
    }

    #[tokio::test]
    async fn same_session_and_pane_ids_remain_distinct_by_machine_endpoint() {
        let fixture = include_str!("../tests/fixtures/v0.8.0/snapshot.json");
        let body = format!(
            r#"case "$*" in
              'machine list --json') printf '%s' '{}' ;;
              '--machine {ID} api snapshot') printf '%s' '{}' ;;
              *) echo unexpected-command > "{}"; exit 42 ;;
            esac"#,
            catalog(),
            fixture.replace('\\', "\\\\").replace('\'', "'\\''"),
            "/tmp/yard-herdr-unexpected-command"
        );
        let (_temp, config) = script(&body);
        let remote = inventory(&config, ID).await.unwrap();
        assert_eq!(
            remote.endpoint,
            RuntimeEndpointRef::Machine {
                machine_id: ID.to_owned()
            }
        );
        assert_eq!(remote.inventory.workers[0].pane_id, "w1:p1");
        let second_machine = RuntimeEndpointRef::Machine {
            machine_id: "ffffffffffffffffffffffffffffffff".to_owned(),
        };
        assert_ne!(remote.endpoint, second_machine);
    }

    #[tokio::test]
    async fn missing_and_disabled_catalog_entries_never_forward() {
        let disabled = catalog().replace("\"enabled\":true", "\"enabled\":false");
        let body = format!(r"printf '%s' '{disabled}'");
        let (_temp, config) = script(&body);
        let error = inventory(&config, ID).await.unwrap_err();
        assert!(matches!(error, HerdrError::MachineDisabled(_)));
        assert!(matches!(
            inventory(&config, "ffffffffffffffffffffffffffffffff").await,
            Err(HerdrError::MachineNotFound(_))
        ));
    }

    #[tokio::test]
    async fn invalid_machine_selector_never_reaches_herdr() {
        let (_temp, config) = script("echo command-ran >&2; exit 42");
        assert!(matches!(
            inventory(&config, "user@arbitrary-host").await,
            Err(HerdrError::InvalidMachineId)
        ));
    }

    #[tokio::test]
    async fn forwarded_timeout_and_malformed_snapshot_are_typed() {
        let timeout_body = format!(
            r#"case "$*" in
              'machine list --json') printf '%s' '{}' ;;
              '--machine {ID} api snapshot') sleep 1 ;;
              *) exit 42 ;;
            esac"#,
            catalog()
        );
        let (_temp, mut config) = script(&timeout_body);
        config.request_timeout = Duration::from_millis(20);
        assert!(matches!(
            inventory(&config, ID).await,
            Err(HerdrError::MachineForwardTimeout)
        ));

        let malformed_body = format!(
            r#"case "$*" in
              'machine list --json') printf '%s' '{}' ;;
              '--machine {ID} api snapshot') printf not-json ;;
              *) exit 42 ;;
            esac"#,
            catalog()
        );
        let (_temp, config) = script(&malformed_body);
        let error = inventory(&config, ID).await.unwrap_err();
        assert!(matches!(error, HerdrError::MachineInventoryInvalid(_)));
        assert_eq!(
            error.to_string(),
            "Herdr forwarded machine inventory is invalid: forwarded snapshot failed validation"
        );
    }

    #[tokio::test]
    async fn malformed_remote_topology_is_sanitized_at_the_inventory_boundary() {
        let secret_id = "snapshot-secret-user@raw-host";
        let ssh_stderr = "ssh stderr from raw-secret-host";
        let mut fixture: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json"))
                .unwrap();
        fixture["result"]["snapshot"]["agents"][0]["pane_id"] = secret_id.into();
        let fixture = serde_json::to_string(&fixture).unwrap();
        let body = format!(
            r#"case "$*" in
              'machine list --json') printf '%s' '{}' ;;
              '--machine {ID} api snapshot') printf '%s' '{}'; printf '%s' '{ssh_stderr}' >&2 ;;
              *) exit 42 ;;
            esac"#,
            catalog(),
            fixture.replace('\\', "\\\\").replace('\'', "'\\''"),
        );
        let (_temp, config) = script(&body);

        let error = inventory(&config, ID).await.unwrap_err();
        let rendered = error.to_string();
        assert!(matches!(error, HerdrError::MachineInventoryInvalid(_)));
        assert!(!rendered.contains(secret_id));
        assert!(!rendered.contains(ssh_stderr));
        assert_eq!(
            rendered,
            "Herdr forwarded machine inventory is invalid: forwarded snapshot failed validation"
        );
    }

    #[tokio::test]
    async fn duplicate_catalog_ids_are_rejected_before_status_or_forwarding_regardless_of_order() {
        let second_id = "11111111111111111111111111111111";
        let row = |id: &str, label: &str| {
            format!(
                r#"{{"id":"{id}","label":"{label}","target":"secret-host","session":"default","enabled":true,"selected":false}}"#
            )
        };
        let duplicate_orders = [
            format!(
                "[{}, {}, {}]",
                row(ID, "first"),
                row(second_id, "other"),
                row(ID, "last")
            ),
            format!(
                "[{}, {}, {}]",
                row(second_id, "other"),
                row(ID, "first"),
                row(ID, "last")
            ),
        ];

        for (index, duplicate_catalog) in duplicate_orders.into_iter().enumerate() {
            for operation in ["inventory", "endpoints"] {
                let temp = tempfile::tempdir().unwrap();
                let marker = temp.path().join(format!("unexpected-{index}-{operation}"));
                let body = format!(
                    r#"case "$*" in
                      'session list --json') printf '%s' '{{"sessions":[]}}' ;;
                      'machine list --json') printf '%s' '{duplicate_catalog}' ;;
                      'machine status --json'|'--machine '*) printf ran > '{}' ;;
                      *) exit 42 ;;
                    esac"#,
                    marker.display()
                );
                let (_script_temp, config) = script(&body);
                let error = if operation == "inventory" {
                    inventory(&config, ID).await.unwrap_err()
                } else {
                    super::endpoints(&config).await.unwrap_err()
                };

                assert!(matches!(error, HerdrError::MachineCatalogInvalid(_)));
                assert_eq!(
                    error.to_string(),
                    "Herdr machine catalog is invalid: catalog contains duplicate machine IDs"
                );
                assert!(
                    !marker.exists(),
                    "{operation} executed after duplicate catalog"
                );
            }
        }
    }

    #[tokio::test]
    async fn ambiguous_status_keeps_saved_machine_visible_as_unknown() {
        let body = format!(
            r#"case "$*" in
              'session list --json') printf '%s' '{{"sessions":[]}}' ;;
              'machine list --json') printf '%s' '{}' ;;
              'machine status --json') printf '%s' '[{{"id":"{ID}","label":"Build box","status":"reachable","error":null}},{{"id":"{ID}","label":"Build box","status":"reachable","error":null}}]' ;;
              *) exit 42 ;;
            esac"#,
            catalog()
        );
        let (_temp, config) = script(&body);
        let endpoints = super::endpoints(&config).await.unwrap();
        assert_eq!(endpoints.endpoints.len(), 2);
        assert_eq!(
            endpoints.endpoints[1].connection_state,
            yard_domain::RuntimeEndpointConnectionState::Unknown
        );
        assert!(!endpoints.endpoints[1].capabilities.inventory_read);
    }

    #[tokio::test]
    async fn malformed_oversized_timeout_and_nonzero_are_typed() {
        let (_temp, config) = script("printf not-json");
        assert!(matches!(
            list_saved_machines(&config).await,
            Err(HerdrError::MachineCatalogDecode(_))
        ));

        let (_temp, config) = script("head -c 1048577 /dev/zero");
        assert!(matches!(
            list_saved_machines(&config).await,
            Err(HerdrError::MachineOutputTooLarge(_))
        ));

        let (_temp, mut config) = script("sleep 1");
        config.request_timeout = Duration::from_millis(20);
        assert!(matches!(
            list_saved_machines(&config).await,
            Err(HerdrError::MachineCatalogTimeout)
        ));

        let (_temp, config) = script("echo secret >&2; exit 7");
        assert!(matches!(
            list_saved_machines(&config).await,
            Err(HerdrError::MachineCatalogFailed)
        ));
    }

    #[tokio::test]
    async fn forwarded_failure_does_not_expose_stderr_or_retry_local() {
        let body = format!(
            r#"case "$*" in
              'machine list --json') printf '%s' '{}' ;;
              '--machine {ID} api snapshot') echo 'ssh user@secret-host failed' >&2; exit 1 ;;
              *) echo local-fallback >&2; exit 42 ;;
            esac"#,
            catalog()
        );
        let (_temp, config) = script(&body);
        let error = inventory(&config, ID).await.unwrap_err();
        assert!(matches!(error, HerdrError::MachineForwardFailed));
        assert!(!error.to_string().contains("secret-host"));
    }
}

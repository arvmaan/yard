use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSession {
    pub name: String,
    pub is_default: bool,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSessions {
    pub adapter: String,
    pub sessions: Vec<RuntimeSession>,
}

/// Stable runtime location. Machine labels are display metadata and never
/// participate in identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeEndpointRef {
    #[default]
    Local,
    Machine {
        machine_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEndpointConnectionState {
    Reachable,
    Disabled,
    AuthenticationRequired,
    Unreachable,
    Incompatible,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEndpointCapabilities {
    pub inventory_read: bool,
    pub mutations: bool,
    pub terminal_streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEndpointSession {
    pub name: String,
    pub is_default: bool,
    /// `None` means the endpoint was not contacted successfully, not stopped.
    pub observed_running: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEndpoint {
    pub endpoint: RuntimeEndpointRef,
    pub label: String,
    pub enabled: bool,
    pub connection_state: RuntimeEndpointConnectionState,
    pub capabilities: RuntimeEndpointCapabilities,
    pub sessions: Vec<RuntimeEndpointSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEndpoints {
    pub adapter: String,
    pub endpoints: Vec<RuntimeEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointRuntimeInventory {
    pub endpoint: RuntimeEndpointRef,
    pub inventory: RuntimeInventory,
}

impl EndpointRuntimeInventory {
    /// Existing durable runtime bindings are Local until schema 40 adds
    /// endpoint identity. Remote observations must not cross that boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeEndpointBoundaryError`] for every non-Local endpoint.
    pub fn into_local_inventory(self) -> Result<RuntimeInventory, RuntimeEndpointBoundaryError> {
        if self.endpoint == RuntimeEndpointRef::Local {
            Ok(self.inventory)
        } else {
            Err(RuntimeEndpointBoundaryError::RemotePersistenceUnsupported)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeEndpointBoundaryError {
    #[error("remote runtime persistence is not supported until endpoint identity is durable")]
    RemotePersistenceUnsupported,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusObservation {
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeObservation {
    pub repository_key: String,
    pub repository_name: String,
    pub repository_root: String,
    pub checkout_path: String,
    pub is_linked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceObservation {
    pub runtime_id: String,
    pub order: usize,
    pub label: String,
    pub focused: bool,
    pub active_tab_id: String,
    pub pane_count: usize,
    pub tab_count: usize,
    pub status: ObservedStatus,
    pub tokens: BTreeMap<String, String>,
    pub worktree: Option<WorktreeObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabObservation {
    pub runtime_id: String,
    pub workspace_id: String,
    pub order: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub status: ObservedStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSessionRef {
    pub source: String,
    pub provider: String,
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneObservation {
    pub runtime_id: String,
    #[serde(default)]
    pub pane_instance_id: Option<String>,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub label: Option<String>,
    pub provider: Option<String>,
    pub display_provider: Option<String>,
    pub status: ObservedStatus,
    pub tokens: BTreeMap<String, String>,
    pub provider_session: Option<ProviderSessionRef>,
    #[serde(with = "crate::serde_u64")]
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedWorker {
    pub runtime_id: String,
    #[serde(default)]
    pub pane_instance_id: Option<String>,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub display_provider: Option<String>,
    pub status: ObservedStatus,
    pub focused: bool,
    pub launch_pending: bool,
    pub interactive_ready: bool,
    #[serde(with = "crate::serde_u64")]
    pub state_change_sequence: u64,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub tokens: BTreeMap<String, String>,
    pub provider_session: Option<ProviderSessionRef>,
    #[serde(with = "crate::serde_u64")]
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedChildAgent {
    pub runtime_id: String,
    pub parent_provider_session: ProviderSessionRef,
    pub parent_agent_id: Option<String>,
    pub provider: String,
    pub provider_agent_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub role: Option<String>,
    pub status: ObservedStatus,
    pub depth: u32,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInventory {
    pub adapter: String,
    pub session: String,
    pub runtime_version: String,
    pub protocol: u32,
    pub observed_at_unix_ms: u64,
    pub focus: FocusObservation,
    pub workspaces: Vec<WorkspaceObservation>,
    pub tabs: Vec<TabObservation>,
    pub panes: Vec<PaneObservation>,
    pub workers: Vec<ObservedWorker>,
    #[serde(default)]
    pub child_agents: Vec<ObservedChildAgent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRuntimeWorkspaceKind {
    YardCentral,
    Coordination,
    Provisioning,
    Quarantined,
    CleanupPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRuntimeOccupantKind {
    Provisioning,
    Quarantined,
    CleanupPending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedRuntimeOccupant {
    pub kind: ManagedRuntimeOccupantKind,
    pub terminal_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub label: String,
    pub reason: String,
    pub project_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedRuntimeWorkspace {
    pub workspace_id: String,
    pub kind: ManagedRuntimeWorkspaceKind,
    pub label: String,
    pub occupants: Vec<ManagedRuntimeOccupant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTopology {
    pub adapter: String,
    pub session: String,
    pub managed_workspaces: Vec<ManagedRuntimeWorkspace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReconciliation {
    pub adapter: String,
    pub session: String,
    pub observed_at_unix_ms: u64,
    pub adopted_workers: usize,
    pub updated_bindings: usize,
    pub missing_bindings: usize,
    pub ambiguous_bindings: usize,
    pub exited_processes: usize,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        EndpointRuntimeInventory, FocusObservation, ObservedStatus, PaneObservation,
        RuntimeEndpointBoundaryError, RuntimeEndpointRef, RuntimeInventory,
    };

    #[test]
    fn serializes_runtime_counters_without_javascript_precision_loss() {
        let pane = PaneObservation {
            runtime_id: "pane".to_owned(),
            pane_instance_id: None,
            terminal_id: "terminal".to_owned(),
            workspace_id: "workspace".to_owned(),
            tab_id: "tab".to_owned(),
            focused: false,
            cwd: None,
            foreground_cwd: None,
            label: None,
            provider: None,
            display_provider: None,
            status: ObservedStatus::Unknown,
            tokens: BTreeMap::new(),
            provider_session: None,
            revision: u64::MAX,
        };

        let json = serde_json::to_value(pane).unwrap();

        assert_eq!(json["revision"], u64::MAX.to_string());
    }

    fn inventory() -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.9.3".to_owned(),
            protocol: 22,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            workers: Vec::new(),
            child_agents: Vec::new(),
        }
    }

    #[test]
    fn endpoint_identity_is_structured_and_label_independent() {
        let endpoint = RuntimeEndpointRef::Machine {
            machine_id: "0123456789abcdef0123456789abcdef".to_owned(),
        };
        let encoded = serde_json::to_value(&endpoint).unwrap();

        assert_eq!(encoded["kind"], "machine");
        assert_eq!(encoded["machine_id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(
            serde_json::from_value::<RuntimeEndpointRef>(encoded).unwrap(),
            endpoint
        );
    }

    #[test]
    fn remote_inventory_cannot_cross_local_persistence_boundary() {
        let remote = EndpointRuntimeInventory {
            endpoint: RuntimeEndpointRef::Machine {
                machine_id: "0123456789abcdef0123456789abcdef".to_owned(),
            },
            inventory: inventory(),
        };
        assert_eq!(
            remote.into_local_inventory(),
            Err(RuntimeEndpointBoundaryError::RemotePersistenceUnsupported)
        );

        let local = EndpointRuntimeInventory {
            endpoint: RuntimeEndpointRef::Local,
            inventory: inventory(),
        };
        assert_eq!(local.into_local_inventory().unwrap().session, "default");
    }
}

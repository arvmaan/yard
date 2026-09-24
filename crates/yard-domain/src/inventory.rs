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

    use super::{ObservedStatus, PaneObservation};

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
}

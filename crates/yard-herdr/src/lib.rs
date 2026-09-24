mod config;
mod control;
mod discovery;
mod error;
mod management;
mod normalize;
mod session;
mod socket;
mod terminal;
mod wire;

use std::time::{SystemTime, UNIX_EPOCH};

pub use config::HerdrConfig;
pub use control::{
    BootstrapAgentRequest, HerdrControlError, PaneOutput, PrepareAgentRequest,
    PrepareWorkspaceAgentRequest, PreparedAgent, PromptAgentRequest, PromptedAgent,
    ProvisionAgentRequest, ProvisionedAgent, ReadPaneRequest, StartPreparedAgentRequest,
};
pub use error::HerdrError;
pub use management::{
    AcquirePaneLeaseRequest, CloseLeasedPaneRequest, LeaseToken, PaneLease,
    PaneLeaseOperationResult, PaneLeaseStatus, PaneLeaseStatusRequest, PaneManagementCapability,
    PaneManagementRpcError, ReleasePaneLeaseRequest, RenewPaneLeaseRequest,
};
pub use terminal::{
    HerdrTerminal, HerdrTerminalError, MAX_TERMINAL_COLS, MAX_TERMINAL_COMMAND_LINE_BYTES,
    MAX_TERMINAL_EVENT_LINE_BYTES, MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_ROWS, MIN_TERMINAL_COLS,
    MIN_TERMINAL_ROWS, OpenTerminalRequest, TerminalClosed, TerminalCommand, TerminalDimensions,
    TerminalEncoding, TerminalEvent, TerminalFrame, TerminalInput,
};
use yard_domain::{RuntimeInventory, RuntimeSessions};

#[derive(Debug, Clone)]
pub struct DiscoveredHerdrSession {
    session: discovery::HerdrSession,
}

impl DiscoveredHerdrSession {
    #[must_use]
    pub fn id(&self) -> &str {
        self.session.session_dir.to_str().unwrap_or_default()
    }

    #[must_use]
    pub fn snapshot_key(&self) -> &std::path::Path {
        &self.session.socket_path
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.session.name
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        self.session.is_default
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.session.running
    }
}

#[derive(Debug, Clone)]
pub struct HerdrAdapter {
    config: HerdrConfig,
}

impl HerdrAdapter {
    #[must_use]
    pub fn new(config: HerdrConfig) -> Self {
        Self { config }
    }

    /// Discover the Herdr sessions visible to the configured CLI.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the CLI cannot be executed, times out,
    /// reports failure, or returns an invalid session document.
    pub async fn sessions(&self) -> Result<RuntimeSessions, HerdrError> {
        let sessions = self.discover_sessions().await?;
        Ok(RuntimeSessions {
            adapter: "herdr".to_owned(),
            sessions: sessions
                .into_iter()
                .map(|session| session.session.into_summary())
                .collect(),
        })
    }

    /// Discover session descriptors that can be snapshotted without another
    /// session-list operation.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when session discovery fails.
    pub async fn discover_sessions(&self) -> Result<Vec<DiscoveredHerdrSession>, HerdrError> {
        discovery::discover_sessions(&self.config)
            .await
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| DiscoveredHerdrSession { session })
                    .collect()
            })
    }

    /// Snapshot and normalize one explicitly selected Herdr session.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the session is missing or stopped, the
    /// socket request fails, the protocol differs, or topology is inconsistent.
    pub async fn inventory(&self, session_name: &str) -> Result<RuntimeInventory, HerdrError> {
        let sessions = self.discover_sessions().await?;
        let session = sessions
            .into_iter()
            .find(|session| session.name() == session_name)
            .ok_or_else(|| HerdrError::SessionNotFound(session_name.to_owned()))?;
        self.inventory_for_discovered_session(&session).await
    }

    /// Snapshot a previously discovered session without listing sessions again.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the discovered session was stopped, the
    /// socket request fails, the protocol differs, or topology is inconsistent.
    pub async fn inventory_for_discovered_session(
        &self,
        session: &DiscoveredHerdrSession,
    ) -> Result<RuntimeInventory, HerdrError> {
        let (snapshot, observed_at_unix_ms) = self.snapshot_for_discovered_session(session).await?;
        normalize::normalize(snapshot, session.name(), observed_at_unix_ms, &self.config)
    }

    /// Snapshot one discovered session for fleet projection while preserving
    /// every live agent observation, including duplicate/conflicting evidence.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the discovered session was stopped, the
    /// socket request fails, the protocol differs, or topology is inconsistent.
    pub async fn fleet_inventory_for_discovered_session(
        &self,
        session: &DiscoveredHerdrSession,
    ) -> Result<RuntimeInventory, HerdrError> {
        let (snapshot, observed_at_unix_ms) = self.snapshot_for_discovered_session(session).await?;
        normalize::normalize_fleet(snapshot, session.id(), observed_at_unix_ms, &self.config)
    }

    async fn snapshot_for_discovered_session(
        &self,
        session: &DiscoveredHerdrSession,
    ) -> Result<(wire::SessionSnapshot, u64), HerdrError> {
        if !session.running() {
            return Err(HerdrError::SessionNotRunning(session.name().to_owned()));
        }

        let snapshot = socket::request_snapshot(&self.config, &session.session.socket_path).await?;
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        Ok((snapshot, observed_at_unix_ms))
    }

    /// Ensure one named persistent Herdr server is running.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when session discovery or server startup fails.
    pub async fn ensure_session(
        &self,
        session_name: &str,
        startup_cwd: &std::path::Path,
    ) -> Result<(), HerdrError> {
        session::ensure_session(&self.config, session_name, startup_cwd).await
    }

    /// Create one owned tab, start a supported agent, and deliver one prompt.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when resource creation, agent start, or
    /// prompt delivery fails. Prompt delivery failures retain the provisioned
    /// runtime binding so Yard can persist the failed attempt.
    pub async fn provision_agent(
        &self,
        request: ProvisionAgentRequest,
    ) -> Result<ProvisionedAgent, HerdrControlError> {
        control::provision_agent(&self.config, request).await
    }

    /// Create one workspace and start and prompt an agent in its root pane.
    ///
    /// Herdr request IDs are correlation IDs, not idempotency keys. This
    /// operation never retries workspace creation.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when workspace creation, agent start,
    /// rollback, or prompt delivery fails.
    pub async fn bootstrap_agent(
        &self,
        request: BootstrapAgentRequest,
    ) -> Result<ProvisionedAgent, HerdrControlError> {
        control::bootstrap_agent(&self.config, request).await
    }

    /// Create an owned target tab without starting an agent.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when the session or tab creation fails.
    pub async fn prepare_agent(
        &self,
        request: PrepareAgentRequest,
    ) -> Result<PreparedAgent, HerdrControlError> {
        control::prepare_agent(&self.config, request).await
    }

    /// Create one workspace and return its root pane without starting an agent.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when the session or workspace creation fails.
    pub async fn prepare_workspace_agent(
        &self,
        request: PrepareWorkspaceAgentRequest,
    ) -> Result<PreparedAgent, HerdrControlError> {
        control::prepare_workspace_agent(&self.config, request).await
    }

    /// Start and prompt an agent in a previously prepared tab.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when agent start or prompt delivery fails.
    pub async fn start_prepared_agent(
        &self,
        request: StartPreparedAgentRequest,
    ) -> Result<ProvisionedAgent, HerdrControlError> {
        control::start_prepared_agent(&self.config, request).await
    }

    /// Start and prompt an agent in an existing retained shell pane.
    ///
    /// Unlike prepared-runtime startup, a failure never closes the existing
    /// tab or workspace because Yard did not create that topology in this
    /// operation.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when agent start or prompt delivery fails.
    pub async fn start_existing_agent(
        &self,
        request: StartPreparedAgentRequest,
    ) -> Result<ProvisionedAgent, HerdrControlError> {
        control::start_existing_agent(&self.config, request).await
    }

    /// Start and prompt an agent in a previously prepared workspace root pane.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrControlError`] when agent start, workspace rollback, or prompt delivery
    /// fails.
    pub async fn start_prepared_workspace_agent(
        &self,
        request: StartPreparedAgentRequest,
    ) -> Result<ProvisionedAgent, HerdrControlError> {
        control::start_prepared_workspace_agent(&self.config, request).await
    }

    /// Submit one prompt to an existing Herdr agent without waiting for a
    /// provider turn to settle.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the session or target is unavailable,
    /// submission is rejected, or transport is ambiguous.
    pub async fn prompt_agent(
        &self,
        request: PromptAgentRequest,
    ) -> Result<PromptedAgent, HerdrError> {
        control::prompt_agent(&self.config, request).await
    }

    /// Read a bounded recent text snapshot from one Herdr pane.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the session or pane is unavailable or the
    /// response does not match the pinned protocol.
    pub async fn read_pane(&self, request: ReadPaneRequest) -> Result<PaneOutput, HerdrError> {
        control::read_pane(&self.config, request).await
    }

    pub async fn pane_management_capability(
        &self,
        session_name: &str,
    ) -> Result<PaneManagementCapability, PaneManagementRpcError> {
        management::capability(&self.config, session_name).await
    }

    pub async fn acquire_pane_lease(
        &self,
        request: AcquirePaneLeaseRequest,
    ) -> Result<PaneLease, PaneManagementRpcError> {
        management::acquire(&self.config, request).await
    }

    pub async fn renew_pane_lease(
        &self,
        request: RenewPaneLeaseRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        management::renew(&self.config, request).await
    }

    pub async fn release_pane_lease(
        &self,
        request: ReleasePaneLeaseRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        management::release(&self.config, request).await
    }

    pub async fn pane_lease_status(
        &self,
        request: PaneLeaseStatusRequest,
    ) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
        management::status(&self.config, request).await
    }

    pub async fn close_if_pane_leased(
        &self,
        request: CloseLeasedPaneRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        management::close_if_leased(&self.config, request).await
    }

    /// Open one interactive Herdr terminal controller process.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError`] when the controller cannot be spawned or
    /// the request falls outside the terminal adapter's validated bounds.
    pub fn open_terminal(
        &self,
        request: OpenTerminalRequest,
    ) -> Result<HerdrTerminal, HerdrTerminalError> {
        HerdrTerminal::spawn(&self.config, request)
    }
}

impl Default for HerdrAdapter {
    fn default() -> Self {
        Self::new(HerdrConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    use super::{DiscoveredHerdrSession, HerdrAdapter, HerdrConfig};

    #[tokio::test]
    async fn snapshots_a_discovered_session_without_listing_again() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let fixture: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json",))
                .unwrap();
        let mut fixture = serde_json::to_vec(&fixture).unwrap();
        fixture.push(b'\n');
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut request = String::new();
            BufReader::new(reader)
                .read_line(&mut request)
                .await
                .unwrap();
            writer.write_all(&fixture).await.unwrap();
        });
        let adapter = HerdrAdapter::new(HerdrConfig {
            binary: PathBuf::from("session-discovery-must-not-run").into_os_string(),
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        });
        let session = DiscoveredHerdrSession {
            session: crate::discovery::HerdrSession {
                name: "default".to_owned(),
                is_default: true,
                running: true,
                session_dir: socket_path.parent().unwrap().to_owned(),
                socket_path,
            },
        };

        let inventory = adapter
            .inventory_for_discovered_session(&session)
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(inventory.session, "default");
        assert!(!inventory.panes.is_empty());
    }

    #[tokio::test]
    async fn fleet_snapshot_preserves_conflicting_agent_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let mut fixture: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json"))
                .unwrap();
        let panes = fixture["result"]["snapshot"]["panes"]
            .as_array_mut()
            .unwrap();
        let mut conflicting_pane = panes[0].clone();
        conflicting_pane["tab_id"] = "conflicting-tab".into();
        panes.push(conflicting_pane);
        let agents = fixture["result"]["snapshot"]["agents"]
            .as_array_mut()
            .unwrap();
        let mut conflicting = agents[0].clone();
        conflicting["name"] = "conflicting-agent".into();
        conflicting["agent"] = "claude".into();
        conflicting["display_agent"] = "Claude".into();
        conflicting["agent_session"]["source"] = "herdr:claude".into();
        conflicting["agent_session"]["agent"] = "claude".into();
        conflicting["agent_session"]["value"] = "session-conflict".into();
        agents.push(conflicting);
        let mut fixture = serde_json::to_vec(&fixture).unwrap();
        fixture.push(10);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut request = String::new();
            BufReader::new(reader)
                .read_line(&mut request)
                .await
                .unwrap();
            writer.write_all(&fixture).await.unwrap();
        });
        let adapter = HerdrAdapter::new(HerdrConfig {
            binary: PathBuf::from("session-discovery-must-not-run").into_os_string(),
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        });
        let session = DiscoveredHerdrSession {
            session: crate::discovery::HerdrSession {
                name: "default".to_owned(),
                is_default: true,
                running: true,
                session_dir: socket_path.parent().unwrap().to_owned(),
                socket_path,
            },
        };

        let inventory = adapter
            .fleet_inventory_for_discovered_session(&session)
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(inventory.panes.len(), 2);
        assert_eq!(inventory.panes[0].runtime_id, inventory.panes[1].runtime_id);
        assert_ne!(inventory.panes[0].tab_id, inventory.panes[1].tab_id);
        assert_eq!(inventory.workers.len(), 2);
        assert_eq!(inventory.workers[0].pane_id, inventory.workers[1].pane_id);
        assert_ne!(inventory.workers[0].provider, inventory.workers[1].provider);
    }

    #[tokio::test]
    async fn fleet_snapshot_preserves_missing_and_conflicting_pane_ancestry() {
        let base: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json"))
                .unwrap();
        let mut missing_workspace = base.clone();
        missing_workspace["result"]["snapshot"]["workspaces"] = serde_json::json!([]);
        let mut missing_tab = base.clone();
        missing_tab["result"]["snapshot"]["tabs"] = serde_json::json!([]);
        let mut conflicting = base;
        conflicting["result"]["snapshot"]["tabs"][0]["workspace_id"] =
            "conflicting-workspace".into();
        let fixtures = [missing_workspace, missing_tab, conflicting].map(|fixture| {
            let mut bytes = serde_json::to_vec(&fixture).unwrap();
            bytes.push(b'\n');
            bytes
        });

        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for fixture in fixtures {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut request = String::new();
                BufReader::new(reader)
                    .read_line(&mut request)
                    .await
                    .unwrap();
                writer.write_all(&fixture).await.unwrap();
            }
        });
        let adapter = HerdrAdapter::new(HerdrConfig {
            binary: PathBuf::from("session-discovery-must-not-run").into_os_string(),
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        });
        let session = DiscoveredHerdrSession {
            session: crate::discovery::HerdrSession {
                name: "default".to_owned(),
                is_default: true,
                running: true,
                session_dir: socket_path.parent().unwrap().to_owned(),
                socket_path,
            },
        };

        let missing_workspace = adapter
            .fleet_inventory_for_discovered_session(&session)
            .await
            .unwrap();
        assert!(missing_workspace.workspaces.is_empty());
        assert_eq!(missing_workspace.panes.len(), 1);
        let missing_tab = adapter
            .fleet_inventory_for_discovered_session(&session)
            .await
            .unwrap();
        assert!(missing_tab.tabs.is_empty());
        assert_eq!(missing_tab.panes.len(), 1);
        let conflicting = adapter
            .fleet_inventory_for_discovered_session(&session)
            .await
            .unwrap();
        assert_ne!(
            conflicting.panes[0].workspace_id,
            conflicting.tabs[0].workspace_id
        );
        server.await.unwrap();
    }
}

mod config;
mod control;
mod discovery;
mod error;
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
pub use terminal::{
    HerdrTerminal, HerdrTerminalError, MAX_TERMINAL_COLS, MAX_TERMINAL_COMMAND_LINE_BYTES,
    MAX_TERMINAL_EVENT_LINE_BYTES, MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_ROWS, MIN_TERMINAL_COLS,
    MIN_TERMINAL_ROWS, OpenTerminalRequest, TerminalClosed, TerminalCommand, TerminalDimensions,
    TerminalEncoding, TerminalEvent, TerminalFrame, TerminalInput,
};
use yard_domain::{RuntimeInventory, RuntimeSessions};

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
        let sessions = discovery::discover_sessions(&self.config).await?;
        Ok(RuntimeSessions {
            adapter: "herdr".to_owned(),
            sessions: sessions
                .into_iter()
                .map(discovery::HerdrSession::into_summary)
                .collect(),
        })
    }

    /// Snapshot and normalize one explicitly selected Herdr session.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrError`] when the session is missing or stopped, the
    /// socket request fails, the protocol differs, or topology is inconsistent.
    pub async fn inventory(&self, session_name: &str) -> Result<RuntimeInventory, HerdrError> {
        let sessions = discovery::discover_sessions(&self.config).await?;
        let session = sessions
            .into_iter()
            .find(|session| session.name == session_name)
            .ok_or_else(|| HerdrError::SessionNotFound(session_name.to_owned()))?;

        if !session.running {
            return Err(HerdrError::SessionNotRunning(session_name.to_owned()));
        }

        let snapshot = socket::request_snapshot(&self.config, &session.socket_path).await?;
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        normalize::normalize(snapshot, session_name, observed_at_unix_ms, &self.config)
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

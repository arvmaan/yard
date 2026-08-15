use std::{collections::HashSet, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::Mutex,
    time::{Instant, sleep},
};
use yard_domain::{
    ConfigureYardOrchestrator, ConfiguredYardOrchestrator, ObservedWorker,
    ProvisionYardOrchestrator, RecoverYardOrchestrator, RecoveredYardOrchestrator,
    RuntimeInventory, RuntimeObservationState, RuntimeProcessState, WorkerAvailability,
};
use yard_store::{ProjectStoreError, YardStore};

use crate::{
    allocation_service::{
        AllocationServiceError, RuntimeControl, RuntimeProvisionError, RuntimeSessionRequest,
        RuntimeWorkspaceProvisionRequest, assignment_prompt, provider_args,
        validate_supported_profile,
    },
    reconciliation_service::{ReconciliationService, ReconciliationServiceError},
    status_protocol::with_orchestrator_status_contract,
};

pub const YARD_ORCHESTRATOR_SESSION: &str = "yard-orchestrator";
pub const YARD_ORCHESTRATOR_WORKSPACE_LABEL: &str = "Yard central coordination";
pub const YARD_ORCHESTRATOR_AGENT_NAME: &str = "yard-orchestrator";

const RUNTIME_IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);
const CENTRAL_COORDINATION_OBJECTIVE: &str = "Coordinate all Yard projects from this dedicated \
    central session. Maintain portfolio-wide priorities, route work to project orchestrators, \
    surface blockers, and leave project-scoped implementation to those project teams.";

#[derive(Clone)]
pub struct YardOrchestratorService {
    runtime: Arc<dyn RuntimeControl>,
    store: Arc<dyn YardStore>,
    reconciliation: ReconciliationService,
    operation: Arc<Mutex<()>>,
    cwd: String,
}

impl YardOrchestratorService {
    /// Build the singleton provisioner with a backend-resolved stable local
    /// CWD. The same absolute path is used for the Herdr server and workspace.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn RuntimeControl>,
        store: Arc<dyn YardStore>,
        reconciliation: ReconciliationService,
        cwd: String,
    ) -> Self {
        Self {
            runtime,
            store,
            reconciliation,
            operation: Arc::default(),
            cwd,
        }
    }

    /// Ensure, adopt, and configure the dedicated Yard orchestrator.
    ///
    /// Calls are serialized in-process. Every attempt snapshots and
    /// reconciles the deterministic Herdr session before deciding whether a
    /// new workspace is needed.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorServiceError`] for invalid or stale commands,
    /// unsupported profiles, Herdr failures, missing reconciled identities, or
    /// persistence failures.
    pub async fn provision(
        &self,
        command: ProvisionYardOrchestrator,
    ) -> Result<ConfiguredYardOrchestrator, YardOrchestratorServiceError> {
        let command = command.normalize()?;
        let _operation = self.operation.lock().await;

        let profile = self.store.get_worker_profile(&command.profile_id).await?;
        if profile.version != command.expected_profile_version {
            return Err(ProjectStoreError::ProfileVersionConflict {
                current_version: profile.version,
            }
            .into());
        }
        validate_supported_profile(&profile)
            .map_err(YardOrchestratorServiceError::UnsupportedProfile)?;
        let args = provider_args(&profile).map_err(|error| match error {
            AllocationServiceError::UnsupportedProfile(message) => {
                YardOrchestratorServiceError::UnsupportedProfile(message)
            }
            other => YardOrchestratorServiceError::UnsupportedProfile(other.to_string()),
        })?;

        self.runtime
            .ensure_session(RuntimeSessionRequest {
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                startup_cwd: self.cwd.clone(),
            })
            .await
            .map_err(runtime_error)?;

        let current = self.store.get_yard_orchestrator().await?;
        let (inventory, _) = self
            .reconciliation
            .refresh(YARD_ORCHESTRATOR_SESSION)
            .await?;
        let mut observed = dedicated_worker(&inventory, current.worker.as_ref());

        if observed.is_none() {
            let runtime = self
                .runtime
                .bootstrap_worker(RuntimeWorkspaceProvisionRequest {
                    command_id: command.command_id.clone(),
                    session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                    workspace_label: YARD_ORCHESTRATOR_WORKSPACE_LABEL.to_owned(),
                    cwd: self.cwd.clone(),
                    agent_name: YARD_ORCHESTRATOR_AGENT_NAME.to_owned(),
                    kind: profile.spec.provider.clone(),
                    args,
                    prompt: with_orchestrator_status_contract(
                        &assignment_prompt(
                            CENTRAL_COORDINATION_OBJECTIVE,
                            "central orchestrator",
                            &profile,
                        ),
                        &command.command_id,
                    ),
                })
                .await
                .map_err(runtime_error)?;
            observed = Some(
                self.reconcile_provisioned_worker(&runtime.terminal_id)
                    .await?,
            );
        }

        let observed = observed.ok_or(YardOrchestratorServiceError::RuntimeBindingUnverified)?;
        let candidates = self.store.list_worker_candidates().await?;
        let candidate = candidates
            .workers
            .into_iter()
            .find(|candidate| {
                candidate.worker.runtime.as_ref().is_some_and(|runtime| {
                    runtime.adapter == "herdr"
                        && runtime.session == YARD_ORCHESTRATOR_SESSION
                        && runtime.terminal_id == observed.terminal_id
                })
            })
            .ok_or(YardOrchestratorServiceError::ReconciledWorkerMissing)?;
        if !matches!(
            candidate.availability,
            WorkerAvailability::UnassignedLive | WorkerAvailability::YardOrchestrator
        ) {
            return Err(ProjectStoreError::WorkerNotAvailable {
                availability: candidate.availability,
            }
            .into());
        }
        let worker = self
            .store
            .pin_worker_profile(
                &candidate.worker.id,
                &profile.id,
                command.expected_profile_version,
            )
            .await?;

        self.store
            .configure_yard_orchestrator(ConfigureYardOrchestrator {
                command_id: command.command_id,
                actor: command.actor,
                worker_id: worker.id,
                expected_worker_version: worker.version,
                expected_orchestrator_version: command.expected_orchestrator_version,
            })
            .await
            .map_err(Into::into)
    }

    /// Restart the dedicated Herdr session and restore the existing Yard
    /// orchestrator binding without provisioning or changing ownership.
    ///
    /// # Errors
    ///
    /// Returns [`YardOrchestratorServiceError`] when the command is stale, the
    /// current orchestrator is not dedicated, Herdr cannot restart, or the
    /// configured worker cannot be restored unambiguously.
    pub async fn recover(
        &self,
        command: RecoverYardOrchestrator,
    ) -> Result<RecoveredYardOrchestrator, YardOrchestratorServiceError> {
        let command = command.normalize()?;
        let _operation = self.operation.lock().await;
        let current = self.store.get_yard_orchestrator().await?;
        if current.version != command.expected_orchestrator_version {
            return Err(ProjectStoreError::YardOrchestratorVersionConflict {
                current_version: current.version,
            }
            .into());
        }
        let current_worker = current
            .worker
            .as_ref()
            .ok_or(YardOrchestratorServiceError::RecoveryNotConfigured)?;
        let current_runtime = current_worker
            .runtime
            .as_ref()
            .filter(|runtime| runtime.session == YARD_ORCHESTRATOR_SESSION)
            .ok_or(YardOrchestratorServiceError::RecoveryNotDedicated)?;

        self.runtime
            .ensure_session(RuntimeSessionRequest {
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                startup_cwd: self.cwd.clone(),
            })
            .await
            .map_err(runtime_error)?;

        let (inventory, _) = self
            .reconciliation
            .refresh(YARD_ORCHESTRATOR_SESSION)
            .await?;
        let restored = dedicated_worker(&inventory, current.worker.as_ref())
            .ok_or(YardOrchestratorServiceError::RecoveryBindingMissing)?;
        if current_runtime.provider_session.is_some()
            && restored.provider_session != current_runtime.provider_session
        {
            return Err(YardOrchestratorServiceError::RecoveryBindingAmbiguous);
        }

        let orchestrator = self.store.get_yard_orchestrator().await?;
        let worker = orchestrator
            .worker
            .as_ref()
            .filter(|worker| worker.id == current_worker.id)
            .ok_or(YardOrchestratorServiceError::RecoveryOwnershipChanged)?;
        let runtime = worker
            .runtime
            .as_ref()
            .filter(|runtime| {
                runtime.session == YARD_ORCHESTRATOR_SESSION
                    && runtime.terminal_id == restored.terminal_id
                    && runtime.observation_state == RuntimeObservationState::Observed
                    && runtime.process_state == RuntimeProcessState::Running
            })
            .ok_or(YardOrchestratorServiceError::RecoveryBindingMissing)?;

        if current_runtime.provider_session.is_some()
            && runtime.provider_session != current_runtime.provider_session
        {
            return Err(YardOrchestratorServiceError::RecoveryBindingAmbiguous);
        }

        Ok(RecoveredYardOrchestrator {
            command_id: command.command_id,
            orchestrator,
        })
    }

    async fn reconcile_provisioned_worker(
        &self,
        terminal_id: &str,
    ) -> Result<ObservedWorker, YardOrchestratorServiceError> {
        let deadline = Instant::now() + RUNTIME_IDENTITY_TIMEOUT;
        loop {
            let (inventory, _) = self
                .reconciliation
                .refresh(YARD_ORCHESTRATOR_SESSION)
                .await?;
            if let Some(worker) = inventory.workers.into_iter().find(|worker| {
                worker.terminal_id == terminal_id
                    && worker.interactive_ready
                    && worker.provider_session.is_some()
            }) {
                return Ok(worker);
            }
            if Instant::now() >= deadline {
                return Err(YardOrchestratorServiceError::RuntimeBindingUnverified);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

fn dedicated_worker(
    inventory: &RuntimeInventory,
    current: Option<&yard_domain::Worker>,
) -> Option<ObservedWorker> {
    let dedicated_workspaces = inventory
        .workspaces
        .iter()
        .filter(|workspace| workspace.label == YARD_ORCHESTRATOR_WORKSPACE_LABEL)
        .map(|workspace| workspace.runtime_id.as_str())
        .collect::<HashSet<_>>();
    let current_terminal = current
        .and_then(|worker| worker.runtime.as_ref())
        .filter(|runtime| runtime.session == YARD_ORCHESTRATOR_SESSION)
        .map(|runtime| runtime.terminal_id.as_str());
    let mut workers = inventory
        .workers
        .iter()
        .filter(|worker| {
            worker.name.as_deref() == Some(YARD_ORCHESTRATOR_AGENT_NAME)
                && dedicated_workspaces.contains(worker.workspace_id.as_str())
                && worker.interactive_ready
                && worker.provider_session.is_some()
        })
        .cloned()
        .collect::<Vec<_>>();
    workers.sort_by_key(|worker| {
        (
            current_terminal != Some(worker.terminal_id.as_str()),
            worker.workspace_id.clone(),
            worker.terminal_id.clone(),
        )
    });
    workers.into_iter().next()
}

fn runtime_error(error: RuntimeProvisionError) -> YardOrchestratorServiceError {
    match error {
        RuntimeProvisionError::PromptDelivery { message, .. } => {
            YardOrchestratorServiceError::ObjectiveDeliveryFailed(message)
        }
        RuntimeProvisionError::BeforeWorker(message)
        | RuntimeProvisionError::AfterPreparation { message, .. } => {
            YardOrchestratorServiceError::RuntimeProvision(message)
        }
    }
}

#[derive(Debug, Error)]
pub enum YardOrchestratorServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::YardOrchestratorValidationError),
    #[error(transparent)]
    Reconciliation(#[from] ReconciliationServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("unsupported worker profile: {0}")]
    UnsupportedProfile(String),
    #[error("Herdr Yard orchestrator provisioning failed: {0}")]
    RuntimeProvision(String),
    #[error("the provisioned Herdr worker did not appear in a fresh inventory")]
    RuntimeBindingUnverified,
    #[error("the reconciled Herdr worker is missing from Yard's durable worker inventory")]
    ReconciledWorkerMissing,
    #[error("the worker was created but the central coordination prompt failed: {0}")]
    ObjectiveDeliveryFailed(String),
    #[error("the Yard orchestrator has not been configured")]
    RecoveryNotConfigured,
    #[error("the configured Yard orchestrator does not use the dedicated Herdr session")]
    RecoveryNotDedicated,
    #[error("the configured Yard orchestrator did not reappear after the session restarted")]
    RecoveryBindingMissing,
    #[error("the restarted session did not restore the configured provider identity uniquely")]
    RecoveryBindingAmbiguous,
    #[error("Yard orchestrator ownership changed while the session was restarting")]
    RecoveryOwnershipChanged,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use tempfile::TempDir;
    use yard_domain::{
        CreateWorkerProfile, FocusObservation, ObservedStatus, ObservedWorker, ProviderSessionRef,
        ProvisionYardOrchestrator, RuntimeInventory, RuntimeSession, RuntimeSessions,
        WorkerProfileSpec, WorkerRuntimeBinding, WorkspaceObservation,
    };
    use yard_store::{SqliteProjectStore, YardStore};

    use crate::{
        allocation_service::{
            RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeSessionRequest,
            RuntimeWorkspaceProvisionRequest,
        },
        inventory_service::{InventoryServiceError, InventorySource},
        reconciliation_service::ReconciliationService,
    };

    use super::{
        YARD_ORCHESTRATOR_AGENT_NAME, YARD_ORCHESTRATOR_SESSION, YARD_ORCHESTRATOR_WORKSPACE_LABEL,
        YardOrchestratorService,
    };

    #[derive(Default)]
    struct DedicatedRuntime {
        live: AtomicBool,
        observed_at: AtomicU64,
        ensure_calls: AtomicUsize,
        bootstrap_calls: AtomicUsize,
        restore_on_ensure: AtomicBool,
        session_requests: Mutex<Vec<RuntimeSessionRequest>>,
        bootstrap_requests: Mutex<Vec<RuntimeWorkspaceProvisionRequest>>,
    }

    impl DedicatedRuntime {
        fn live() -> Self {
            Self {
                live: AtomicBool::new(true),
                ..Self::default()
            }
        }

        fn binding() -> WorkerRuntimeBinding {
            WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                workspace_id: "workspace-yard-orchestrator".to_owned(),
                terminal_id: "terminal-yard-orchestrator".to_owned(),
                tab_id: Some("tab-yard-orchestrator".to_owned()),
                pane_id: "pane-yard-orchestrator".to_owned(),
                provider_session: Some(provider_session()),
                owns_tab: true,
                observation_state: yard_domain::RuntimeObservationState::Observed,
                process_state: yard_domain::RuntimeProcessState::Running,
                status: ObservedStatus::Idle,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 1,
            }
        }
    }

    #[async_trait]
    impl InventorySource for DedicatedRuntime {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![RuntimeSession {
                    name: YARD_ORCHESTRATOR_SESSION.to_owned(),
                    is_default: false,
                    running: true,
                }],
            })
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            assert_eq!(session_name, YARD_ORCHESTRATOR_SESSION);
            let observed_at_unix_ms = self.observed_at.fetch_add(1, Ordering::SeqCst) + 1;
            let live = self.live.load(Ordering::SeqCst);
            Ok(RuntimeInventory {
                adapter: "herdr".to_owned(),
                session: YARD_ORCHESTRATOR_SESSION.to_owned(),
                runtime_version: "test".to_owned(),
                protocol: 19,
                observed_at_unix_ms,
                focus: FocusObservation::default(),
                workspaces: live
                    .then(|| WorkspaceObservation {
                        runtime_id: "workspace-yard-orchestrator".to_owned(),
                        order: 1,
                        label: YARD_ORCHESTRATOR_WORKSPACE_LABEL.to_owned(),
                        focused: false,
                        active_tab_id: "tab-yard-orchestrator".to_owned(),
                        pane_count: 1,
                        tab_count: 1,
                        status: ObservedStatus::Idle,
                        tokens: BTreeMap::new(),
                        worktree: None,
                    })
                    .into_iter()
                    .collect(),
                tabs: Vec::new(),
                panes: Vec::new(),
                workers: live
                    .then(|| ObservedWorker {
                        runtime_id: "terminal-yard-orchestrator".to_owned(),
                        terminal_id: "terminal-yard-orchestrator".to_owned(),
                        workspace_id: "workspace-yard-orchestrator".to_owned(),
                        tab_id: "tab-yard-orchestrator".to_owned(),
                        pane_id: "pane-yard-orchestrator".to_owned(),
                        name: Some(YARD_ORCHESTRATOR_AGENT_NAME.to_owned()),
                        provider: Some("codex".to_owned()),
                        display_provider: Some("Codex".to_owned()),
                        status: ObservedStatus::Idle,
                        focused: false,
                        launch_pending: false,
                        interactive_ready: true,
                        state_change_sequence: 1,
                        cwd: Some("/tmp/yard-backend".to_owned()),
                        foreground_cwd: Some("/tmp/yard-backend".to_owned()),
                        tokens: BTreeMap::new(),
                        provider_session: Some(provider_session()),
                        revision: 1,
                    })
                    .into_iter()
                    .collect(),
                child_agents: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl RuntimeControl for DedicatedRuntime {
        async fn ensure_session(
            &self,
            request: RuntimeSessionRequest,
        ) -> Result<(), RuntimeProvisionError> {
            self.ensure_calls.fetch_add(1, Ordering::SeqCst);
            self.session_requests.lock().unwrap().push(request);
            if self.restore_on_ensure.load(Ordering::SeqCst) {
                self.live.store(true, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn bootstrap_worker(
            &self,
            request: RuntimeWorkspaceProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            self.bootstrap_calls.fetch_add(1, Ordering::SeqCst);
            self.bootstrap_requests.lock().unwrap().push(request);
            self.live.store(true, Ordering::SeqCst);
            Ok(Self::binding())
        }

        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            unreachable!("the Yard singleton always bootstraps a new workspace")
        }
    }

    fn profile() -> CreateWorkerProfile {
        CreateWorkerProfile {
            spec: WorkerProfileSpec {
                name: "Central coordinator".to_owned(),
                runtime_adapter: "herdr".to_owned(),
                provider: "codex".to_owned(),
                model: Some("gpt-5.4".to_owned()),
                default_role: "orchestrator".to_owned(),
                instructions_ref: None,
                tools: Vec::new(),
                skills: Vec::new(),
                mcp_servers: Vec::new(),
                sandbox_policy: "runtime_default".to_owned(),
                worktree_policy: "project_workspace".to_owned(),
                permission_policy: "yolo".to_owned(),
                completion_contract: "manual_receipt".to_owned(),
            },
        }
    }

    fn provider_session() -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: "yard-orchestrator-session".to_owned(),
        }
    }

    async fn service(
        runtime: Arc<DedicatedRuntime>,
    ) -> (
        YardOrchestratorService,
        Arc<SqliteProjectStore>,
        TempDir,
        String,
    ) {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let profile = store.create_worker_profile(profile()).await.unwrap();
        let source: Arc<dyn InventorySource> = runtime.clone();
        let reconciliation = ReconciliationService::new(source, store.clone());
        let control: Arc<dyn RuntimeControl> = runtime;
        (
            YardOrchestratorService::new(
                control,
                store.clone(),
                reconciliation,
                "/tmp/yard-backend".to_owned(),
            ),
            store,
            temp,
            profile.id,
        )
    }

    fn command(profile_id: &str) -> ProvisionYardOrchestrator {
        ProvisionYardOrchestrator {
            command_id: "provision-yard-orchestrator".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile_id.to_owned(),
            expected_profile_version: 1,
            expected_orchestrator_version: 1,
        }
    }

    #[tokio::test]
    async fn concurrent_retries_bootstrap_once_and_replay_configuration() {
        let runtime = Arc::new(DedicatedRuntime::default());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let first_service = service.clone();
        let second_service = service;
        let first_command = command(&profile_id);
        let second_command = first_command.clone();

        let (first, second) = tokio::join!(
            first_service.provision(first_command),
            second_service.provision(second_command)
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.ensure_calls.load(Ordering::SeqCst), 2);
        assert_ne!(first.replayed, second.replayed);
        assert_eq!(
            first.orchestrator.worker.as_ref().unwrap().id,
            second.orchestrator.worker.as_ref().unwrap().id
        );
        assert_eq!(
            first.orchestrator.worker.as_ref().unwrap().profile_id,
            Some(profile_id)
        );
        assert_eq!(
            first.orchestrator.worker.as_ref().unwrap().profile_version,
            Some(1)
        );

        let sessions = runtime.session_requests.lock().unwrap();
        assert!(sessions.iter().all(|request| {
            request.session == YARD_ORCHESTRATOR_SESSION
                && request.startup_cwd == "/tmp/yard-backend"
        }));
        let bootstraps = runtime.bootstrap_requests.lock().unwrap();
        assert_eq!(bootstraps.len(), 1);
        let request = &bootstraps[0];
        assert_eq!(request.session, YARD_ORCHESTRATOR_SESSION);
        assert_eq!(request.workspace_label, YARD_ORCHESTRATOR_WORKSPACE_LABEL);
        assert_eq!(request.agent_name, YARD_ORCHESTRATOR_AGENT_NAME);
        assert_eq!(request.cwd, "/tmp/yard-backend");
        assert_eq!(request.args, ["--yolo", "-m", "gpt-5.4"]);
        assert!(request.prompt.contains("Coordinate all Yard projects"));
        assert!(request.prompt.contains("Role: central orchestrator"));
        assert!(
            request
                .prompt
                .contains("command \"provision-yard-orchestrator\"")
        );
        assert!(request.prompt.contains("There is no completed state"));
        assert!(
            yard_domain::OrchestratorStatusReport::scan_terminal_output(&request.prompt).is_none()
        );
    }

    #[tokio::test]
    async fn adopts_a_preexisting_dedicated_worker_without_bootstrapping() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;

        let configured = service.provision(command(&profile_id)).await.unwrap();

        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        let worker = configured.orchestrator.worker.unwrap();
        assert_eq!(
            worker.runtime.unwrap().terminal_id,
            "terminal-yard-orchestrator"
        );
    }

    #[tokio::test]
    async fn recovers_the_configured_worker_without_bootstrapping_or_reassigning() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, store, _temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        let worker_id = configured.orchestrator.worker.unwrap().id;

        runtime.live.store(false, Ordering::SeqCst);
        runtime.restore_on_ensure.store(true, Ordering::SeqCst);
        let recovered = service
            .recover(yard_domain::RecoverYardOrchestrator {
                command_id: "recover-yard-orchestrator".to_owned(),
                actor: "local-user".to_owned(),
                expected_orchestrator_version: configured.orchestrator.version,
            })
            .await
            .unwrap();

        assert_eq!(recovered.command_id, "recover-yard-orchestrator");
        assert_eq!(
            recovered.orchestrator.worker.as_ref().unwrap().id,
            worker_id
        );
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .get_yard_orchestrator()
                .await
                .unwrap()
                .worker
                .unwrap()
                .runtime
                .unwrap()
                .observation_state,
            yard_domain::RuntimeObservationState::Observed
        );
    }

    #[tokio::test]
    async fn recovery_fails_closed_when_the_configured_worker_is_not_restored() {
        let runtime = Arc::new(DedicatedRuntime::live());
        let (service, _store, _temp, profile_id) = service(runtime.clone()).await;
        let configured = service.provision(command(&profile_id)).await.unwrap();
        runtime.live.store(false, Ordering::SeqCst);

        let error = service
            .recover(yard_domain::RecoverYardOrchestrator {
                command_id: "recover-yard-orchestrator".to_owned(),
                actor: "local-user".to_owned(),
                expected_orchestrator_version: configured.orchestrator.version,
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            super::YardOrchestratorServiceError::RecoveryBindingMissing
        ));
        assert_eq!(runtime.bootstrap_calls.load(Ordering::SeqCst), 0);
    }
}

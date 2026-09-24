use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use yard_domain::{
    ObservedWorker, RuntimeInventory, RuntimeObservationState, RuntimeProcessState, RuntimeSession,
    RuntimeSessions, WorkerRuntimeBinding,
};
use yard_herdr::{
    AcquirePaneLeaseRequest, BootstrapAgentRequest, CloseLeasedPaneRequest, DiscoveredHerdrSession,
    HerdrAdapter, HerdrControlError, HerdrError, HerdrTerminal, HerdrTerminalError,
    OpenTerminalRequest as HerdrOpenTerminalRequest, PaneLease, PaneLeaseOperationResult,
    PaneLeaseStatus, PaneLeaseStatusRequest, PaneManagementCapability, PaneManagementRpcError,
    PrepareAgentRequest, PrepareWorkspaceAgentRequest, PromptAgentRequest, ProvisionAgentRequest,
    ReadPaneRequest, ReleasePaneLeaseRequest, RenewPaneLeaseRequest, StartPreparedAgentRequest,
    TerminalCommand as HerdrTerminalCommand, TerminalDimensions, TerminalEncoding, TerminalEvent,
    TerminalInput,
};

use crate::allocation_service::{
    RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest, RuntimeRetirementError,
    RuntimeRetirementRequest, RuntimeSessionRequest, RuntimeWorkerRestartRequest,
    RuntimeWorkspaceProvisionRequest,
};
use crate::intervention_service::{
    RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest, RuntimeOutputResult,
    RuntimePromptRequest, RuntimePromptResult,
};
use crate::provider_agents::ProviderAgentObserver;
use crate::terminal_service::{
    OpenTerminalRequest, RuntimeTerminal, RuntimeTerminalError, RuntimeTerminalSession,
    TerminalClientMessage, TerminalServerMessage,
};

#[derive(Debug, Error)]
pub enum InventoryServiceError {
    #[error(transparent)]
    Herdr(#[from] HerdrError),
}

#[derive(Debug, Clone)]
pub struct RuntimeSessionDescriptor {
    id: String,
    snapshot_key: String,
    summary: RuntimeSession,
    source: RuntimeSessionDescriptorSource,
}

#[derive(Debug, Clone)]
enum RuntimeSessionDescriptorSource {
    Named,
    Herdr(DiscoveredHerdrSession),
}

impl RuntimeSessionDescriptor {
    fn named(summary: RuntimeSession) -> Self {
        Self {
            id: summary.name.clone(),
            snapshot_key: summary.name.clone(),
            summary,
            source: RuntimeSessionDescriptorSource::Named,
        }
    }

    fn herdr(session: DiscoveredHerdrSession) -> Self {
        Self {
            id: session.id().to_owned(),
            snapshot_key: session.snapshot_key().to_string_lossy().into_owned(),
            summary: RuntimeSession {
                name: session.name().to_owned(),
                is_default: session.is_default(),
                running: session.running(),
            },
            source: RuntimeSessionDescriptorSource::Herdr(session),
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn snapshot_key(&self) -> &str {
        &self.snapshot_key
    }

    #[must_use]
    pub fn summary(&self) -> &RuntimeSession {
        &self.summary
    }

    #[cfg(test)]
    pub(crate) fn identified(id: &str, snapshot_key: &str, summary: RuntimeSession) -> Self {
        Self {
            id: id.to_owned(),
            snapshot_key: snapshot_key.to_owned(),
            summary,
            source: RuntimeSessionDescriptorSource::Named,
        }
    }
}

#[async_trait]
pub trait InventorySource: Send + Sync {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError>;
    async fn inventory(
        &self,
        session_name: &str,
    ) -> Result<RuntimeInventory, InventoryServiceError>;

    async fn session_descriptors(
        &self,
    ) -> Result<(String, Vec<RuntimeSessionDescriptor>), InventoryServiceError> {
        let sessions = self.sessions().await?;
        Ok((
            sessions.adapter,
            sessions
                .sessions
                .into_iter()
                .map(RuntimeSessionDescriptor::named)
                .collect(),
        ))
    }

    async fn inventory_for_descriptor(
        &self,
        session: &RuntimeSessionDescriptor,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        self.inventory(&session.summary.name).await
    }

    async fn fleet_inventory_for_descriptor(
        &self,
        session: &RuntimeSessionDescriptor,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        self.inventory_for_descriptor(session).await
    }

    async fn pane_management_capability(
        &self,
        _session_name: &str,
    ) -> Result<PaneManagementCapability, PaneManagementRpcError> {
        Ok(PaneManagementCapability::unsupported())
    }

    async fn acquire_pane_lease(
        &self,
        _request: AcquirePaneLeaseRequest,
    ) -> Result<PaneLease, PaneManagementRpcError> {
        Err(PaneManagementRpcError::Unsupported)
    }

    async fn renew_pane_lease(
        &self,
        _request: RenewPaneLeaseRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        Err(PaneManagementRpcError::Unsupported)
    }

    async fn release_pane_lease(
        &self,
        _request: ReleasePaneLeaseRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        Err(PaneManagementRpcError::Unsupported)
    }

    async fn pane_lease_status(
        &self,
        _request: PaneLeaseStatusRequest,
    ) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
        Err(PaneManagementRpcError::Unsupported)
    }

    async fn close_if_pane_leased(
        &self,
        _request: CloseLeasedPaneRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        Err(PaneManagementRpcError::Unsupported)
    }
}

pub(crate) fn runtime_binding_from_observed_worker(
    inventory: &RuntimeInventory,
    worker: &ObservedWorker,
    owns_tab: bool,
) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        adapter: inventory.adapter.clone(),
        session: inventory.session.clone(),
        workspace_id: worker.workspace_id.clone(),
        terminal_id: worker.terminal_id.clone(),
        tab_id: Some(worker.tab_id.clone()),
        pane_id: worker.pane_id.clone(),
        provider_session: worker.provider_session.clone(),
        owns_tab,
        observation_state: RuntimeObservationState::Observed,
        process_state: RuntimeProcessState::Running,
        status: worker.status,
        state_change_sequence: worker.state_change_sequence,
        revision: worker.revision,
        version: 1,
        last_observed_at_unix_ms: inventory.observed_at_unix_ms,
    }
}

#[cfg(test)]
pub(crate) fn seed_inventory_workers(
    database_path: &std::path::Path,
    inventory: &RuntimeInventory,
    terminal_ids: &[&str],
) {
    let mut connection = rusqlite::Connection::open(database_path).unwrap();
    let transaction = connection.transaction().unwrap();
    let observed_at = i64::try_from(inventory.observed_at_unix_ms).unwrap();
    let mut seeded = 0;
    for worker in inventory
        .workers
        .iter()
        .filter(|worker| terminal_ids.contains(&worker.terminal_id.as_str()))
    {
        let worker_id = uuid::Uuid::now_v7().to_string();
        let provider = worker.provider_session.as_ref();
        let state_change_sequence = i64::try_from(worker.state_change_sequence).unwrap();
        let revision = i64::try_from(worker.revision).unwrap();
        transaction
            .execute(
                "INSERT INTO workers (
                    id, profile_id, profile_version, desired_state, version,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, NULL, NULL, ?3, 1, ?2, ?2)",
                rusqlite::params![worker_id, observed_at, "running"],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO worker_runtime_bindings (
                    worker_id, adapter, runtime_session, runtime_workspace_id,
                    terminal_id, tab_id, pane_id, provider_session_source,
                    provider_session_provider, provider_session_kind,
                    provider_session_value, owns_tab, observation_state,
                    observed_status, process_state, state_change_sequence,
                    runtime_revision, version, last_observed_at_unix_ms,
                    updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                           0, ?12, ?13, ?14, ?15, ?16, 1, ?17, ?17)",
                rusqlite::params![
                    worker_id,
                    inventory.adapter,
                    inventory.session,
                    worker.workspace_id,
                    worker.terminal_id,
                    worker.tab_id,
                    worker.pane_id,
                    provider.map(|value| value.source.as_str()),
                    provider.map(|value| value.provider.as_str()),
                    provider.map(|value| value.kind.as_str()),
                    provider.map(|value| value.value.as_str()),
                    "observed",
                    serde_json::to_value(worker.status)
                        .unwrap()
                        .as_str()
                        .unwrap(),
                    "running",
                    state_change_sequence,
                    revision,
                    observed_at,
                ],
            )
            .unwrap();
        seeded += 1;
    }
    assert_eq!(seeded, terminal_ids.len());
    transaction.commit().unwrap();
}

#[derive(Debug)]
pub struct HerdrInventorySource {
    adapter: HerdrAdapter,
    provider_agents: Arc<ProviderAgentObserver>,
}

struct HerdrRuntimeTerminalSession {
    terminal: Option<HerdrTerminal>,
    first_message: Option<TerminalServerMessage>,
}

impl HerdrInventorySource {
    #[must_use]
    pub fn new(adapter: HerdrAdapter) -> Self {
        Self {
            adapter,
            provider_agents: Arc::new(ProviderAgentObserver::from_env()),
        }
    }

    async fn observe_provider_agents(&self, mut inventory: RuntimeInventory) -> RuntimeInventory {
        let workers = inventory.workers.clone();
        let observer = Arc::clone(&self.provider_agents);
        match tokio::task::spawn_blocking(move || observer.observe(&workers)).await {
            Ok(children) => inventory.child_agents = children,
            Err(error) => {
                tracing::warn!(%error, "provider child-agent observation task failed");
            }
        }
        inventory
    }
}

#[async_trait]
impl InventorySource for HerdrInventorySource {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
        self.adapter.sessions().await.map_err(Into::into)
    }

    async fn inventory(
        &self,
        session_name: &str,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        let inventory = self
            .adapter
            .inventory(session_name)
            .await
            .map_err(InventoryServiceError::from)?;
        Ok(self.observe_provider_agents(inventory).await)
    }

    async fn session_descriptors(
        &self,
    ) -> Result<(String, Vec<RuntimeSessionDescriptor>), InventoryServiceError> {
        let sessions = self.adapter.discover_sessions().await?;
        Ok((
            "herdr".to_owned(),
            sessions
                .into_iter()
                .map(RuntimeSessionDescriptor::herdr)
                .collect(),
        ))
    }

    async fn inventory_for_descriptor(
        &self,
        session: &RuntimeSessionDescriptor,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        let RuntimeSessionDescriptorSource::Herdr(session) = &session.source else {
            return self.inventory(&session.summary.name).await;
        };
        self.adapter
            .inventory_for_discovered_session(session)
            .await
            .map_err(Into::into)
    }

    async fn fleet_inventory_for_descriptor(
        &self,
        session: &RuntimeSessionDescriptor,
    ) -> Result<RuntimeInventory, InventoryServiceError> {
        let RuntimeSessionDescriptorSource::Herdr(session) = &session.source else {
            return self.inventory_for_descriptor(session).await;
        };
        self.adapter
            .fleet_inventory_for_discovered_session(session)
            .await
            .map_err(Into::into)
    }

    async fn pane_management_capability(
        &self,
        session_name: &str,
    ) -> Result<PaneManagementCapability, PaneManagementRpcError> {
        self.adapter.pane_management_capability(session_name).await
    }

    async fn acquire_pane_lease(
        &self,
        request: AcquirePaneLeaseRequest,
    ) -> Result<PaneLease, PaneManagementRpcError> {
        self.adapter.acquire_pane_lease(request).await
    }

    async fn renew_pane_lease(
        &self,
        request: RenewPaneLeaseRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        self.adapter.renew_pane_lease(request).await
    }

    async fn release_pane_lease(
        &self,
        request: ReleasePaneLeaseRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        self.adapter.release_pane_lease(request).await
    }

    async fn pane_lease_status(
        &self,
        request: PaneLeaseStatusRequest,
    ) -> Result<PaneLeaseStatus, PaneManagementRpcError> {
        self.adapter.pane_lease_status(request).await
    }

    async fn close_if_pane_leased(
        &self,
        request: CloseLeasedPaneRequest,
    ) -> Result<PaneLeaseOperationResult, PaneManagementRpcError> {
        self.adapter.close_if_pane_leased(request).await
    }
}

fn prompt_delivery_error(
    source: &HerdrError,
    rollback: &str,
    rollback_succeeded: bool,
    started_runtime: Option<Box<yard_domain::WorkerRuntimeBinding>>,
) -> RuntimeProvisionError {
    let message = format!(
        "Herdr created the worker but initial prompt delivery failed: {source}; runtime rollback: {rollback}"
    );
    let ambiguous = !rollback_succeeded || started_runtime.is_some();
    RuntimeProvisionError::AfterPreparation {
        message,
        ambiguous,
        started_runtime,
    }
}

fn start_failure_error(
    start: HerdrError,
    rollback: String,
    rollback_succeeded: bool,
    started_runtime: Option<Box<yard_domain::WorkerRuntimeBinding>>,
) -> RuntimeProvisionError {
    let ambiguous = started_runtime.is_some();
    let message = HerdrControlError::StartFailed {
        start,
        rollback,
        rollback_succeeded,
        started_runtime: started_runtime.clone(),
    }
    .to_string();
    RuntimeProvisionError::AfterPreparation {
        message,
        ambiguous,
        started_runtime,
    }
}

#[async_trait]
impl RuntimeControl for HerdrInventorySource {
    async fn ensure_session(
        &self,
        request: RuntimeSessionRequest,
    ) -> Result<(), RuntimeProvisionError> {
        self.adapter
            .ensure_session(&request.session, std::path::Path::new(&request.startup_cwd))
            .await
            .map_err(|error| RuntimeProvisionError::BeforeWorker(error.to_string()))
    }

    async fn restart_worker(
        &self,
        request: RuntimeWorkerRestartRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .start_existing_agent(StartPreparedAgentRequest {
                command_id: request.command_id,
                prepared: request.runtime,
                agent_name: request.agent_name,
                kind: request.kind,
                args: request.args,
                prompt: request.prompt,
            })
            .await
        {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed {
                source,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(prompt_delivery_error(
                &source,
                &rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(start_failure_error(
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(error) => Err(RuntimeProvisionError::BeforeWorker(error.to_string())),
        }
    }

    async fn bootstrap_worker(
        &self,
        request: RuntimeWorkspaceProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        let result = self
            .adapter
            .bootstrap_agent(BootstrapAgentRequest {
                command_id: request.command_id,
                session: request.session,
                workspace_label: request.workspace_label,
                cwd: request.cwd,
                agent_name: request.agent_name,
                kind: request.kind,
                args: request.args,
                prompt: request.prompt,
            })
            .await;
        match result {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed {
                source,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(prompt_delivery_error(
                &source,
                &rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::WorkspaceCreateFailed { source, ambiguous }) if ambiguous => {
                Err(RuntimeProvisionError::AfterPreparation {
                    message: source.to_string(),
                    ambiguous: true,
                    started_runtime: None,
                })
            }
            Err(
                HerdrControlError::WorkspaceCreateFailed { source, .. }
                | HerdrControlError::PrepareFailed { source, .. },
            ) => Err(RuntimeProvisionError::BeforeWorker(source.to_string())),
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(start_failure_error(
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::Runtime(error)) => {
                Err(RuntimeProvisionError::BeforeWorker(error.to_string()))
            }
        }
    }

    async fn prepare_workspace_worker(
        &self,
        request: RuntimeWorkspaceProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .prepare_workspace_agent(PrepareWorkspaceAgentRequest {
                command_id: request.command_id,
                session: request.session,
                workspace_label: request.workspace_label,
                cwd: request.cwd,
            })
            .await
        {
            Ok(prepared) => Ok(prepared.runtime),
            Err(HerdrControlError::WorkspaceCreateFailed { source, ambiguous }) => {
                if ambiguous {
                    Err(RuntimeProvisionError::AfterPreparation {
                        message: source.to_string(),
                        ambiguous,
                        started_runtime: None,
                    })
                } else {
                    Err(RuntimeProvisionError::BeforeWorker(source.to_string()))
                }
            }
            Err(error) => Err(RuntimeProvisionError::BeforeWorker(error.to_string())),
        }
    }

    async fn start_prepared_workspace_worker(
        &self,
        request: RuntimeWorkspaceProvisionRequest,
        prepared: yard_domain::WorkerRuntimeBinding,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .start_prepared_workspace_agent(StartPreparedAgentRequest {
                command_id: request.command_id,
                prepared,
                agent_name: request.agent_name,
                kind: request.kind,
                args: request.args,
                prompt: request.prompt,
            })
            .await
        {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed {
                source,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(prompt_delivery_error(
                &source,
                &rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(start_failure_error(
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(error) => Err(RuntimeProvisionError::AfterPreparation {
                message: error.to_string(),
                ambiguous: true,
                started_runtime: None,
            }),
        }
    }

    async fn provision_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        let request = ProvisionAgentRequest {
            command_id: request.command_id,
            session: request.session,
            workspace_id: request.workspace_id,
            cwd: request.cwd,
            tab_label: request.tab_label,
            agent_name: request.agent_name,
            kind: request.kind,
            args: request.args,
            prompt: request.prompt,
        };
        match self.adapter.provision_agent(request).await {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed {
                source,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(prompt_delivery_error(
                &source,
                &rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::PrepareFailed { source, ambiguous }) => {
                if ambiguous {
                    Err(RuntimeProvisionError::AfterPreparation {
                        message: source.to_string(),
                        ambiguous,
                        started_runtime: None,
                    })
                } else {
                    Err(RuntimeProvisionError::BeforeWorker(source.to_string()))
                }
            }
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(start_failure_error(
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::WorkspaceCreateFailed { source, ambiguous }) => {
                Err(RuntimeProvisionError::AfterPreparation {
                    message: source.to_string(),
                    ambiguous,
                    started_runtime: None,
                })
            }
            Err(HerdrControlError::Runtime(error)) => {
                Err(RuntimeProvisionError::BeforeWorker(error.to_string()))
            }
        }
    }

    async fn prepare_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .prepare_agent(PrepareAgentRequest {
                command_id: request.command_id,
                session: request.session,
                workspace_id: request.workspace_id,
                cwd: request.cwd,
                tab_label: request.tab_label,
            })
            .await
        {
            Ok(prepared) => Ok(prepared.runtime),
            Err(HerdrControlError::PrepareFailed { source, ambiguous }) => {
                if ambiguous {
                    Err(RuntimeProvisionError::AfterPreparation {
                        message: source.to_string(),
                        ambiguous,
                        started_runtime: None,
                    })
                } else {
                    Err(RuntimeProvisionError::BeforeWorker(source.to_string()))
                }
            }
            Err(error) => Err(RuntimeProvisionError::BeforeWorker(error.to_string())),
        }
    }

    async fn start_prepared_worker(
        &self,
        request: RuntimeProvisionRequest,
        prepared: yard_domain::WorkerRuntimeBinding,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .start_prepared_agent(StartPreparedAgentRequest {
                command_id: request.command_id,
                prepared,
                agent_name: request.agent_name,
                kind: request.kind,
                args: request.args,
                prompt: request.prompt,
            })
            .await
        {
            Ok(agent) => Ok(agent.runtime),
            Err(HerdrControlError::PromptDeliveryFailed {
                source,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(prompt_delivery_error(
                &source,
                &rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(HerdrControlError::StartFailed {
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            }) => Err(start_failure_error(
                start,
                rollback,
                rollback_succeeded,
                started_runtime,
            )),
            Err(error) => Err(RuntimeProvisionError::AfterPreparation {
                message: error.to_string(),
                ambiguous: true,
                started_runtime: None,
            }),
        }
    }

    async fn prepare_replacement_worker(
        &self,
        request: RuntimeProvisionRequest,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        match self
            .adapter
            .prepare_agent(PrepareAgentRequest {
                command_id: request.command_id,
                session: request.session,
                workspace_id: request.workspace_id,
                cwd: request.cwd,
                tab_label: request.tab_label,
            })
            .await
        {
            Ok(prepared) => Ok(prepared.runtime),
            Err(HerdrControlError::PrepareFailed {
                source,
                ambiguous: true,
            }) => Err(RuntimeProvisionError::AfterPreparation {
                message: source.to_string(),
                ambiguous: true,
                started_runtime: None,
            }),
            Err(HerdrControlError::PrepareFailed { source, .. }) => {
                Err(RuntimeProvisionError::BeforeWorker(source.to_string()))
            }
            Err(error) => Err(RuntimeProvisionError::BeforeWorker(error.to_string())),
        }
    }

    async fn start_prepared_replacement_worker(
        &self,
        request: RuntimeProvisionRequest,
        prepared: yard_domain::WorkerRuntimeBinding,
    ) -> Result<yard_domain::WorkerRuntimeBinding, RuntimeProvisionError> {
        self.start_prepared_worker(request, prepared).await
    }

    async fn retire_runtime(
        &self,
        request: RuntimeRetirementRequest,
    ) -> Result<(), RuntimeRetirementError> {
        if request.adapter != "herdr" {
            return Err(RuntimeRetirementError::UnsupportedAdapter(request.adapter));
        }
        let inventory = self
            .adapter
            .inventory(&request.session)
            .await
            .map_err(|error| RuntimeRetirementError::Runtime(error.to_string()))?;
        retirement_outcome(&inventory, &request)
    }
}

fn retirement_outcome(
    inventory: &RuntimeInventory,
    request: &RuntimeRetirementRequest,
) -> Result<(), RuntimeRetirementError> {
    match retirement_resolution(inventory, request) {
        // The installed Herdr protocol accepts only a target ID, so any
        // separate identity precheck still races.
        RetirementResolution::Target => Err(RuntimeRetirementError::AtomicIdentityGuardUnavailable),
        RetirementResolution::Absent => Ok(()),
        RetirementResolution::Conflict => Err(RuntimeRetirementError::IdentityNotObserved),
    }
}

struct RetirementIdentity {
    workspace: String,
}

enum RetirementResolution {
    Target,
    Absent,
    Conflict,
}

fn retirement_resolution(
    inventory: &RuntimeInventory,
    request: &RuntimeRetirementRequest,
) -> RetirementResolution {
    let identity = if let Some(expected) = request.provider_session.as_ref() {
        retirement_by_provider(inventory, &request.terminal_id, expected)
    } else {
        retirement_by_topology(inventory, request)
    };
    let identity = match identity {
        Ok(Some(identity)) => identity,
        Ok(None) => return RetirementResolution::Absent,
        Err(()) => return RetirementResolution::Conflict,
    };
    if identity.workspace != request.workspace_id {
        return RetirementResolution::Conflict;
    }
    RetirementResolution::Target
}

fn retirement_by_provider(
    inventory: &RuntimeInventory,
    captured_terminal_id: &str,
    expected: &yard_domain::ProviderSessionRef,
) -> Result<Option<RetirementIdentity>, ()> {
    let matching_workers = inventory
        .workers
        .iter()
        .filter(|worker| worker.provider_session.as_ref() == Some(expected))
        .collect::<Vec<_>>();
    if matching_workers.len() > 1 {
        return Err(());
    }
    if let Some(worker) = matching_workers.first() {
        if inventory.panes.iter().any(|pane| {
            pane.provider_session.as_ref() == Some(expected)
                && pane.terminal_id != worker.terminal_id
        }) {
            return Err(());
        }
        return Ok(Some(RetirementIdentity {
            workspace: worker.workspace_id.clone(),
        }));
    }

    let matching_panes = inventory
        .panes
        .iter()
        .filter(|pane| pane.provider_session.as_ref() == Some(expected))
        .collect::<Vec<_>>();
    if matching_panes.len() > 1 {
        return Err(());
    }
    if let Some(pane) = matching_panes.first() {
        return Ok(Some(RetirementIdentity {
            workspace: pane.workspace_id.clone(),
        }));
    }
    let captured_terminal_reused = inventory
        .workers
        .iter()
        .any(|worker| worker.terminal_id == captured_terminal_id)
        || inventory
            .panes
            .iter()
            .any(|pane| pane.terminal_id == captured_terminal_id);
    if captured_terminal_reused {
        Err(())
    } else {
        Ok(None)
    }
}

fn retirement_by_topology(
    inventory: &RuntimeInventory,
    request: &RuntimeRetirementRequest,
) -> Result<Option<RetirementIdentity>, ()> {
    if let Some(worker) = inventory
        .workers
        .iter()
        .find(|worker| worker.terminal_id == request.terminal_id)
    {
        if worker.provider_session.is_some()
            || request.tab_id.as_deref() != Some(worker.tab_id.as_str())
            || request.pane_id != worker.pane_id
        {
            return Err(());
        }
        return Ok(Some(RetirementIdentity {
            workspace: worker.workspace_id.clone(),
        }));
    }
    let pane = inventory
        .panes
        .iter()
        .find(|pane| pane.terminal_id == request.terminal_id)
        .or_else(|| {
            inventory.panes.iter().find(|pane| {
                pane.runtime_id == request.pane_id
                    && pane.workspace_id == request.workspace_id
                    && request.tab_id.as_deref() == Some(pane.tab_id.as_str())
                    && pane.provider_session.is_none()
                    && !inventory
                        .workers
                        .iter()
                        .any(|worker| worker.pane_id == pane.runtime_id)
            })
        });
    let Some(pane) = pane else {
        return Ok(None);
    };
    if pane.provider_session.is_some()
        || request.tab_id.as_deref() != Some(pane.tab_id.as_str())
        || request.pane_id != pane.runtime_id
    {
        return Err(());
    }
    Ok(Some(RetirementIdentity {
        workspace: pane.workspace_id.clone(),
    }))
}

#[async_trait]
impl RuntimeIntervention for HerdrInventorySource {
    async fn prompt(
        &self,
        request: RuntimePromptRequest,
    ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
        self.adapter
            .prompt_agent(PromptAgentRequest {
                command_id: request.command_id,
                session: request.session,
                pane_id: request.pane_id,
                text: request.text,
            })
            .await
            .map(|result| RuntimePromptResult {
                status: result.status,
            })
            .map_err(|error| classify_prompt_error(&error))
    }

    async fn read_output(
        &self,
        request: RuntimeOutputRequest,
    ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
        self.adapter
            .read_pane(ReadPaneRequest {
                request_id: request.request_id,
                session: request.session,
                pane_id: request.pane_id,
                lines: request.lines,
            })
            .await
            .map(|output| RuntimeOutputResult {
                pane_id: output.pane_id,
                workspace_id: output.workspace_id,
                tab_id: output.tab_id,
                source: output.source,
                format: output.format,
                text: output.text,
                revision: output.revision,
                truncated: output.truncated,
            })
            .map_err(|error| RuntimeInterventionError::Unavailable(error.to_string()))
    }
}

#[async_trait]
impl RuntimeTerminal for HerdrInventorySource {
    async fn open_terminal(
        &self,
        request: OpenTerminalRequest,
    ) -> Result<Box<dyn RuntimeTerminalSession>, RuntimeTerminalError> {
        let dimensions = TerminalDimensions::new(request.cols, request.rows)
            .map_err(|error| classify_terminal_error(&error))?;
        let request =
            HerdrOpenTerminalRequest::new(request.session, request.terminal_id, dimensions)
                .map_err(|error| classify_terminal_error(&error))?;
        let mut terminal = self
            .adapter
            .open_terminal(request)
            .map_err(|error| classify_terminal_error(&error))?;
        let first_event = terminal
            .next_event()
            .await
            .map_err(|error| classify_terminal_error(&error))?;
        let Some(first_event) = first_event else {
            return Err(RuntimeTerminalError::Unavailable(
                "Herdr terminal control closed before its initial frame".to_owned(),
            ));
        };
        if let TerminalEvent::Closed(closed) = &first_event {
            let reason = closed.reason.clone();
            let _ = terminal.close().await;
            return if is_ownership_failure(&reason) {
                Err(RuntimeTerminalError::Ownership(reason))
            } else {
                Err(RuntimeTerminalError::Unavailable(reason))
            };
        }
        let first_message = terminal_server_message(first_event);
        if !matches!(
            first_message,
            TerminalServerMessage::Frame { full: true, .. }
        ) {
            let _ = terminal.close().await;
            return Err(RuntimeTerminalError::Protocol(
                "Herdr terminal stream did not begin with a full frame".to_owned(),
            ));
        }
        Ok(Box::new(HerdrRuntimeTerminalSession {
            terminal: Some(terminal),
            first_message: Some(first_message),
        }))
    }
}

#[async_trait]
impl RuntimeTerminalSession for HerdrRuntimeTerminalSession {
    async fn next_message(
        &mut self,
    ) -> Result<Option<TerminalServerMessage>, RuntimeTerminalError> {
        if let Some(message) = self.first_message.take() {
            return Ok(Some(message));
        }
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(None);
        };
        match terminal.next_event().await {
            Ok(Some(event)) => Ok(Some(terminal_server_message(event))),
            Ok(None) => {
                self.terminal.take();
                Ok(None)
            }
            Err(error) => {
                self.terminal.take();
                Err(classify_terminal_error(&error))
            }
        }
    }

    async fn send(&mut self, command: TerminalClientMessage) -> Result<(), RuntimeTerminalError> {
        let terminal = self.terminal.as_mut().ok_or_else(|| {
            RuntimeTerminalError::Unavailable("Herdr terminal control is closed".to_owned())
        })?;
        let command = match command {
            TerminalClientMessage::Input {
                text: Some(text),
                bytes: None,
            } => HerdrTerminalCommand::Input(TerminalInput::Text(text)),
            TerminalClientMessage::Input {
                text: None,
                bytes: Some(bytes),
            } => HerdrTerminalCommand::Input(TerminalInput::BytesBase64(bytes)),
            TerminalClientMessage::Resize { cols, rows } => HerdrTerminalCommand::Resize(
                TerminalDimensions::new(cols, rows)
                    .map_err(|error| classify_terminal_error(&error))?,
            ),
            TerminalClientMessage::Release => HerdrTerminalCommand::Release,
            TerminalClientMessage::Input { .. } => {
                return Err(RuntimeTerminalError::Protocol(
                    "terminal.input requires exactly one text or bytes field".to_owned(),
                ));
            }
        };
        terminal
            .send(command)
            .await
            .map_err(|error| classify_terminal_error(&error))
    }

    async fn release(&mut self) -> Result<(), RuntimeTerminalError> {
        let Some(terminal) = self.terminal.take() else {
            return Ok(());
        };
        terminal
            .close()
            .await
            .map_err(|error| classify_terminal_error(&error))
    }
}

fn terminal_server_message(event: TerminalEvent) -> TerminalServerMessage {
    match event {
        TerminalEvent::Frame(frame) => TerminalServerMessage::Frame {
            bytes: frame.bytes,
            encoding: match frame.encoding {
                TerminalEncoding::Ansi => "ansi".to_owned(),
            },
            seq: frame.seq,
            width: frame.width,
            height: frame.height,
            full: frame.full,
        },
        TerminalEvent::Closed(closed) => TerminalServerMessage::Closed {
            reason: closed.reason,
        },
    }
}

fn classify_terminal_error(error: &HerdrTerminalError) -> RuntimeTerminalError {
    let message = error.to_string();
    if let HerdrTerminalError::UnexpectedExit { stderr, .. } = error
        && is_ownership_failure(stderr)
    {
        return RuntimeTerminalError::Ownership(message);
    }
    match error {
        HerdrTerminalError::EventLineTooLarge
        | HerdrTerminalError::UnterminatedEvent
        | HerdrTerminalError::MalformedEvent(_)
        | HerdrTerminalError::InvalidEvent(_)
        | HerdrTerminalError::InvalidBase64Input
        | HerdrTerminalError::CommandEncode(_)
        | HerdrTerminalError::CommandLineTooLarge => RuntimeTerminalError::Protocol(message),
        _ => RuntimeTerminalError::Unavailable(message),
    }
}

fn is_ownership_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("ownership")
        || message.contains("already controlled")
        || message.contains("controller already")
        || message.contains("control conflict")
}

fn classify_prompt_error(error: &HerdrError) -> RuntimeInterventionError {
    match error {
        HerdrError::Api { code, .. }
            if matches!(
                code.as_str(),
                "agent_not_found"
                    | "agent_not_ready"
                    | "empty_agent_prompt"
                    | "agent_prompt_failed"
            ) =>
        {
            RuntimeInterventionError::Rejected(error.to_string())
        }
        HerdrError::DiscoveryIo(_)
        | HerdrError::DiscoveryTimeout
        | HerdrError::DiscoveryResponseTooLarge { .. }
        | HerdrError::DiscoveryFailed { .. }
        | HerdrError::DiscoveryDecode(_)
        | HerdrError::SessionNotFound(_)
        | HerdrError::SessionNotRunning(_)
        | HerdrError::SocketConnect { .. } => {
            RuntimeInterventionError::Unavailable(error.to_string())
        }
        _ => RuntimeInterventionError::Ambiguous(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yard_domain::{
        FocusObservation, ObservedStatus, ObservedWorker, PaneObservation, ProviderSessionRef,
        RuntimeInventory, TabObservation,
    };

    use super::{
        RetirementResolution, retirement_outcome, retirement_resolution, start_failure_error,
    };
    use crate::allocation_service::{
        RuntimeProvisionError, RuntimeRetirementError, RuntimeRetirementRequest,
    };

    fn provider(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn inventory(provider_session: ProviderSessionRef, pane_count: usize) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "0.8.0".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: vec![TabObservation {
                runtime_id: "tab-current".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                order: 1,
                label: "Worker".to_owned(),
                focused: false,
                pane_count,
                status: ObservedStatus::Idle,
            }],
            panes: Vec::new(),
            workers: vec![ObservedWorker {
                runtime_id: "terminal-1".to_owned(),
                pane_instance_id: None,
                terminal_id: "terminal-1".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                tab_id: "tab-current".to_owned(),
                pane_id: "pane-current".to_owned(),
                name: Some("worker".to_owned()),
                provider: Some("codex".to_owned()),
                display_provider: Some("Codex".to_owned()),
                status: ObservedStatus::Idle,
                focused: false,
                launch_pending: false,
                interactive_ready: true,
                state_change_sequence: 1,
                cwd: Some("/tmp/project".to_owned()),
                foreground_cwd: Some("/tmp/project".to_owned()),
                tokens: BTreeMap::new(),
                provider_session: Some(provider_session),
                revision: 1,
            }],
            child_agents: Vec::new(),
        }
    }

    fn request(provider_session: ProviderSessionRef) -> RuntimeRetirementRequest {
        RuntimeRetirementRequest {
            cleanup_id: "cleanup-1".to_owned(),
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            tab_id: Some("tab-current".to_owned()),
            pane_id: "pane-stale".to_owned(),
            provider_session: Some(provider_session),
            owns_tab: true,
        }
    }

    #[test]
    fn definite_start_rejection_with_failed_cleanup_remains_non_ambiguous() {
        let RuntimeProvisionError::AfterPreparation {
            message,
            ambiguous,
            started_runtime,
        } = start_failure_error(
            yard_herdr::HerdrError::Api {
                code: "agent_name_taken".to_owned(),
                message: "agent name is already in use".to_owned(),
            },
            "Herdr API tab_close_failed: tab is still busy".to_owned(),
            false,
            None,
        )
        else {
            panic!("expected an after-preparation start failure");
        };

        assert!(!ambiguous);
        assert!(started_runtime.is_none());
        assert!(message.contains("tab_close_failed"));
    }

    #[test]
    fn retirement_follows_the_matching_runtime_identity() {
        let RetirementResolution::Target = retirement_resolution(
            &inventory(provider("session-1"), 1),
            &request(provider("session-1")),
        ) else {
            panic!("expected matching retirement target");
        };
    }

    #[test]
    fn retirement_follows_a_provider_session_that_moved_terminals() {
        let mut inventory = inventory(provider("session-1"), 1);
        inventory.workers[0].runtime_id = "terminal-moved".to_owned();
        inventory.workers[0].terminal_id = "terminal-moved".to_owned();
        inventory.workers[0].tab_id = "tab-moved".to_owned();
        inventory.workers[0].pane_id = "pane-moved".to_owned();
        inventory.tabs[0].runtime_id = "tab-moved".to_owned();
        let mut replacement = inventory.workers[0].clone();
        replacement.runtime_id = "terminal-1".to_owned();
        replacement.terminal_id = "terminal-1".to_owned();
        replacement.tab_id = "tab-reused".to_owned();
        replacement.pane_id = "pane-reused".to_owned();
        replacement.provider_session = Some(provider("replacement-session"));
        inventory.workers.push(replacement);

        let RetirementResolution::Target =
            retirement_resolution(&inventory, &request(provider("session-1")))
        else {
            panic!("expected moved retirement target");
        };
    }

    #[test]
    fn retirement_rejects_a_provider_session_that_moved_workspaces() {
        let mut inventory = inventory(provider("session-1"), 1);
        inventory.workers[0].terminal_id = "terminal-moved".to_owned();
        inventory.workers[0].workspace_id = "workspace-other".to_owned();

        assert!(matches!(
            retirement_resolution(&inventory, &request(provider("session-1"))),
            RetirementResolution::Conflict
        ));
    }

    #[test]
    fn retirement_converges_without_closing_a_reused_terminal_identity() {
        let resolution = retirement_resolution(
            &inventory(provider("replacement-session"), 1),
            &request(provider("original-session")),
        );

        assert!(matches!(resolution, RetirementResolution::Conflict));
    }

    #[test]
    fn retirement_fails_closed_when_identity_changes_after_inventory() {
        let request = request(provider("original-session"));
        let captured_inventory = inventory(provider("original-session"), 1);
        assert!(matches!(
            retirement_resolution(&captured_inventory, &request),
            RetirementResolution::Target
        ));

        let live_after_inventory = inventory(provider("replacement-session"), 1);
        let stale_result = retirement_outcome(&captured_inventory, &request);

        assert!(matches!(
            stale_result,
            Err(RuntimeRetirementError::AtomicIdentityGuardUnavailable)
        ));
        assert_eq!(
            live_after_inventory.workers[0]
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("replacement-session")
        );
        assert!(matches!(
            retirement_outcome(&live_after_inventory, &request),
            Err(RuntimeRetirementError::IdentityNotObserved)
        ));
    }

    #[test]
    fn retirement_requires_exact_topology_without_provider_identity() {
        let mut inventory = inventory(provider("session-1"), 1);
        inventory.workers[0].provider_session = None;
        let mut request = request(provider("session-1"));
        request.provider_session = None;

        assert!(matches!(
            retirement_resolution(&inventory, &request),
            RetirementResolution::Conflict
        ));

        request.tab_id = Some("tab-current".to_owned());
        request.pane_id = "pane-current".to_owned();
        assert!(matches!(
            retirement_resolution(&inventory, &request),
            RetirementResolution::Target
        ));
    }

    #[test]
    fn retirement_follows_a_restored_shell_by_stable_topology() {
        let mut inventory = inventory(provider("session-1"), 1);
        inventory.workers.clear();
        inventory.panes.push(PaneObservation {
            runtime_id: "pane-current".to_owned(),
            pane_instance_id: None,
            terminal_id: "terminal-restored".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            tab_id: "tab-current".to_owned(),
            focused: false,
            cwd: Some("/tmp/project".to_owned()),
            foreground_cwd: Some("/tmp/project".to_owned()),
            label: None,
            provider: None,
            display_provider: None,
            status: ObservedStatus::Unknown,
            tokens: BTreeMap::new(),
            provider_session: None,
            revision: 0,
        });
        let mut request = request(provider("session-1"));
        request.terminal_id = "terminal-before-restart".to_owned();
        request.tab_id = Some("tab-current".to_owned());
        request.pane_id = "pane-current".to_owned();
        request.provider_session = None;

        assert!(matches!(
            retirement_resolution(&inventory, &request),
            RetirementResolution::Target
        ));
    }

    #[test]
    fn retirement_does_not_upgrade_an_unknown_captured_identity() {
        let mut request = request(provider("session-1"));
        request.provider_session = None;
        request.tab_id = Some("tab-current".to_owned());
        request.pane_id = "pane-current".to_owned();

        assert!(matches!(
            retirement_resolution(&inventory(provider("replacement-session"), 1), &request),
            RetirementResolution::Conflict
        ));
    }

    #[test]
    fn retirement_treats_a_fully_absent_captured_terminal_as_converged() {
        let mut inventory = inventory(provider("session-1"), 1);
        inventory.workers.clear();
        let request = request(provider("session-1"));

        assert!(matches!(
            retirement_resolution(&inventory, &request),
            RetirementResolution::Absent
        ));
        assert!(retirement_outcome(&inventory, &request).is_ok());
    }
}

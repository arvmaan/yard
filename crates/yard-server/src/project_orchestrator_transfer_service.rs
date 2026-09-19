use std::sync::Arc;

use thiserror::Error;
use yard_domain::{
    ObservedWorker, ProjectRuntimeBinding, RuntimeInventory, RuntimeObservationState,
    RuntimeProcessState, TransferProjectOrchestrator, TransferredProjectOrchestrator,
    WorkerAvailability, WorkerRuntimeBinding,
};
use yard_store::{ProjectStoreError, YardStore};

use crate::inventory_service::{InventoryServiceError, InventorySource};

#[derive(Clone)]
pub struct ProjectOrchestratorTransferService {
    source: Arc<dyn InventorySource>,
    store: Arc<dyn YardStore>,
}

impl ProjectOrchestratorTransferService {
    #[must_use]
    pub fn new(source: Arc<dyn InventorySource>, store: Arc<dyn YardStore>) -> Self {
        Self { source, store }
    }

    /// Atomically transfer a project's required orchestrator role to a live worker.
    ///
    /// Both durable runtime snapshots are checked before one fresh inventory
    /// verifies unique topology and provider identity. The store then repeats
    /// every optimistic check in the ownership-changing transaction.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectOrchestratorTransferServiceError`] for invalid,
    /// unavailable, stale, cross-workspace, or ambiguous identities.
    pub async fn transfer(
        &self,
        project_id: &str,
        command: TransferProjectOrchestrator,
    ) -> Result<TransferredProjectOrchestrator, ProjectOrchestratorTransferServiceError> {
        let command = command.normalize()?;
        if let Some(replayed) = self
            .store
            .replay_project_orchestrator_transfer(project_id, command.clone())
            .await?
        {
            return Ok(replayed);
        }

        let project = self.store.get_project(project_id).await?;
        if project.version != command.expected_project_version {
            return Err(ProjectStoreError::ProjectVersionConflict {
                current_version: project.version,
            }
            .into());
        }
        if project.orchestrator.id != command.expected_orchestrator_worker_id {
            return Err(ProjectStoreError::OrchestratorNotCurrent {
                current_worker_id: project.orchestrator.id,
            }
            .into());
        }
        if project.orchestrator.version != command.expected_orchestrator_worker_version {
            return Err(ProjectStoreError::WorkerVersionConflict {
                current_version: project.orchestrator.version,
            }
            .into());
        }
        if project.orchestrator.runtime.as_ref() != Some(&command.expected_orchestrator_runtime) {
            return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityChanged);
        }
        let candidate = self
            .store
            .list_worker_candidates()
            .await?
            .workers
            .into_iter()
            .find(|candidate| candidate.worker.id == command.worker_id)
            .ok_or(ProjectStoreError::WorkerNotFound)?;
        if candidate.worker.version != command.expected_worker_version {
            return Err(ProjectStoreError::WorkerVersionConflict {
                current_version: candidate.worker.version,
            }
            .into());
        }
        if candidate.availability != WorkerAvailability::UnassignedLive {
            return Err(ProjectStoreError::WorkerNotAvailable {
                availability: candidate.availability,
            }
            .into());
        }
        if candidate.worker.runtime.as_ref() != Some(&command.expected_worker_runtime) {
            return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityChanged);
        }
        validate_project_runtime(&project.runtime, &command.expected_orchestrator_runtime)?;
        validate_project_runtime(&project.runtime, &command.expected_worker_runtime)?;

        let inventory = self.source.inventory(&project.runtime.session).await?;
        validate_inventory(&inventory, &project.runtime)?;
        let displaced = unique_observed_worker(&inventory, &command.expected_orchestrator_runtime)?;
        let promoted = unique_observed_worker(&inventory, &command.expected_worker_runtime)?;
        if displaced.runtime_id == promoted.runtime_id {
            return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityAmbiguous);
        }

        self.store
            .transfer_project_orchestrator(project_id, command)
            .await
            .map_err(Into::into)
    }
}

fn validate_project_runtime(
    project: &ProjectRuntimeBinding,
    worker: &WorkerRuntimeBinding,
) -> Result<(), ProjectOrchestratorTransferServiceError> {
    if worker.adapter != project.adapter
        || worker.session != project.session
        || worker.workspace_id != project.workspace_id
    {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeWorkspaceMismatch);
    }
    if worker.observation_state != RuntimeObservationState::Observed
        || worker.process_state != RuntimeProcessState::Running
    {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeUnavailable);
    }
    Ok(())
}

fn validate_inventory(
    inventory: &RuntimeInventory,
    project: &ProjectRuntimeBinding,
) -> Result<(), ProjectOrchestratorTransferServiceError> {
    if inventory.adapter != project.adapter || inventory.session != project.session {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityChanged);
    }
    match inventory
        .workspaces
        .iter()
        .filter(|workspace| workspace.runtime_id == project.workspace_id)
        .count()
    {
        1 => Ok(()),
        0 => Err(ProjectOrchestratorTransferServiceError::RuntimeWorkspaceMissing),
        _ => Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityAmbiguous),
    }
}

fn unique_observed_worker<'a>(
    inventory: &'a RuntimeInventory,
    expected: &WorkerRuntimeBinding,
) -> Result<&'a ObservedWorker, ProjectOrchestratorTransferServiceError> {
    let mut terminal_matches = inventory
        .workers
        .iter()
        .filter(|worker| worker.terminal_id == expected.terminal_id);
    let observed = terminal_matches
        .next()
        .ok_or(ProjectOrchestratorTransferServiceError::RuntimeUnavailable)?;
    if terminal_matches.next().is_some() {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityAmbiguous);
    }
    if observed.workspace_id != expected.workspace_id
        || observed.pane_id != expected.pane_id
        || Some(observed.tab_id.as_str()) != expected.tab_id.as_deref()
        || observed.provider_session != expected.provider_session
    {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityChanged);
    }
    if !observed.interactive_ready || observed.launch_pending {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeUnavailable);
    }
    if let Some(provider) = expected.provider_session.as_ref()
        && inventory
            .workers
            .iter()
            .filter(|worker| worker.provider_session.as_ref() == Some(provider))
            .count()
            != 1
    {
        return Err(ProjectOrchestratorTransferServiceError::RuntimeIdentityAmbiguous);
    }
    Ok(observed)
}

#[derive(Debug, Error)]
pub enum ProjectOrchestratorTransferServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::OrchestratorTransferValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(
        "the worker runtime does not belong to the exact project adapter, session, and workspace"
    )]
    RuntimeWorkspaceMismatch,
    #[error("the project runtime workspace is not present in the fresh inventory")]
    RuntimeWorkspaceMissing,
    #[error("a required worker runtime is not freshly observed as live and interactive")]
    RuntimeUnavailable,
    #[error("a required worker runtime topology or provider identity changed")]
    RuntimeIdentityChanged,
    #[error("a required worker runtime identity is ambiguous in the fresh inventory")]
    RuntimeIdentityAmbiguous,
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use async_trait::async_trait;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, CreateProject, FocusObservation, ObservedStatus, ObservedWorker,
        ProjectRuntimeBinding, ProviderSessionRef, RuntimeInventory, RuntimeSessions,
        TransferProjectOrchestrator, WorkerRuntimeBinding, WorkspaceObservation,
    };
    use yard_store::{SqliteProjectStore, YardStore};

    use crate::inventory_service::{
        InventoryServiceError, InventorySource, seed_inventory_workers,
    };

    use super::{ProjectOrchestratorTransferService, ProjectOrchestratorTransferServiceError};

    #[derive(Clone)]
    struct StaticInventory {
        inventory: RuntimeInventory,
    }

    #[async_trait]
    impl InventorySource for StaticInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            unreachable!("transfer validation reads one known session")
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            Ok(self.inventory.clone())
        }
    }

    fn provider(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    fn observed(terminal: &str, tab: &str, pane: &str, provider_value: &str) -> ObservedWorker {
        ObservedWorker {
            runtime_id: terminal.to_owned(),
            terminal_id: terminal.to_owned(),
            workspace_id: "workspace-1".to_owned(),
            tab_id: tab.to_owned(),
            pane_id: pane.to_owned(),
            name: Some(terminal.to_owned()),
            provider: Some("codex".to_owned()),
            display_provider: Some("Codex".to_owned()),
            status: ObservedStatus::Working,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 1,
            cwd: None,
            foreground_cwd: None,
            tokens: BTreeMap::new(),
            provider_session: Some(provider(provider_value)),
            revision: 1,
        }
    }

    fn inventory() -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "test".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 20,
            focus: FocusObservation::default(),
            workspaces: vec![WorkspaceObservation {
                runtime_id: "workspace-1".to_owned(),
                order: 0,
                label: "Project".to_owned(),
                focused: true,
                active_tab_id: "tab-current".to_owned(),
                pane_count: 2,
                tab_count: 2,
                status: ObservedStatus::Working,
                tokens: BTreeMap::new(),
                worktree: None,
            }],
            tabs: Vec::new(),
            panes: Vec::new(),
            workers: vec![
                observed("terminal-current", "tab-current", "pane-current", "current"),
                observed(
                    "terminal-candidate",
                    "tab-candidate",
                    "pane-candidate",
                    "candidate",
                ),
            ],
            child_agents: Vec::new(),
        }
    }

    async fn fixture(
        inventory: RuntimeInventory,
    ) -> (
        ProjectOrchestratorTransferService,
        String,
        TransferProjectOrchestrator,
        TempDir,
    ) {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let current = &inventory.workers[0];
        let runtime = WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: current.terminal_id.clone(),
            tab_id: Some(current.tab_id.clone()),
            pane_id: current.pane_id.clone(),
            provider_session: current.provider_session.clone(),
            owns_tab: false,
            observation_state: yard_domain::RuntimeObservationState::Observed,
            process_state: yard_domain::RuntimeProcessState::Running,
            status: current.status,
            state_change_sequence: current.state_change_sequence,
            revision: current.revision,
            version: 1,
            last_observed_at_unix_ms: 1,
        };
        let project = store
            .create_project(
                CreateProject {
                    name: "Project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                    },
                    orchestrator_observed_worker_id: current.terminal_id.clone(),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                runtime,
            )
            .await
            .unwrap();
        seed_inventory_workers(
            &temp.path().join("yard.sqlite3"),
            &inventory,
            &["terminal-candidate"],
        );
        store
            .reconcile_runtime_inventory(inventory.clone())
            .await
            .unwrap();
        let project = store.get_project(&project.id).await.unwrap();
        let candidate = store
            .list_worker_candidates()
            .await
            .unwrap()
            .workers
            .into_iter()
            .find(|candidate| {
                candidate
                    .worker
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.terminal_id == "terminal-candidate")
            })
            .unwrap();
        let command = TransferProjectOrchestrator {
            command_id: "transfer".to_owned(),
            actor: "local-user".to_owned(),
            worker_id: candidate.worker.id,
            expected_worker_version: candidate.worker.version,
            expected_worker_runtime: candidate.worker.runtime.unwrap(),
            expected_project_version: project.version,
            expected_orchestrator_worker_id: project.orchestrator.id,
            expected_orchestrator_worker_version: project.orchestrator.version,
            expected_orchestrator_runtime: project.orchestrator.runtime.unwrap(),
        };
        let source = Arc::new(StaticInventory { inventory });
        let service = ProjectOrchestratorTransferService::new(source, store);
        (service, project.id, command, temp)
    }

    #[tokio::test]
    async fn verifies_fresh_identity_then_commits_and_replays() {
        let (service, project_id, command, _temp) = fixture(inventory()).await;
        let worker_id = command.worker_id.clone();

        let transferred = service
            .transfer(&project_id, command.clone())
            .await
            .unwrap();
        let replayed = service.transfer(&project_id, command).await.unwrap();

        assert_eq!(transferred.project.orchestrator.id, worker_id);
        assert!(!transferred.replayed);
        assert!(replayed.replayed);
    }

    #[tokio::test]
    async fn rejects_ambiguous_provider_identity_before_commit() {
        let mut duplicate = inventory();
        duplicate.workers.push(observed(
            "terminal-duplicate",
            "tab-duplicate",
            "pane-duplicate",
            "candidate",
        ));
        let (service, project_id, command, _temp) = fixture(duplicate).await;

        let error = service.transfer(&project_id, command).await.unwrap_err();

        assert!(matches!(
            error,
            ProjectOrchestratorTransferServiceError::RuntimeIdentityAmbiguous
        ));
    }
}

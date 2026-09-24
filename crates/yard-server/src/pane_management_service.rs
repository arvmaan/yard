use std::{collections::HashMap, sync::Arc, time::Duration};

use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, warn};
use yard_domain::{
    MAX_PANE_MANAGEMENT_CANDIDATES, ManageAllAgents, ObservedWorker, PaneManagementBatchResult,
    PaneManagementCandidate, PaneManagementCategory, PaneManagementItemResult,
    PaneManagementOutcome, PaneManagementPreview, Project, RuntimeInventory, WorkerCandidate,
};
use yard_herdr::{
    AcquirePaneLeaseRequest, LeaseToken, PaneManagementCapability, PaneManagementRpcError,
    ReleasePaneLeaseRequest, RenewPaneLeaseRequest,
};
use yard_store::{
    BeginPaneManagementBatch, ManagedPaneAdoption, ProjectStoreError, StoredLeaseToken,
    StoredPaneManagementLease, YardStore,
};

use crate::inventory_service::{
    InventoryServiceError, InventorySource, runtime_binding_from_observed_worker,
};

const LEASE_TTL_MS: u64 = 60_000;
const RENEWAL_INTERVAL: Duration = Duration::from_secs(10);
const RENEWAL_BATCH_SIZE: usize = 100;

#[derive(Clone)]
pub struct PaneManagementService {
    source: Arc<dyn InventorySource>,
    store: Arc<dyn YardStore>,
}

#[derive(Debug, Error)]
pub enum PaneManagementServiceError {
    #[error(transparent)]
    InvalidCommand(#[from] yard_domain::PaneManagementValidationError),
    #[error(transparent)]
    Inventory(#[from] InventoryServiceError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

#[derive(Clone)]
struct ClassifiedCandidate {
    public: PaneManagementCandidate,
    worker: Option<ObservedWorker>,
    observed_at_unix_ms: u64,
}

impl PaneManagementService {
    #[must_use]
    pub fn new(source: Arc<dyn InventorySource>, store: Arc<dyn YardStore>) -> Self {
        Self { source, store }
    }

    /// Build one bounded, read-only pane-management preview.
    ///
    /// # Errors
    ///
    /// Returns an error when inventory or durable Yard state cannot be read.
    pub async fn preview(&self) -> Result<PaneManagementPreview, PaneManagementServiceError> {
        let (classified, endpoint) = self.classify().await?;
        Ok(preview_from(classified, endpoint))
    }

    /// Acquire and durably adopt every explicitly confirmed eligible pane.
    ///
    /// # Errors
    ///
    /// Returns an error when command validation or batch persistence fails.
    pub async fn manage_all_agents(
        &self,
        command: ManageAllAgents,
    ) -> Result<PaneManagementBatchResult, PaneManagementServiceError> {
        let command = command.normalize()?;
        let input_hash = input_hash(&command);
        let (classified, endpoint) = self.classify().await?;
        if endpoint.is_none() {
            return Ok(PaneManagementBatchResult {
                command_id: command.command_id,
                replayed: false,
                managed_count: 0,
                results: command
                    .candidate_keys
                    .into_iter()
                    .map(|candidate_key| {
                        item(
                            &candidate_key,
                            PaneManagementOutcome::Skipped,
                            "Herdr does not support pane management leases.",
                            None,
                        )
                    })
                    .collect(),
            });
        }
        match self
            .store
            .begin_pane_management_batch(&command.command_id, &command.actor, &input_hash)
            .await?
        {
            BeginPaneManagementBatch::Replayed(mut result) => {
                result.replayed = true;
                return Ok(result);
            }
            BeginPaneManagementBatch::Started => {}
        }

        let installation_uuid = self.store.pane_management_installation_uuid().await?;
        let owner_id = format!("yard:{installation_uuid}");
        let candidates = classified
            .into_iter()
            .map(|candidate| (candidate.public.candidate_key.clone(), candidate))
            .collect::<HashMap<_, _>>();
        let mut results = Vec::with_capacity(command.candidate_keys.len());

        for candidate_key in &command.candidate_keys {
            let Some(candidate) = candidates.get(candidate_key) else {
                results.push(item(
                    candidate_key,
                    PaneManagementOutcome::Skipped,
                    "Candidate is no longer present in the live preview.",
                    None,
                ));
                continue;
            };
            if candidate.public.category != PaneManagementCategory::Eligible {
                results.push(item(
                    candidate_key,
                    outcome_for(candidate.public.category),
                    &candidate.public.reason,
                    candidate.public.worker_id.clone(),
                ));
                continue;
            }
            let Some(worker) = candidate.worker.as_ref() else {
                results.push(item(
                    candidate_key,
                    PaneManagementOutcome::Failed,
                    "Live worker evidence disappeared before acquisition.",
                    None,
                ));
                continue;
            };
            let Some(project_id) = candidate.public.project_id.as_deref() else {
                results.push(item(
                    candidate_key,
                    PaneManagementOutcome::Failed,
                    "Project association disappeared before acquisition.",
                    None,
                ));
                continue;
            };
            let request_suffix = short_hash(candidate_key);
            let acquisition_request_id =
                format!("yard:{}:{request_suffix}:acquire", command.command_id);
            let lease = match self
                .source
                .acquire_pane_lease(AcquirePaneLeaseRequest {
                    request_id: acquisition_request_id.clone(),
                    acquisition_request_id: acquisition_request_id.clone(),
                    session: candidate.public.session.clone(),
                    pane_id: worker.pane_id.clone(),
                    pane_instance_id: worker.pane_instance_id.clone().unwrap_or_default(),
                    owner_id: owner_id.clone(),
                    ttl_ms: LEASE_TTL_MS,
                })
                .await
            {
                Ok(lease) => lease,
                Err(PaneManagementRpcError::Conflict) => {
                    results.push(item(
                        candidate_key,
                        PaneManagementOutcome::Conflict,
                        "Pane is managed by another owner.",
                        None,
                    ));
                    continue;
                }
                Err(PaneManagementRpcError::Unsupported) => {
                    results.push(item(
                        candidate_key,
                        PaneManagementOutcome::Skipped,
                        "Herdr does not support pane management leases.",
                        None,
                    ));
                    continue;
                }
                Err(error) => {
                    results.push(item(
                        candidate_key,
                        PaneManagementOutcome::Failed,
                        &error.to_string(),
                        None,
                    ));
                    continue;
                }
            };
            if lease.pane_id != worker.pane_id
                || Some(lease.pane_instance_id.as_str()) != worker.pane_instance_id.as_deref()
                || lease.owner_id != owner_id
            {
                results.push(item(
                    candidate_key,
                    PaneManagementOutcome::Failed,
                    "Herdr returned a lease for a different pane identity.",
                    None,
                ));
                continue;
            }
            let runtime = runtime_binding_from_observed_worker(
                &RuntimeInventory {
                    adapter: "herdr".to_owned(),
                    session: candidate.public.session.clone(),
                    runtime_version: String::new(),
                    protocol: 0,
                    observed_at_unix_ms: candidate.observed_at_unix_ms,
                    focus: Default::default(),
                    workspaces: Vec::new(),
                    tabs: Vec::new(),
                    panes: Vec::new(),
                    workers: Vec::new(),
                    child_agents: Vec::new(),
                },
                worker,
                false,
            );
            let adoption = ManagedPaneAdoption {
                command_id: command.command_id.clone(),
                project_id: project_id.to_owned(),
                runtime,
                installation_uuid: installation_uuid.clone(),
                owner_id: owner_id.clone(),
                token: StoredLeaseToken::from_secret(lease.token.expose_secret().to_owned()),
                pane_instance_id: lease.pane_instance_id.clone(),
                expires_at_unix_ms: lease.expires_at_unix_ms,
                acquisition_request_id: acquisition_request_id.clone(),
            };
            match self.store.adopt_managed_pane(adoption).await {
                Ok(worker_id) => results.push(item(
                    candidate_key,
                    PaneManagementOutcome::Managed,
                    "Pane is now managed by Yard.",
                    Some(worker_id),
                )),
                Err(error) => {
                    let rollback = self
                        .source
                        .release_pane_lease(ReleasePaneLeaseRequest {
                            request_id: format!(
                                "yard:{}:{request_suffix}:release",
                                command.command_id
                            ),
                            session: candidate.public.session.clone(),
                            pane_id: lease.pane_id,
                            pane_instance_id: lease.pane_instance_id,
                            owner_id: owner_id.clone(),
                            token: lease.token,
                        })
                        .await;
                    results.push(item(
                        candidate_key,
                        if rollback.is_ok() {
                            PaneManagementOutcome::Failed
                        } else {
                            PaneManagementOutcome::RollbackFailed
                        },
                        &error.to_string(),
                        None,
                    ));
                }
            }
        }

        let result = PaneManagementBatchResult {
            command_id: command.command_id,
            replayed: false,
            managed_count: results
                .iter()
                .filter(|result| result.outcome == PaneManagementOutcome::Managed)
                .count(),
            results,
        };
        self.store
            .complete_pane_management_batch(result.clone())
            .await?;
        Ok(result)
    }

    /// Renew one bounded batch of due leases.
    ///
    /// # Errors
    ///
    /// Returns an error only when durable lease state cannot be read or saved.
    pub async fn renew_due(&self) -> Result<usize, ProjectStoreError> {
        let leases = self
            .store
            .list_due_pane_management_leases(now_unix_ms(), RENEWAL_BATCH_SIZE)
            .await?;
        let attempted = leases.len();
        for lease in leases {
            let request = RenewPaneLeaseRequest {
                request_id: format!(
                    "yard:renew:{}:{}",
                    lease.worker_id, lease.expires_at_unix_ms
                ),
                session: lease.session.clone(),
                pane_id: lease.pane_id.clone(),
                pane_instance_id: lease.pane_instance_id.clone(),
                owner_id: lease.owner_id.clone(),
                token: LeaseToken::from_secret(lease.token.expose_secret().to_owned()),
                ttl_ms: LEASE_TTL_MS,
            };
            match self.source.renew_pane_lease(request).await {
                Ok(result) => {
                    if let Some(expires_at) = result.expires_at_unix_ms {
                        self.store
                            .renew_pane_management_lease(&lease.worker_id, expires_at)
                            .await?;
                    } else {
                        self.store
                            .mark_pane_management_recovery_required(
                                &lease.worker_id,
                                "missing_expiry",
                            )
                            .await?;
                    }
                }
                Err(error) if lease_lost(&error) => {
                    self.store
                        .mark_pane_management_recovery_required(
                            &lease.worker_id,
                            error_code(&error),
                        )
                        .await?;
                }
                Err(error) => {
                    warn!(worker_id = %lease.worker_id, %error, "Pane lease renewal failed; retrying before expiry")
                }
            }
        }
        Ok(attempted)
    }

    pub async fn run(self) {
        loop {
            match self.renew_due().await {
                Ok(attempted) if attempted > 0 => {
                    debug!(attempted, "Renewed due pane management leases");
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "Pane management renewal pass failed"),
            }
            tokio::time::sleep(RENEWAL_INTERVAL).await;
        }
    }

    async fn classify(
        &self,
    ) -> Result<(Vec<ClassifiedCandidate>, Option<String>), PaneManagementServiceError> {
        let projects = self.store.list_projects().await?.projects;
        let workers = self.store.list_worker_candidates().await?.workers;
        let leases = self.store.list_pane_management_leases().await?;
        let sessions = self.source.sessions().await?;
        let mut candidates = Vec::new();
        let mut endpoint = None;
        for session in sessions
            .sessions
            .into_iter()
            .filter(|session| session.running)
        {
            let capability = self
                .source
                .pane_management_capability(&session.name)
                .await
                .unwrap_or_else(|_| PaneManagementCapability::unsupported());
            if capability.supported && endpoint.is_none() {
                endpoint.clone_from(&capability.endpoint);
            }
            let inventory = self.source.inventory(&session.name).await?;
            classify_inventory(
                &inventory,
                &projects,
                &workers,
                &leases,
                &capability,
                &mut candidates,
            );
            if candidates.len() >= MAX_PANE_MANAGEMENT_CANDIDATES {
                break;
            }
        }
        Ok((candidates, endpoint))
    }
}

fn classify_inventory(
    inventory: &RuntimeInventory,
    projects: &[Project],
    existing_workers: &[WorkerCandidate],
    leases: &[StoredPaneManagementLease],
    capability: &PaneManagementCapability,
    candidates: &mut Vec<ClassifiedCandidate>,
) {
    for pane in &inventory.panes {
        if candidates.len() >= MAX_PANE_MANAGEMENT_CANDIDATES {
            return;
        }
        let matching_workers = inventory
            .workers
            .iter()
            .filter(|worker| {
                worker.pane_id == pane.runtime_id || worker.terminal_id == pane.terminal_id
            })
            .collect::<Vec<_>>();
        if matching_workers.is_empty() && pane.provider.is_none() {
            continue;
        }
        let worker = (matching_workers.len() == 1).then(|| matching_workers[0]);
        let pane_instance_id = pane.pane_instance_id.clone();
        let candidate_key = format!(
            "{}:{}:{}",
            inventory.session,
            pane.runtime_id,
            pane_instance_id.as_deref().unwrap_or("missing")
        );
        let project_matches = projects
            .iter()
            .filter(|project| {
                project.runtime.adapter == inventory.adapter
                    && project.runtime.session == inventory.session
                    && project.runtime.workspace_id == pane.workspace_id
            })
            .collect::<Vec<_>>();
        let coherent = worker.is_some_and(|worker| {
            pane_instance_id.is_some()
                && worker.pane_instance_id == pane_instance_id
                && worker.workspace_id == pane.workspace_id
                && worker.tab_id == pane.tab_id
                && worker.pane_id == pane.runtime_id
                && worker.terminal_id == pane.terminal_id
                && worker.interactive_ready
                && !worker.launch_pending
        }) && inventory
            .panes
            .iter()
            .filter(|candidate| candidate.runtime_id == pane.runtime_id)
            .count()
            == 1;
        let existing = existing_workers.iter().find(|candidate| {
            candidate.worker.runtime.as_ref().is_some_and(|runtime| {
                runtime.adapter == inventory.adapter
                    && runtime.session == inventory.session
                    && (runtime.terminal_id == pane.terminal_id
                        || runtime.pane_id == pane.runtime_id)
            })
        });
        let lease = pane_instance_id.as_deref().and_then(|instance| {
            leases.iter().find(|lease| {
                lease.session == inventory.session
                    && lease.pane_id == pane.runtime_id
                    && lease.pane_instance_id == instance
            })
        });
        let (category, reason, recovery_required, controls_enabled) = if !coherent {
            (
                PaneManagementCategory::Ambiguous,
                "Pane does not have one coherent, interactive live agent record.",
                false,
                false,
            )
        } else if project_matches.len() != 1 {
            (
                PaneManagementCategory::Ambiguous,
                "Pane workspace is not explicitly associated with exactly one Yard project.",
                false,
                false,
            )
        } else if let Some(lease) = lease {
            (
                PaneManagementCategory::AlreadyManaged,
                if lease.recovery_required {
                    "Yard lost the pane lease; recovery is required."
                } else {
                    "Pane is already managed by Yard."
                },
                lease.recovery_required,
                !lease.recovery_required,
            )
        } else if existing.is_some() {
            (
                PaneManagementCategory::Conflict,
                "Pane is already bound to an existing Yard worker and was left untouched.",
                false,
                false,
            )
        } else if !capability.supported {
            (
                PaneManagementCategory::Unsupported,
                "This Herdr session does not support pane management leases.",
                false,
                false,
            )
        } else {
            (
                PaneManagementCategory::Eligible,
                "Live agent pane is eligible for Yard management.",
                false,
                true,
            )
        };
        let project = (project_matches.len() == 1).then(|| project_matches[0]);
        candidates.push(ClassifiedCandidate {
            public: PaneManagementCandidate {
                candidate_key,
                category,
                reason: reason.to_owned(),
                session: inventory.session.clone(),
                workspace_id: Some(pane.workspace_id.clone()),
                workspace_label: inventory
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.runtime_id == pane.workspace_id)
                    .map(|workspace| workspace.label.clone()),
                pane_id: Some(pane.runtime_id.clone()),
                pane_instance_id,
                terminal_id: Some(pane.terminal_id.clone()),
                project_id: project.map(|project| project.id.clone()),
                project_name: project.map(|project| project.name.clone()),
                worker_id: lease
                    .map(|lease| lease.worker_id.clone())
                    .or_else(|| existing.map(|candidate| candidate.worker.id.clone())),
                provider: worker.and_then(|worker| worker.provider.clone()),
                display_provider: worker.and_then(|worker| worker.display_provider.clone()),
                recovery_required,
                management_controls_enabled: controls_enabled,
            },
            worker: worker.cloned(),
            observed_at_unix_ms: inventory.observed_at_unix_ms,
        });
    }
}

fn preview_from(
    candidates: Vec<ClassifiedCandidate>,
    endpoint: Option<String>,
) -> PaneManagementPreview {
    let supported = endpoint.is_some();
    let eligible_count = candidates
        .iter()
        .filter(|candidate| candidate.public.category == PaneManagementCategory::Eligible)
        .count();
    let candidate_count = candidates.len();
    PaneManagementPreview {
        supported,
        endpoint,
        limit: MAX_PANE_MANAGEMENT_CANDIDATES,
        truncated: candidate_count == MAX_PANE_MANAGEMENT_CANDIDATES,
        candidate_count,
        eligible_count,
        candidates: candidates
            .into_iter()
            .map(|candidate| candidate.public)
            .collect(),
    }
}

fn item(
    candidate_key: &str,
    outcome: PaneManagementOutcome,
    reason: &str,
    worker_id: Option<String>,
) -> PaneManagementItemResult {
    PaneManagementItemResult {
        candidate_key: candidate_key.to_owned(),
        outcome,
        reason: reason.to_owned(),
        worker_id,
    }
}

const fn outcome_for(category: PaneManagementCategory) -> PaneManagementOutcome {
    match category {
        PaneManagementCategory::AlreadyManaged => PaneManagementOutcome::AlreadyManaged,
        PaneManagementCategory::Conflict => PaneManagementOutcome::Conflict,
        PaneManagementCategory::Eligible
        | PaneManagementCategory::Ambiguous
        | PaneManagementCategory::Unsupported => PaneManagementOutcome::Skipped,
    }
}

fn input_hash(command: &ManageAllAgents) -> String {
    let bytes = serde_json::to_vec(command).expect("validated command must serialize");
    format!("{:x}", Sha256::digest(bytes))
}

fn short_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_owned()
}

fn lease_lost(error: &PaneManagementRpcError) -> bool {
    matches!(
        error,
        PaneManagementRpcError::Conflict
            | PaneManagementRpcError::NotFound
            | PaneManagementRpcError::Expired
            | PaneManagementRpcError::OwnerMismatch
            | PaneManagementRpcError::TokenMismatch
            | PaneManagementRpcError::PaneInstanceMismatch
            | PaneManagementRpcError::ReplayConflict
            | PaneManagementRpcError::Unsupported
    )
}

const fn error_code(error: &PaneManagementRpcError) -> &'static str {
    match error {
        PaneManagementRpcError::Unsupported => "unsupported",
        PaneManagementRpcError::Conflict => "conflict",
        PaneManagementRpcError::NotFound => "not_found",
        PaneManagementRpcError::Expired => "expired",
        PaneManagementRpcError::OwnerMismatch => "owner_mismatch",
        PaneManagementRpcError::TokenMismatch => "token_mismatch",
        PaneManagementRpcError::PaneInstanceMismatch => "pane_instance_mismatch",
        PaneManagementRpcError::ReplayConflict => "replay_conflict",
        PaneManagementRpcError::Decode(_) | PaneManagementRpcError::Runtime(_) => "runtime_error",
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use async_trait::async_trait;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, FocusObservation, ManageAllAgents, ObservedStatus, PaneObservation,
        ProjectPlacement, ProjectRuntimeBinding, ProjectWorkflowProfilePin,
        RuntimeObservationState, RuntimeProcessState, RuntimeSession, RuntimeSessions,
        TabObservation, Worker, WorkerAvailability, WorkerDesiredState, WorkerRuntimeBinding,
        WorkspaceObservation,
    };
    use yard_herdr::{AcquirePaneLeaseRequest, PaneLease};
    use yard_store::{BeginPaneManagementBatch, SqliteProjectStore, YardStore};

    use super::*;
    use crate::inventory_service::{InventoryServiceError, InventorySource};

    fn runtime(terminal: &str, pane: &str) -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "alpha".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: terminal.to_owned(),
            tab_id: Some(format!("tab-{pane}")),
            pane_id: pane.to_owned(),
            provider_session: None,
            owns_tab: false,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Working,
            state_change_sequence: 1,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: 100,
        }
    }

    fn project() -> Project {
        Project {
            id: "project-1".to_owned(),
            name: "Project one".to_owned(),
            runtime: ProjectRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "alpha".to_owned(),
                workspace_id: "workspace-1".to_owned(),
            },
            orchestrator: Worker {
                id: "orchestrator".to_owned(),
                profile_id: None,
                profile_version: None,
                desired_state: WorkerDesiredState::Running,
                runtime: Some(runtime("terminal-orchestrator", "pane-orchestrator")),
                version: 1,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            },
            placement: ProjectPlacement {
                geometry: CanvasPlacement {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0,
                },
                version: 1,
                updated_at_unix_ms: 1,
            },
            workflow_profile: ProjectWorkflowProfilePin {
                profile_id: "profile".to_owned(),
                profile_version: 1,
                pinned_by: "test".to_owned(),
                pinned_at_unix_ms: 1,
            },
            version: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn inventory(count: usize) -> RuntimeInventory {
        let panes = (0..count)
            .map(|index| PaneObservation {
                runtime_id: format!("pane-{index}"),
                pane_instance_id: Some(format!("instance-{index}")),
                terminal_id: format!("terminal-{index}"),
                workspace_id: "workspace-1".to_owned(),
                tab_id: format!("tab-pane-{index}"),
                focused: false,
                cwd: None,
                foreground_cwd: None,
                label: None,
                provider: Some("codex".to_owned()),
                display_provider: Some("Codex".to_owned()),
                status: ObservedStatus::Working,
                tokens: BTreeMap::new(),
                provider_session: None,
                revision: 1,
            })
            .collect::<Vec<_>>();
        let workers = (0..count)
            .map(|index| ObservedWorker {
                runtime_id: format!("terminal-{index}"),
                pane_instance_id: Some(format!("instance-{index}")),
                terminal_id: format!("terminal-{index}"),
                workspace_id: "workspace-1".to_owned(),
                tab_id: format!("tab-pane-{index}"),
                pane_id: format!("pane-{index}"),
                name: None,
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
                provider_session: None,
                revision: 1,
            })
            .collect();
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "alpha".to_owned(),
            runtime_version: "0.10.0".to_owned(),
            protocol: 23,
            observed_at_unix_ms: 100,
            focus: FocusObservation::default(),
            workspaces: vec![WorkspaceObservation {
                runtime_id: "workspace-1".to_owned(),
                order: 0,
                label: "Project one".to_owned(),
                focused: false,
                active_tab_id: "tab-pane-0".to_owned(),
                pane_count: count,
                tab_count: count,
                status: ObservedStatus::Working,
                tokens: BTreeMap::new(),
                worktree: None,
            }],
            tabs: vec![TabObservation {
                runtime_id: "tab-pane-0".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                order: 0,
                label: "Agent".to_owned(),
                focused: false,
                pane_count: 1,
                status: ObservedStatus::Working,
            }],
            panes,
            workers,
            child_agents: Vec::new(),
        }
    }

    fn capability(supported: bool) -> PaneManagementCapability {
        PaneManagementCapability {
            supported,
            endpoint: supported.then(|| "pane_management_lease_v1".to_owned()),
        }
    }

    #[test]
    fn preview_classifies_eligible_conflict_and_ambiguous() {
        let mut observed = inventory(3);
        observed.workers[2].interactive_ready = false;
        let existing = WorkerCandidate {
            worker: Worker {
                id: "existing".to_owned(),
                profile_id: None,
                profile_version: None,
                desired_state: WorkerDesiredState::Running,
                runtime: Some(runtime("terminal-1", "pane-1")),
                version: 1,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            },
            profile_name: None,
            default_role: None,
            availability: WorkerAvailability::UnassignedLive,
            project_id: None,
            assignment_id: None,
            reason: None,
        };
        let mut candidates = Vec::new();
        classify_inventory(
            &observed,
            &[project()],
            &[existing],
            &[],
            &capability(true),
            &mut candidates,
        );
        assert_eq!(
            candidates[0].public.category,
            PaneManagementCategory::Eligible
        );
        assert_eq!(
            candidates[1].public.category,
            PaneManagementCategory::Conflict
        );
        assert_eq!(
            candidates[2].public.category,
            PaneManagementCategory::Ambiguous
        );
    }

    #[test]
    fn herdr_0_9_style_capability_is_unsupported() {
        let mut candidates = Vec::new();
        classify_inventory(
            &inventory(1),
            &[project()],
            &[],
            &[],
            &capability(false),
            &mut candidates,
        );
        assert_eq!(
            candidates[0].public.category,
            PaneManagementCategory::Unsupported
        );
    }

    #[test]
    fn preview_is_bounded_at_five_hundred_panes() {
        let mut candidates = Vec::new();
        classify_inventory(
            &inventory(501),
            &[project()],
            &[],
            &[],
            &capability(true),
            &mut candidates,
        );
        let preview = preview_from(candidates, Some("pane_management_lease_v1".to_owned()));
        assert_eq!(preview.candidate_count, 500);
        assert!(preview.truncated);
    }

    struct Herdr090Inventory;

    #[async_trait]
    impl InventorySource for Herdr090Inventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![RuntimeSession {
                    name: "alpha".to_owned(),
                    is_default: true,
                    running: true,
                }],
            })
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let mut inventory = inventory(1);
            inventory.runtime_version = "0.9.0".to_owned();
            inventory.protocol = 22;
            inventory.panes[0].cwd = Some("/projects/looks-associated".to_owned());
            inventory.workers[0].cwd = Some("/projects/looks-associated".to_owned());
            Ok(inventory)
        }

        async fn acquire_pane_lease(
            &self,
            _request: AcquirePaneLeaseRequest,
        ) -> Result<PaneLease, PaneManagementRpcError> {
            panic!("unsupported Herdr must remain read-only")
        }
    }

    #[tokio::test]
    async fn herdr_0_9_without_claim_capabilities_is_read_only() {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        let service = PaneManagementService::new(Arc::new(Herdr090Inventory), store.clone());
        let preview = service.preview().await.unwrap();
        assert!(!preview.supported);
        assert_eq!(preview.endpoint, None);
        assert_eq!(preview.candidates[0].project_id, None);
        assert_eq!(
            preview.candidates[0].category,
            PaneManagementCategory::Ambiguous
        );
        assert!(!preview.candidates[0].management_controls_enabled);

        let command = ManageAllAgents {
            command_id: "unsupported-batch".to_owned(),
            actor: "test".to_owned(),
            confirmed: true,
            candidate_keys: vec![preview.candidates[0].candidate_key.clone()],
        };
        let hash = input_hash(&command.clone().normalize().unwrap());
        let result = service.manage_all_agents(command).await.unwrap();
        assert_eq!(result.managed_count, 0);
        assert_eq!(result.results[0].outcome, PaneManagementOutcome::Skipped);
        assert!(matches!(
            store
                .begin_pane_management_batch("unsupported-batch", "test", &hash)
                .await
                .unwrap(),
            BeginPaneManagementBatch::Started
        ));
    }

    #[test]
    fn typed_lease_loss_disables_management_until_recovery() {
        assert!(lease_lost(&PaneManagementRpcError::Expired));
        assert!(lease_lost(&PaneManagementRpcError::PaneInstanceMismatch));
        assert!(!lease_lost(&PaneManagementRpcError::Runtime(
            yard_herdr::HerdrError::MissingCommandResult,
        )));
    }
}

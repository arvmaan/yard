use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use thiserror::Error;
use tracing::warn;
use yard_domain::{
    CleanupAdvisorRecommendation, CleanupAdvisorRequest, CleanupAdvisorResult,
    StartWorkerCleanupRun, WorkerCleanupItemStatus, WorkerCleanupPolicy, WorkerCleanupRunTrigger,
};
use yard_store::{ClaimedWorkerCleanupItem, ProjectStoreError, YardStore};

use crate::{
    cleanup_retirement::{
        CleanupRetirement, CleanupRetirementCapability, CleanupRetirementError,
        CloseManagedPaneRequest,
    },
    inventory_service::InventorySource,
};

const CLAIM_LIMIT: usize = 100;
const CLAIM_TTL_MS: u64 = 30_000;
const RETRY_BASE_MS: u64 = 1_000;
const RETRY_CAP_MS: u64 = 60_000;
const MAX_ADVISOR_ATTEMPTS: u32 = 3;
const LOOP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CleanupAdvisorError {
    #[error("cleanup advisor invocation is unsupported")]
    Unsupported,
    #[error("cleanup advisor returned malformed output: {0}")]
    Malformed(String),
    #[error("cleanup advisor invocation failed: {0}")]
    Transient(String),
}

#[async_trait]
pub trait CleanupAdvisorInvoker: Send + Sync {
    async fn invoke(
        &self,
        _profile_id: &str,
        _request: CleanupAdvisorRequest,
    ) -> Result<CleanupAdvisorResult, CleanupAdvisorError> {
        Err(CleanupAdvisorError::Unsupported)
    }
}

struct UnsupportedCleanupAdvisor;

#[async_trait]
impl CleanupAdvisorInvoker for UnsupportedCleanupAdvisor {}

#[async_trait]
trait WorkerCleanupPersistence: Send + Sync {
    async fn policy(&self) -> Result<WorkerCleanupPolicy, ProjectStoreError>;
    async fn schedule(
        &self,
        trigger: WorkerCleanupRunTrigger,
        command: StartWorkerCleanupRun,
    ) -> Result<(), ProjectStoreError>;
    async fn latest_runs(&self) -> Result<yard_domain::WorkerCleanupRuns, ProjectStoreError>;
    async fn claim(&self) -> Result<Vec<ClaimedWorkerCleanupItem>, ProjectStoreError>;
    async fn finish(
        &self,
        item: &ClaimedWorkerCleanupItem,
        status: WorkerCleanupItemStatus,
        reason: &str,
    ) -> Result<(), ProjectStoreError>;
    async fn retry(
        &self,
        item: &ClaimedWorkerCleanupItem,
        reason: &str,
        retry_after_ms: u64,
    ) -> Result<(), ProjectStoreError>;
    async fn reconcile_missing(
        &self,
        item: &ClaimedWorkerCleanupItem,
        observed_at_unix_ms: u64,
    ) -> Result<(), ProjectStoreError>;
    async fn authorize_close(
        &self,
        item: &ClaimedWorkerCleanupItem,
    ) -> Result<(), ProjectStoreError>;
    async fn record_advisor(
        &self,
        item: &ClaimedWorkerCleanupItem,
        result: &CleanupAdvisorResult,
    ) -> Result<(), ProjectStoreError>;
}

struct StorePersistence {
    store: Arc<dyn YardStore>,
}

#[async_trait]
impl WorkerCleanupPersistence for StorePersistence {
    async fn policy(&self) -> Result<WorkerCleanupPolicy, ProjectStoreError> {
        self.store.get_worker_cleanup_policy().await
    }

    async fn schedule(
        &self,
        trigger: WorkerCleanupRunTrigger,
        command: StartWorkerCleanupRun,
    ) -> Result<(), ProjectStoreError> {
        self.store
            .start_worker_cleanup_run(trigger, command)
            .await?;
        Ok(())
    }

    async fn latest_runs(&self) -> Result<yard_domain::WorkerCleanupRuns, ProjectStoreError> {
        self.store.list_worker_cleanup_runs(50).await
    }

    async fn claim(&self) -> Result<Vec<ClaimedWorkerCleanupItem>, ProjectStoreError> {
        self.store
            .claim_worker_cleanup_items(None, CLAIM_LIMIT, CLAIM_TTL_MS)
            .await
    }

    async fn finish(
        &self,
        item: &ClaimedWorkerCleanupItem,
        status: WorkerCleanupItemStatus,
        reason: &str,
    ) -> Result<(), ProjectStoreError> {
        self.store
            .finish_worker_cleanup_item(
                &item.run_id,
                &item.worker_id,
                &item.claim_token,
                status,
                reason,
            )
            .await
    }

    async fn retry(
        &self,
        item: &ClaimedWorkerCleanupItem,
        reason: &str,
        retry_after_ms: u64,
    ) -> Result<(), ProjectStoreError> {
        self.store
            .retry_worker_cleanup_item(
                &item.run_id,
                &item.worker_id,
                &item.claim_token,
                reason,
                retry_after_ms,
            )
            .await
    }

    async fn reconcile_missing(
        &self,
        item: &ClaimedWorkerCleanupItem,
        observed_at_unix_ms: u64,
    ) -> Result<(), ProjectStoreError> {
        self.store
            .reconcile_missing_worker_cleanup_item(item, observed_at_unix_ms)
            .await
    }

    async fn authorize_close(
        &self,
        item: &ClaimedWorkerCleanupItem,
    ) -> Result<(), ProjectStoreError> {
        self.store.authorize_worker_cleanup_close(item).await
    }

    async fn record_advisor(
        &self,
        item: &ClaimedWorkerCleanupItem,
        result: &CleanupAdvisorResult,
    ) -> Result<(), ProjectStoreError> {
        self.store
            .record_cleanup_advisor_assignment(
                item,
                &item.worker_id,
                &result.advisor_worker_id,
                &result.advisor_assignment_id,
            )
            .await?;
        self.store
            .record_cleanup_advisor_artifact(
                item,
                &result.completion_receipt_id,
                &result.artifact_id,
                result.artifact.clone(),
            )
            .await
    }
}

#[derive(Clone)]
pub struct WorkerCleanupService {
    source: Arc<dyn InventorySource>,
    persistence: Arc<dyn WorkerCleanupPersistence>,
    retirement: Arc<dyn CleanupRetirement>,
    advisor: Arc<dyn CleanupAdvisorInvoker>,
}

impl WorkerCleanupService {
    #[must_use]
    pub fn new(source: Arc<dyn InventorySource>, store: Arc<dyn YardStore>) -> Self {
        Self {
            source,
            persistence: Arc::new(StorePersistence { store }),
            retirement: Arc::new(ReadOnlyRetirement),
            advisor: Arc::new(UnsupportedCleanupAdvisor),
        }
    }

    #[cfg(test)]
    fn with_components(
        source: Arc<dyn InventorySource>,
        persistence: Arc<dyn WorkerCleanupPersistence>,
        retirement: Arc<dyn CleanupRetirement>,
        advisor: Arc<dyn CleanupAdvisorInvoker>,
    ) -> Self {
        Self {
            source,
            persistence,
            retirement,
            advisor,
        }
    }

    pub async fn process_pending(&self) -> Result<usize, ProjectStoreError> {
        let items = self.persistence.claim().await?;
        let count = items.len();
        for item in items {
            self.process_item(&item).await?;
        }
        Ok(count)
    }

    pub async fn run(self) {
        loop {
            if let Err(error) = self.schedule_if_due().await {
                warn!(error = %error, "Worker cleanup scheduling failed");
            }
            if let Err(error) = self.process_pending().await {
                warn!(error = %error, "Worker cleanup processing failed; durable items remain retryable");
            }
            tokio::time::sleep(LOOP_INTERVAL).await;
        }
    }

    async fn schedule_if_due(&self) -> Result<(), ProjectStoreError> {
        let policy = self.persistence.policy().await?;
        if !policy.automatic_enabled {
            return Ok(());
        }
        let now = unix_time_ms();
        let interval = policy.schedule_minutes.saturating_mul(60_000);
        let latest = self
            .persistence
            .latest_runs()
            .await?
            .runs
            .into_iter()
            .find(|run| run.trigger == WorkerCleanupRunTrigger::Scheduled);
        if latest.is_some_and(|run| now.saturating_sub(run.created_at_unix_ms) < interval) {
            return Ok(());
        }
        let bucket = now / interval.max(1);
        self.persistence
            .schedule(
                WorkerCleanupRunTrigger::Scheduled,
                StartWorkerCleanupRun {
                    command_id: format!("worker-cleanup-scheduled-{bucket}"),
                    actor: "yard-system".to_owned(),
                    preview: false,
                },
            )
            .await
    }

    async fn process_item(&self, item: &ClaimedWorkerCleanupItem) -> Result<(), ProjectStoreError> {
        if item.preview {
            return self
                .persistence
                .finish(item, WorkerCleanupItemStatus::Keep, "preview_only")
                .await;
        }
        let inventory = match self.source.inventory(&item.runtime.session).await {
            Ok(inventory) => inventory,
            Err(_) => {
                return self
                    .persistence
                    .finish(
                        item,
                        WorkerCleanupItemStatus::Review,
                        "inventory_unreachable",
                    )
                    .await;
            }
        };
        if inventory.adapter != item.runtime.adapter || inventory.session != item.runtime.session {
            return self
                .persistence
                .finish(
                    item,
                    WorkerCleanupItemStatus::Review,
                    "inventory_identity_conflict",
                )
                .await;
        }
        match observed_identity(&inventory, item) {
            ObservedIdentity::Missing => {
                return self
                    .persistence
                    .reconcile_missing(item, inventory.observed_at_unix_ms)
                    .await;
            }
            ObservedIdentity::Conflict => {
                return self
                    .persistence
                    .finish(
                        item,
                        WorkerCleanupItemStatus::Review,
                        "runtime_identity_conflict",
                    )
                    .await;
            }
            ObservedIdentity::Exact => {}
        }

        if !item.is_cleanup_advisor {
            let Some(profile_id) = item.advisor_profile_id.as_deref() else {
                return self
                    .persistence
                    .finish(item, WorkerCleanupItemStatus::Review, "advisor_unsupported")
                    .await;
            };
            let request = CleanupAdvisorRequest {
                run_id: item.run_id.clone(),
                project_id: item.project_id.clone(),
                target_worker_id: item.worker_id.clone(),
                target_assignment_id: item.assignment_id.clone(),
                parent_worker_id: item.worker_id.clone(),
                runtime_adapter: item.runtime.adapter.clone(),
                runtime_session: item.runtime.session.clone(),
                runtime_workspace_id: item.runtime.workspace_id.clone(),
                evidence_ids: vec![item.completion_receipt_id.clone()],
            };
            let result = match self.advisor.invoke(profile_id, request).await {
                Ok(result) => result,
                Err(CleanupAdvisorError::Unsupported) => {
                    return self
                        .persistence
                        .finish(item, WorkerCleanupItemStatus::Review, "advisor_unsupported")
                        .await;
                }
                Err(CleanupAdvisorError::Malformed(_)) => {
                    return self
                        .persistence
                        .finish(item, WorkerCleanupItemStatus::Review, "advisor_malformed")
                        .await;
                }
                Err(CleanupAdvisorError::Transient(message)) => {
                    if item.attempts >= MAX_ADVISOR_ATTEMPTS {
                        return self
                            .persistence
                            .finish(
                                item,
                                WorkerCleanupItemStatus::Review,
                                "advisor_retry_exhausted",
                            )
                            .await;
                    }
                    return self
                        .persistence
                        .retry(item, &message, retry_delay(item.attempts))
                        .await;
                }
            };
            let artifact = match result.artifact.clone().normalize() {
                Ok(artifact) if artifact.evidence_ids.contains(&item.completion_receipt_id) => {
                    artifact
                }
                _ => {
                    return self
                        .persistence
                        .finish(item, WorkerCleanupItemStatus::Review, "advisor_malformed")
                        .await;
                }
            };
            let result = CleanupAdvisorResult { artifact, ..result };
            self.persistence.record_advisor(item, &result).await?;
            match result.artifact.recommendation {
                CleanupAdvisorRecommendation::Keep => {
                    return self
                        .persistence
                        .finish(item, WorkerCleanupItemStatus::Keep, "advisor_keep")
                        .await;
                }
                CleanupAdvisorRecommendation::Review => {
                    return self
                        .persistence
                        .finish(item, WorkerCleanupItemStatus::Review, "advisor_review")
                        .await;
                }
                CleanupAdvisorRecommendation::Retire => {}
            }
        }

        if self.retirement.capability() != CleanupRetirementCapability::PaneManagementLeaseV1 {
            return self
                .persistence
                .finish(
                    item,
                    WorkerCleanupItemStatus::Review,
                    "lease_capability_unsupported",
                )
                .await;
        }
        let lease = match self
            .retirement
            .owned_management_lease(&item.runtime.session, &item.runtime.pane_id)
            .await
        {
            Ok(lease) => lease,
            Err(error) => return self.lease_review(item, error).await,
        };
        let request = CloseManagedPaneRequest {
            request_id: format!("worker-cleanup:{}:{}", item.run_id, item.worker_id),
            session: item.runtime.session.clone(),
            pane_id: item.runtime.pane_id.clone(),
            pane_instance_id: lease.pane_instance_id,
            owner_id: lease.owner_id,
            lease_token: lease.token,
        };
        match self.persistence.authorize_close(item).await {
            Ok(()) => {}
            Err(ProjectStoreError::WorkerCleanupRevalidationFailed) => {
                return self
                    .persistence
                    .finish(
                        item,
                        WorkerCleanupItemStatus::Review,
                        "authority_revalidation_failed",
                    )
                    .await;
            }
            Err(error) => return Err(error),
        }
        match self.retirement.close_if_management_leased(request).await {
            Ok(()) => {
                self.persistence
                    .finish(
                        item,
                        WorkerCleanupItemStatus::Retired,
                        "leased_close_succeeded",
                    )
                    .await
            }
            Err(error) => self.lease_review(item, error).await,
        }
    }

    async fn lease_review(
        &self,
        item: &ClaimedWorkerCleanupItem,
        error: CleanupRetirementError,
    ) -> Result<(), ProjectStoreError> {
        self.persistence
            .finish(
                item,
                WorkerCleanupItemStatus::Review,
                &format!("lease:{error}"),
            )
            .await
    }
}

struct ReadOnlyRetirement;

#[async_trait]
impl CleanupRetirement for ReadOnlyRetirement {}

enum ObservedIdentity {
    Exact,
    Missing,
    Conflict,
}

fn observed_identity(
    inventory: &yard_domain::RuntimeInventory,
    item: &ClaimedWorkerCleanupItem,
) -> ObservedIdentity {
    let mut related = false;
    for pane in &inventory.panes {
        if pane.terminal_id == item.runtime.terminal_id || pane.runtime_id == item.runtime.pane_id {
            related = true;
            if pane.terminal_id == item.runtime.terminal_id
                && pane.runtime_id == item.runtime.pane_id
                && pane.workspace_id == item.runtime.workspace_id
            {
                return ObservedIdentity::Exact;
            }
        }
    }
    if related {
        ObservedIdentity::Conflict
    } else {
        ObservedIdentity::Missing
    }
}

fn retry_delay(attempts: u32) -> u64 {
    RETRY_BASE_MS
        .saturating_mul(1_u64.checked_shl(attempts.min(16)).unwrap_or(u64::MAX))
        .min(RETRY_CAP_MS)
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(retry_delay(0), 1_000);
        assert_eq!(retry_delay(1), 2_000);
        assert_eq!(retry_delay(30), 60_000);
    }

    #[test]
    fn inventory_distinguishes_missing_from_identity_conflict() {
        let item = item();
        let mut inventory = inventory();
        inventory.workers.push(observed("pane-1"));
        assert!(matches!(
            observed_identity(&inventory, &item),
            ObservedIdentity::Missing
        ));
        inventory.panes.push(observed_pane("other-pane"));
        assert!(matches!(
            observed_identity(&inventory, &item),
            ObservedIdentity::Conflict
        ));
        inventory.panes[0].runtime_id = "pane-1".to_owned();
        assert!(matches!(
            observed_identity(&inventory, &item),
            ObservedIdentity::Exact
        ));
    }

    #[tokio::test]
    async fn close_uses_existing_exact_lease_after_store_authorization() {
        let authorized = Arc::new(AtomicBool::new(false));
        let persistence = Arc::new(RecordingPersistence {
            authorized: Arc::clone(&authorized),
            finished: Mutex::new(Vec::new()),
            reconciled: Mutex::new(Vec::new()),
            retried: Mutex::new(Vec::new()),
            recorded_advisors: Mutex::new(Vec::new()),
        });
        let retirement = Arc::new(RecordingRetirement {
            authorized,
            lease_lookups: Mutex::new(Vec::new()),
            closes: Mutex::new(Vec::new()),
        });
        let mut exact = inventory();
        exact.panes.push(observed_pane("pane-1"));
        let advisor = Arc::new(SuccessfulAdvisor {
            requests: Mutex::new(Vec::new()),
        });
        let service = WorkerCleanupService::with_components(
            Arc::new(StaticInventory(exact)),
            persistence.clone(),
            retirement.clone(),
            advisor.clone(),
        );
        let mut cleanup_item = item();
        cleanup_item.advisor_profile_id = Some("read-only-cleanup-advisor".to_owned());

        service.process_item(&cleanup_item).await.unwrap();

        let requests = advisor.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].target_worker_id, "worker-1");
        assert_eq!(requests[0].parent_worker_id, "worker-1");
        assert_eq!(requests[0].evidence_ids, ["receipt-1"]);
        drop(requests);
        assert_eq!(persistence.recorded_advisors.lock().unwrap().len(), 1);
        assert_eq!(
            retirement.lease_lookups.lock().unwrap().as_slice(),
            [("default".to_owned(), "pane-1".to_owned())]
        );
        let closes = retirement.closes.lock().unwrap();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].session, "default");
        assert_eq!(closes[0].pane_id, "pane-1");
        assert_eq!(closes[0].pane_instance_id, "instance-7");
        assert_eq!(closes[0].owner_id, "yard-cleanup-owner");
        assert_eq!(closes[0].token(), "secret-token");
        assert_eq!(
            persistence.finished.lock().unwrap().as_slice(),
            [(
                WorkerCleanupItemStatus::Retired,
                "leased_close_succeeded".to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn missing_reconciles_while_unavailable_observation_reviews() {
        let missing = Arc::new(RecordingPersistence::new());
        let mut cleanup_item = item();
        cleanup_item.is_cleanup_advisor = true;
        WorkerCleanupService::with_components(
            Arc::new(StaticInventory(inventory())),
            missing.clone(),
            Arc::new(ReadOnlyRetirement),
            Arc::new(UnsupportedCleanupAdvisor),
        )
        .process_item(&cleanup_item)
        .await
        .unwrap();
        assert_eq!(missing.reconciled.lock().unwrap().as_slice(), [10]);
        assert!(missing.finished.lock().unwrap().is_empty());

        let unavailable = Arc::new(RecordingPersistence::new());
        WorkerCleanupService::with_components(
            Arc::new(UnavailableInventory),
            unavailable.clone(),
            Arc::new(ReadOnlyRetirement),
            Arc::new(UnsupportedCleanupAdvisor),
        )
        .process_item(&cleanup_item)
        .await
        .unwrap();
        assert_eq!(
            unavailable.finished.lock().unwrap().as_slice(),
            [(
                WorkerCleanupItemStatus::Review,
                "inventory_unreachable".to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn unsupported_lease_and_malformed_advisor_review() {
        let mut exact = inventory();
        exact.panes.push(observed_pane("pane-1"));

        let unsupported = Arc::new(RecordingPersistence::new());
        let mut cleanup_item = item();
        cleanup_item.is_cleanup_advisor = true;
        WorkerCleanupService::with_components(
            Arc::new(StaticInventory(exact.clone())),
            unsupported.clone(),
            Arc::new(ReadOnlyRetirement),
            Arc::new(UnsupportedCleanupAdvisor),
        )
        .process_item(&cleanup_item)
        .await
        .unwrap();
        assert_eq!(
            unsupported.finished.lock().unwrap().as_slice(),
            [(
                WorkerCleanupItemStatus::Review,
                "lease_capability_unsupported".to_owned()
            )]
        );

        let malformed = Arc::new(RecordingPersistence::new());
        let mut advised_item = item();
        advised_item.advisor_profile_id = Some("cleanup-profile".to_owned());
        WorkerCleanupService::with_components(
            Arc::new(StaticInventory(exact)),
            malformed.clone(),
            Arc::new(ReadOnlyRetirement),
            Arc::new(MalformedAdvisor),
        )
        .process_item(&advised_item)
        .await
        .unwrap();
        assert_eq!(
            malformed.finished.lock().unwrap().as_slice(),
            [(
                WorkerCleanupItemStatus::Review,
                "advisor_malformed".to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn transient_advisor_failure_retries_deterministically() {
        let persistence = Arc::new(RecordingPersistence::new());
        let mut exact = inventory();
        exact.panes.push(observed_pane("pane-1"));
        let mut cleanup_item = item();
        cleanup_item.advisor_profile_id = Some("cleanup-profile".to_owned());
        cleanup_item.attempts = 1;
        WorkerCleanupService::with_components(
            Arc::new(StaticInventory(exact)),
            persistence.clone(),
            Arc::new(ReadOnlyRetirement),
            Arc::new(TransientAdvisor),
        )
        .process_item(&cleanup_item)
        .await
        .unwrap();
        assert_eq!(
            persistence.retried.lock().unwrap().as_slice(),
            [("advisor unavailable".to_owned(), 2_000)]
        );
        assert!(persistence.finished.lock().unwrap().is_empty());
    }

    struct StaticInventory(yard_domain::RuntimeInventory);

    #[async_trait]
    impl InventorySource for StaticInventory {
        async fn sessions(
            &self,
        ) -> Result<yard_domain::RuntimeSessions, crate::inventory_service::InventoryServiceError>
        {
            Ok(yard_domain::RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: Vec::new(),
            })
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<yard_domain::RuntimeInventory, crate::inventory_service::InventoryServiceError>
        {
            Ok(self.0.clone())
        }
    }

    struct UnavailableInventory;

    #[async_trait]
    impl InventorySource for UnavailableInventory {
        async fn sessions(
            &self,
        ) -> Result<yard_domain::RuntimeSessions, crate::inventory_service::InventoryServiceError>
        {
            unreachable!()
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<yard_domain::RuntimeInventory, crate::inventory_service::InventoryServiceError>
        {
            Err(crate::inventory_service::InventoryServiceError::Herdr(
                yard_herdr::HerdrError::SessionNotFound("default".to_owned()),
            ))
        }
    }

    struct MalformedAdvisor;

    #[async_trait]
    impl CleanupAdvisorInvoker for MalformedAdvisor {
        async fn invoke(
            &self,
            _profile_id: &str,
            _request: CleanupAdvisorRequest,
        ) -> Result<CleanupAdvisorResult, CleanupAdvisorError> {
            Ok(CleanupAdvisorResult {
                advisor_worker_id: "advisor-worker".to_owned(),
                advisor_assignment_id: "advisor-assignment".to_owned(),
                completion_receipt_id: "advisor-receipt".to_owned(),
                artifact_id: "advisor-artifact".to_owned(),
                artifact: yard_domain::CleanupAdvisorArtifact {
                    recommendation: CleanupAdvisorRecommendation::Retire,
                    evidence_ids: Vec::new(),
                    rationale: "retire".to_owned(),
                },
            })
        }
    }

    struct TransientAdvisor;

    #[async_trait]
    impl CleanupAdvisorInvoker for TransientAdvisor {
        async fn invoke(
            &self,
            _profile_id: &str,
            _request: CleanupAdvisorRequest,
        ) -> Result<CleanupAdvisorResult, CleanupAdvisorError> {
            Err(CleanupAdvisorError::Transient(
                "advisor unavailable".to_owned(),
            ))
        }
    }

    struct SuccessfulAdvisor {
        requests: Mutex<Vec<CleanupAdvisorRequest>>,
    }

    #[async_trait]
    impl CleanupAdvisorInvoker for SuccessfulAdvisor {
        async fn invoke(
            &self,
            profile_id: &str,
            request: CleanupAdvisorRequest,
        ) -> Result<CleanupAdvisorResult, CleanupAdvisorError> {
            assert_eq!(profile_id, "read-only-cleanup-advisor");
            self.requests.lock().unwrap().push(request);
            Ok(CleanupAdvisorResult {
                advisor_worker_id: "advisor-worker".to_owned(),
                advisor_assignment_id: "advisor-assignment".to_owned(),
                completion_receipt_id: "advisor-receipt".to_owned(),
                artifact_id: "advisor-artifact".to_owned(),
                artifact: yard_domain::CleanupAdvisorArtifact {
                    recommendation: CleanupAdvisorRecommendation::Retire,
                    evidence_ids: vec!["receipt-1".to_owned()],
                    rationale: "The completed worker is eligible.".to_owned(),
                },
            })
        }
    }

    struct RecordingPersistence {
        authorized: Arc<AtomicBool>,
        finished: Mutex<Vec<(WorkerCleanupItemStatus, String)>>,
        reconciled: Mutex<Vec<u64>>,
        retried: Mutex<Vec<(String, u64)>>,
        recorded_advisors: Mutex<Vec<CleanupAdvisorResult>>,
    }

    impl RecordingPersistence {
        fn new() -> Self {
            Self {
                authorized: Arc::new(AtomicBool::new(false)),
                finished: Mutex::new(Vec::new()),
                reconciled: Mutex::new(Vec::new()),
                retried: Mutex::new(Vec::new()),
                recorded_advisors: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl WorkerCleanupPersistence for RecordingPersistence {
        async fn policy(&self) -> Result<WorkerCleanupPolicy, ProjectStoreError> {
            unreachable!()
        }

        async fn schedule(
            &self,
            _trigger: WorkerCleanupRunTrigger,
            _command: StartWorkerCleanupRun,
        ) -> Result<(), ProjectStoreError> {
            unreachable!()
        }

        async fn latest_runs(&self) -> Result<yard_domain::WorkerCleanupRuns, ProjectStoreError> {
            unreachable!()
        }

        async fn claim(&self) -> Result<Vec<ClaimedWorkerCleanupItem>, ProjectStoreError> {
            unreachable!()
        }

        async fn finish(
            &self,
            _item: &ClaimedWorkerCleanupItem,
            status: WorkerCleanupItemStatus,
            reason: &str,
        ) -> Result<(), ProjectStoreError> {
            self.finished
                .lock()
                .unwrap()
                .push((status, reason.to_owned()));
            Ok(())
        }

        async fn retry(
            &self,
            _item: &ClaimedWorkerCleanupItem,
            reason: &str,
            retry_after_ms: u64,
        ) -> Result<(), ProjectStoreError> {
            self.retried
                .lock()
                .unwrap()
                .push((reason.to_owned(), retry_after_ms));
            Ok(())
        }

        async fn reconcile_missing(
            &self,
            _item: &ClaimedWorkerCleanupItem,
            observed_at_unix_ms: u64,
        ) -> Result<(), ProjectStoreError> {
            self.reconciled.lock().unwrap().push(observed_at_unix_ms);
            Ok(())
        }

        async fn authorize_close(
            &self,
            _item: &ClaimedWorkerCleanupItem,
        ) -> Result<(), ProjectStoreError> {
            self.authorized.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn record_advisor(
            &self,
            _item: &ClaimedWorkerCleanupItem,
            result: &CleanupAdvisorResult,
        ) -> Result<(), ProjectStoreError> {
            self.recorded_advisors.lock().unwrap().push(result.clone());
            Ok(())
        }
    }

    struct RecordingRetirement {
        authorized: Arc<AtomicBool>,
        lease_lookups: Mutex<Vec<(String, String)>>,
        closes: Mutex<Vec<CloseManagedPaneRequest>>,
    }

    #[async_trait]
    impl CleanupRetirement for RecordingRetirement {
        fn capability(&self) -> CleanupRetirementCapability {
            CleanupRetirementCapability::PaneManagementLeaseV1
        }

        async fn owned_management_lease(
            &self,
            session: &str,
            pane_id: &str,
        ) -> Result<crate::cleanup_retirement::OwnedManagementLease, CleanupRetirementError>
        {
            self.lease_lookups
                .lock()
                .unwrap()
                .push((session.to_owned(), pane_id.to_owned()));
            Ok(crate::cleanup_retirement::OwnedManagementLease {
                pane_instance_id: "instance-7".to_owned(),
                owner_id: "yard-cleanup-owner".to_owned(),
                token: crate::cleanup_retirement::ManagementLeaseToken::new(
                    "secret-token".to_owned(),
                ),
            })
        }

        async fn close_if_management_leased(
            &self,
            request: CloseManagedPaneRequest,
        ) -> Result<(), CleanupRetirementError> {
            assert!(self.authorized.load(Ordering::SeqCst));
            self.closes.lock().unwrap().push(request);
            Ok(())
        }
    }

    fn item() -> ClaimedWorkerCleanupItem {
        ClaimedWorkerCleanupItem {
            run_id: "run-1".to_owned(),
            worker_id: "worker-1".to_owned(),
            project_id: "project-1".to_owned(),
            assignment_id: "assignment-1".to_owned(),
            completion_receipt_id: "receipt-1".to_owned(),
            expected_worker_version: 1,
            expected_runtime_version: 1,
            runtime: yard_domain::WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "default".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                terminal_id: "terminal-1".to_owned(),
                tab_id: Some("tab-1".to_owned()),
                pane_id: "pane-1".to_owned(),
                provider_session: None,
                owns_tab: true,
                observation_state: yard_domain::RuntimeObservationState::Observed,
                process_state: yard_domain::RuntimeProcessState::Running,
                status: yard_domain::ObservedStatus::Done,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 1,
            },
            preview: false,
            advisor_profile_id: None,
            advisor_assignment_id: None,
            is_cleanup_advisor: false,
            claim_token: "claim-1".to_owned(),
            attempts: 0,
        }
    }

    fn inventory() -> yard_domain::RuntimeInventory {
        yard_domain::RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            runtime_version: "test".to_owned(),
            protocol: 1,
            observed_at_unix_ms: 10,
            focus: yard_domain::FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            workers: Vec::new(),
            child_agents: Vec::new(),
        }
    }

    fn observed(pane_id: &str) -> yard_domain::ObservedWorker {
        yard_domain::ObservedWorker {
            runtime_id: pane_id.to_owned(),
            pane_instance_id: None,
            terminal_id: "terminal-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            tab_id: "tab-1".to_owned(),
            pane_id: pane_id.to_owned(),
            name: None,
            provider: None,
            display_provider: None,
            status: yard_domain::ObservedStatus::Done,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 1,
            cwd: None,
            foreground_cwd: None,
            tokens: std::collections::BTreeMap::new(),
            provider_session: None,
            revision: 1,
        }
    }

    fn observed_pane(pane_id: &str) -> yard_domain::PaneObservation {
        yard_domain::PaneObservation {
            runtime_id: pane_id.to_owned(),
            pane_instance_id: None,
            terminal_id: "terminal-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            tab_id: "tab-1".to_owned(),
            focused: false,
            cwd: None,
            foreground_cwd: None,
            label: None,
            provider: None,
            display_provider: None,
            status: yard_domain::ObservedStatus::Done,
            tokens: std::collections::BTreeMap::new(),
            provider_session: None,
            revision: 1,
        }
    }
}

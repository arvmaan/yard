use std::{sync::Arc, time::Duration};

use tracing::{debug, warn};
use yard_store::{PendingRuntimeCleanup, ProjectStoreError, YardStore};

use crate::allocation_service::{RuntimeControl, RuntimeRetirementRequest};

pub const RUNTIME_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);
const RUNTIME_CLEANUP_BATCH_SIZE: usize = 100;
const RUNTIME_CLEANUP_CLAIM_TTL_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeCleanupReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
}

#[derive(Clone)]
pub struct RuntimeCleanupService {
    runtime: Arc<dyn RuntimeControl>,
    store: Arc<dyn YardStore>,
}

impl RuntimeCleanupService {
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeControl>, store: Arc<dyn YardStore>) -> Self {
        Self { runtime, store }
    }

    /// Attempt all due cleanup jobs created by one committed command.
    ///
    /// Runtime failures are persisted for retry and returned in the report. A
    /// storage failure is returned because retry durability could not be
    /// confirmed.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when pending jobs cannot be loaded or their
    /// success/failure result cannot be persisted.
    pub async fn process_command(
        &self,
        command_id: &str,
    ) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        self.process(Some(command_id)).await
    }

    /// Attempt one bounded batch of all due runtime cleanup jobs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError`] when pending jobs cannot be loaded or their
    /// success/failure result cannot be persisted.
    pub async fn process_pending(&self) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        self.process(None).await
    }

    /// Retry durable cleanup work forever. Each pass is independent so a store
    /// or runtime outage cannot terminate future retirement attempts.
    pub async fn run(self) {
        loop {
            match self.process_pending().await {
                Ok(report) if report.attempted > 0 => {
                    debug!(
                        attempted = report.attempted,
                        succeeded = report.succeeded,
                        failed = report.failed,
                        "Processed displaced Herdr runtime cleanup jobs"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        error = %error,
                        "Runtime cleanup pass failed; durable jobs will be retried"
                    );
                }
            }
            tokio::time::sleep(RUNTIME_CLEANUP_INTERVAL).await;
        }
    }

    async fn process(
        &self,
        command_id: Option<&str>,
    ) -> Result<RuntimeCleanupReport, ProjectStoreError> {
        let batch = self
            .store
            .claim_pending_runtime_cleanups(
                command_id,
                RUNTIME_CLEANUP_BATCH_SIZE,
                RUNTIME_CLEANUP_CLAIM_TTL_MS,
            )
            .await?;
        let mut report = RuntimeCleanupReport {
            attempted: batch.jobs.len() + batch.rejected,
            failed: batch.rejected,
            ..RuntimeCleanupReport::default()
        };
        for job in batch.jobs {
            match self.runtime.retire_runtime(retirement_request(&job)).await {
                Ok(()) => {
                    self.store
                        .succeed_runtime_cleanup(&job.id, &job.claim_token)
                        .await?;
                    report.succeeded += 1;
                }
                Err(error) => {
                    self.store
                        .fail_runtime_cleanup(&job.id, &job.claim_token, &error.to_string())
                        .await?;
                    report.failed += 1;
                    warn!(
                        cleanup_id = %job.id,
                        command_id = %job.command_id,
                        worker_id = %job.worker_id,
                        reason = %job.reason,
                        error = %error,
                        "Destructive runtime retirement failed; cleanup remains pending"
                    );
                }
            }
        }
        Ok(report)
    }
}

fn retirement_request(job: &PendingRuntimeCleanup) -> RuntimeRetirementRequest {
    RuntimeRetirementRequest {
        cleanup_id: job.id.clone(),
        adapter: job.adapter.clone(),
        session: job.session.clone(),
        workspace_id: job.workspace_id.clone(),
        terminal_id: job.terminal_id.clone(),
        tab_id: job.tab_id.clone(),
        pane_id: job.pane_id.clone(),
        provider_session: job.provider_session.clone(),
        owns_tab: job.owns_tab,
        require_identity_match: true,
    }
}

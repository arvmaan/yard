use std::sync::Arc;

use thiserror::Error;
use tracing::warn;
use yard_domain::{DeleteWorker, DeletedWorker, EndWorkerSession, EndedWorkerSession};
use yard_store::{ProjectStoreError, YardStore};

use crate::{allocation_service::RuntimeControl, runtime_cleanup_service::RuntimeCleanupService};

#[derive(Clone)]
pub struct WorkerSessionService {
    store: Arc<dyn YardStore>,
    cleanup: RuntimeCleanupService,
}

impl WorkerSessionService {
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeControl>, store: Arc<dyn YardStore>) -> Self {
        Self {
            cleanup: RuntimeCleanupService::new(runtime, Arc::clone(&store)),
            store,
        }
    }

    /// Commit a worker's ended disposition and immediately attempt its durable
    /// runtime cleanup. A cleanup transport failure remains queued for the
    /// background retry loop and does not roll back the committed disposition.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerSessionServiceError`] when validation or the atomic
    /// durable state transition fails.
    pub async fn end(
        &self,
        worker_id: &str,
        command: EndWorkerSession,
    ) -> Result<EndedWorkerSession, WorkerSessionServiceError> {
        let command_id = command.command_id.clone();
        let mut ended = self.store.end_worker_session(worker_id, command).await?;
        if !ended.cleanup_pending {
            return Ok(ended);
        }

        match self.cleanup.process_command(&command_id).await {
            Ok(report) if report.attempted > 0 => {
                ended.cleanup_pending = report.failed > 0;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    command_id,
                    worker_id,
                    error = %error,
                    "Immediate worker runtime cleanup failed; durable retry remains pending"
                );
            }
        }
        Ok(ended)
    }

    /// Permanently remove an ended worker from normal Yard UI queries while
    /// retaining durable audit and cleanup records.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerSessionServiceError`] when the session is not ended,
    /// was already deleted, is stale, or persistence fails.
    pub async fn delete(
        &self,
        worker_id: &str,
        command: DeleteWorker,
    ) -> Result<DeletedWorker, WorkerSessionServiceError> {
        self.store
            .delete_worker(worker_id, command)
            .await
            .map_err(Into::into)
    }
}

#[derive(Debug, Error)]
pub enum WorkerSessionServiceError {
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

//! Complete or cancel an assignment in one durable command.
//!
//! The store commits the receipt or cancellation, the lifecycle transition,
//! the optional session end, and the transcript capture job in one
//! transaction. Afterwards the capture runs once inline and then runtime
//! cleanup is attempted; both are best-effort and never fail the response,
//! because their durable jobs keep retrying in the background.

use std::sync::Arc;

use thiserror::Error;
use tracing::warn;
use yard_domain::{DisposeAssignment, DisposedAssignment, RequestOrigin};
use yard_store::{ProjectStoreError, YardStore};

use crate::allocation_service::RuntimeControl;
use crate::intervention_service::RuntimeIntervention;
use crate::inventory_service::InventorySource;
use crate::runtime_cleanup_service::RuntimeCleanupService;
use crate::transcript_capture_service::TranscriptCaptureService;

#[derive(Clone)]
pub struct AssignmentDispositionService {
    store: Arc<dyn YardStore>,
    cleanup: RuntimeCleanupService,
    transcripts: TranscriptCaptureService,
}

impl AssignmentDispositionService {
    #[must_use]
    pub fn new(
        source: Arc<dyn InventorySource>,
        runtime: Arc<dyn RuntimeControl>,
        intervention: Arc<dyn RuntimeIntervention>,
        store: Arc<dyn YardStore>,
    ) -> Self {
        Self {
            cleanup: RuntimeCleanupService::new(runtime, Arc::clone(&store)),
            transcripts: TranscriptCaptureService::new(source, intervention, Arc::clone(&store)),
            store,
        }
    }

    /// Commit one disposition, then attempt its transcript capture and
    /// runtime cleanup without letting either fail the committed result.
    ///
    /// # Errors
    ///
    /// Returns [`AssignmentDispositionServiceError`] when the command is
    /// invalid, stale, conflicting, or cannot be committed.
    pub async fn dispose(
        &self,
        project_id: &str,
        assignment_id: &str,
        command: DisposeAssignment,
        request_origin: RequestOrigin,
    ) -> Result<DisposedAssignment, AssignmentDispositionServiceError> {
        let mut disposed = self
            .store
            .dispose_assignment(project_id, assignment_id, command, request_origin)
            .await?;
        if disposed.transcript_pending {
            // The transcript must be read before cleanup can observe the
            // runtime closed, so capture runs first.
            match self.transcripts.process_command(&disposed.command_id).await {
                Ok(report) if report.attempted > 0 => {
                    disposed.transcript_pending = report.retrying > 0;
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        command_id = %disposed.command_id,
                        error = %error,
                        "Immediate transcript capture failed; the durable job will retry"
                    );
                }
            }
        }
        if disposed.cleanup_pending {
            match self.cleanup.process_command(&disposed.command_id).await {
                Ok(report) if report.attempted > 0 => {
                    disposed.cleanup_pending = report.failed > 0;
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        command_id = %disposed.command_id,
                        error = %error,
                        "Immediate runtime cleanup failed; the durable job will retry"
                    );
                }
            }
        }
        Ok(disposed)
    }
}

#[derive(Debug, Error)]
pub enum AssignmentDispositionServiceError {
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

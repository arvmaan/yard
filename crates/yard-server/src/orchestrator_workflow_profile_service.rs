use std::sync::Arc;

use thiserror::Error;
use yard_domain::{
    OrchestratorWorkflowProfile, ResetOrchestratorWorkflowProfile,
    UpdateOrchestratorWorkflowProfile,
};
use yard_store::{ProjectStoreError, YardStore};

#[derive(Clone)]
pub struct OrchestratorWorkflowProfileService {
    store: Arc<dyn YardStore>,
}

impl OrchestratorWorkflowProfileService {
    #[must_use]
    pub fn new(store: Arc<dyn YardStore>) -> Self {
        Self { store }
    }

    /// Read the active workflow profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileServiceError`] when storage is unavailable.
    pub async fn get(
        &self,
    ) -> Result<OrchestratorWorkflowProfile, OrchestratorWorkflowProfileServiceError> {
        self.store
            .get_orchestrator_workflow_profile()
            .await
            .map_err(Into::into)
    }

    /// Append and activate an edited workflow profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileServiceError`] for invalid input,
    /// a stale expected version, or storage failure.
    pub async fn update(
        &self,
        command: UpdateOrchestratorWorkflowProfile,
    ) -> Result<OrchestratorWorkflowProfile, OrchestratorWorkflowProfileServiceError> {
        let command = command.normalize()?;
        self.store
            .update_orchestrator_workflow_profile(command)
            .await
            .map_err(Into::into)
    }

    /// Append and activate a factory workflow profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileServiceError`] for invalid input,
    /// a stale expected version, or storage failure.
    pub async fn reset(
        &self,
        command: ResetOrchestratorWorkflowProfile,
    ) -> Result<OrchestratorWorkflowProfile, OrchestratorWorkflowProfileServiceError> {
        let command = command.normalize()?;
        self.store
            .reset_orchestrator_workflow_profile(command)
            .await
            .map_err(Into::into)
    }
}

#[derive(Debug, Error)]
pub enum OrchestratorWorkflowProfileServiceError {
    #[error(transparent)]
    InvalidProfile(#[from] yard_domain::OrchestratorWorkflowProfileValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

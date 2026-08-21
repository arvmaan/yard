use std::sync::Arc;

use thiserror::Error;
use yard_domain::{
    OrchestratorWorkflowProfile, OrchestratorWorkflowProfiles, ResetOrchestratorWorkflowProfile,
    UpdateOrchestratorWorkflowProfile, YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
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

    /// List active revisions in the workflow-profile catalog.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileServiceError`] when storage is unavailable.
    pub async fn list(
        &self,
    ) -> Result<OrchestratorWorkflowProfiles, OrchestratorWorkflowProfileServiceError> {
        self.store
            .list_orchestrator_workflow_profiles()
            .await
            .map_err(Into::into)
    }

    /// Read one active workflow profile by stable identity.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileServiceError`] when the profile does
    /// not exist or storage is unavailable.
    pub async fn get_by_id(
        &self,
        profile_id: &str,
    ) -> Result<OrchestratorWorkflowProfile, OrchestratorWorkflowProfileServiceError> {
        self.store
            .get_orchestrator_workflow_profile_by_id(profile_id)
            .await
            .map_err(Into::into)
    }

    /// Read one immutable workflow profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`OrchestratorWorkflowProfileServiceError`] when the revision
    /// does not exist or storage is unavailable.
    pub async fn get_revision(
        &self,
        profile_id: &str,
        version: u64,
    ) -> Result<OrchestratorWorkflowProfile, OrchestratorWorkflowProfileServiceError> {
        self.store
            .get_orchestrator_workflow_profile_revision(profile_id, version)
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
            .update_orchestrator_workflow_profile(YARD_STANDARD_ORCHESTRATOR_PROFILE_ID, command)
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
            .reset_orchestrator_workflow_profile(YARD_STANDARD_ORCHESTRATOR_PROFILE_ID, command)
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

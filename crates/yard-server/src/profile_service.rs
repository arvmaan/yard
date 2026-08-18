use std::sync::Arc;

use thiserror::Error;
use yard_domain::{
    AgentProfile, AgentProfiles, CreateAgentProfile, CreateWorkerProfile, UpdateAgentProfile,
    UpdateWorkerProfile, WorkerProfile, WorkerProfiles,
};
use yard_store::{ProjectStoreError, YardStore};

#[derive(Clone)]
pub struct ProfileService {
    store: Arc<dyn YardStore>,
}

impl ProfileService {
    #[must_use]
    pub fn new(store: Arc<dyn YardStore>) -> Self {
        Self { store }
    }

    /// List current worker-profile revisions.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileServiceError`] when storage cannot be read.
    pub async fn list(&self) -> Result<WorkerProfiles, ProfileServiceError> {
        self.store.list_worker_profiles().await.map_err(Into::into)
    }

    /// Get one current worker-profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileServiceError`] when the profile is missing or storage
    /// cannot be read.
    pub async fn get(&self, profile_id: &str) -> Result<WorkerProfile, ProfileServiceError> {
        self.store
            .get_worker_profile(profile_id)
            .await
            .map_err(Into::into)
    }

    /// Create a reusable worker profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileServiceError`] when validation or persistence fails.
    pub async fn create(
        &self,
        profile: CreateWorkerProfile,
    ) -> Result<WorkerProfile, ProfileServiceError> {
        let profile = profile.normalize()?;
        self.store
            .create_worker_profile(profile)
            .await
            .map_err(Into::into)
    }

    /// Replace a profile by appending an immutable revision.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileServiceError`] for invalid input, a stale profile
    /// version, a missing profile, or another persistence failure.
    pub async fn update(
        &self,
        profile_id: &str,
        update: UpdateWorkerProfile,
    ) -> Result<WorkerProfile, ProfileServiceError> {
        let update = update.normalize()?;
        self.store
            .update_worker_profile(profile_id, update)
            .await
            .map_err(Into::into)
    }

    /// List current portable agent-profile revisions.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileServiceError`] when storage cannot be read.
    pub async fn list_agents(&self) -> Result<AgentProfiles, AgentProfileServiceError> {
        self.store.list_agent_profiles().await.map_err(Into::into)
    }

    /// Get the current portable revision for one profile identity.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileServiceError`] when the profile is missing or
    /// storage cannot be read.
    pub async fn get_agent(
        &self,
        profile_id: &str,
    ) -> Result<AgentProfile, AgentProfileServiceError> {
        self.store
            .get_agent_profile(profile_id)
            .await
            .map_err(Into::into)
    }

    /// Export one exact immutable portable profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileServiceError`] when the revision is missing or
    /// storage cannot be read.
    pub async fn get_agent_revision(
        &self,
        profile_id: &str,
        profile_version: u64,
    ) -> Result<AgentProfile, AgentProfileServiceError> {
        self.store
            .get_agent_profile_revision(profile_id, profile_version)
            .await
            .map_err(Into::into)
    }

    /// Import a portable profile and create its `WorkerProfile` compatibility row.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileServiceError`] when validation, negotiation, or
    /// persistence fails.
    pub async fn create_agent(
        &self,
        profile: CreateAgentProfile,
    ) -> Result<AgentProfile, AgentProfileServiceError> {
        profile.clone().prepare()?;
        self.store
            .create_agent_profile(profile)
            .await
            .map_err(Into::into)
    }

    /// Import a replacement manifest as the next immutable profile revision.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileServiceError`] for invalid input, unsupported
    /// required capabilities, stale revisions, or persistence failures.
    pub async fn update_agent(
        &self,
        profile_id: &str,
        update: UpdateAgentProfile,
    ) -> Result<AgentProfile, AgentProfileServiceError> {
        update.clone().prepare()?;
        self.store
            .update_agent_profile(profile_id, update)
            .await
            .map_err(Into::into)
    }
}

#[derive(Debug, Error)]
pub enum ProfileServiceError {
    #[error(transparent)]
    InvalidProfile(#[from] yard_domain::ProfileValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

#[derive(Debug, Error)]
pub enum AgentProfileServiceError {
    #[error(transparent)]
    InvalidProfile(#[from] yard_domain::AgentProfileValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

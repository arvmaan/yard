use std::sync::Arc;

use thiserror::Error;
use yard_domain::{CreateWorkerProfile, UpdateWorkerProfile, WorkerProfile, WorkerProfiles};
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
}

#[derive(Debug, Error)]
pub enum ProfileServiceError {
    #[error(transparent)]
    InvalidProfile(#[from] yard_domain::ProfileValidationError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
}

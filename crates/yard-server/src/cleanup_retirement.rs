use async_trait::async_trait;
use thiserror::Error;

#[derive(Clone, PartialEq, Eq)]
pub struct ManagementLeaseToken(String);

impl ManagementLeaseToken {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg(test)]
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ManagementLeaseToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ManagementLeaseToken([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupRetirementCapability {
    ReadOnly,
    PaneManagementLeaseV1,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CloseManagedPaneRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub lease_token: ManagementLeaseToken,
}

impl std::fmt::Debug for CloseManagedPaneRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CloseManagedPaneRequest")
            .field("request_id", &self.request_id)
            .field("session", &self.session)
            .field("pane_id", &self.pane_id)
            .field("pane_instance_id", &self.pane_instance_id)
            .field("owner_id", &self.owner_id)
            .field("lease_token", &"[REDACTED]")
            .finish()
    }
}

impl CloseManagedPaneRequest {
    #[cfg(test)]
    pub(crate) fn token(&self) -> &str {
        self.lease_token.expose()
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CleanupRetirementError {
    #[error("pane management lease capability is unavailable")]
    UnsupportedCapability,
    #[error("pane was not found")]
    PaneNotFound,
    #[error("pane instance changed")]
    PaneInstanceMismatch,
    #[error("another unexpired management lease exists")]
    LeaseConflict,
    #[error("management lease was not found")]
    LeaseNotFound,
    #[error("management lease expired")]
    LeaseExpired,
    #[error("management lease token is invalid")]
    LeaseTokenInvalid,
    #[error("management lease owner does not match")]
    LeaseOwnerMismatch,
    #[error("management lease duration is invalid")]
    LeaseDurationInvalid,
    #[error("request replay does not exactly match the original request")]
    RequestReplayMismatch,
    #[error("runtime retirement failed")]
    Runtime,
}

impl CleanupRetirementError {
    #[must_use]
    pub const fn retryable_by_cleanup(self) -> bool {
        false
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct OwnedManagementLease {
    pub pane_instance_id: String,
    pub owner_id: String,
    pub token: ManagementLeaseToken,
}

impl std::fmt::Debug for OwnedManagementLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedManagementLease")
            .field("pane_instance_id", &self.pane_instance_id)
            .field("owner_id", &self.owner_id)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
pub trait CleanupRetirement: Send + Sync {
    fn capability(&self) -> CleanupRetirementCapability {
        CleanupRetirementCapability::ReadOnly
    }

    /// Returns an already-held management lease; implementations must not acquire one here.
    async fn owned_management_lease(
        &self,
        _session: &str,
        _pane_id: &str,
    ) -> Result<OwnedManagementLease, CleanupRetirementError> {
        Err(CleanupRetirementError::UnsupportedCapability)
    }

    async fn close_if_management_leased(
        &self,
        _request: CloseManagedPaneRequest,
    ) -> Result<(), CleanupRetirementError> {
        Err(CleanupRetirementError::UnsupportedCapability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_and_typed_failures_never_retry() {
        let request = CloseManagedPaneRequest {
            request_id: "close-1".to_owned(),
            session: "default".to_owned(),
            pane_id: "pane-1".to_owned(),
            pane_instance_id: "instance-7".to_owned(),
            owner_id: "yard-worker-1".to_owned(),
            lease_token: ManagementLeaseToken::new("secret-token".to_owned()),
        };

        assert!(!format!("{request:?}").contains("secret-token"));
        for error in [
            CleanupRetirementError::PaneNotFound,
            CleanupRetirementError::PaneInstanceMismatch,
            CleanupRetirementError::LeaseConflict,
            CleanupRetirementError::LeaseNotFound,
            CleanupRetirementError::LeaseExpired,
            CleanupRetirementError::LeaseTokenInvalid,
            CleanupRetirementError::LeaseOwnerMismatch,
            CleanupRetirementError::RequestReplayMismatch,
        ] {
            assert!(!error.retryable_by_cleanup());
        }
    }
}

//! Read-only storage scan results (preview only).
//!
//! These types describe reclaimable generated output under the configured
//! storage roots. "Storage" is used instead of "artifact", which already names
//! completion-receipt artifacts. Nothing here deletes anything: a scan only
//! reports candidates, their sizes, and whether a later cleanup could treat
//! them as `safe`, `review`, or `blocked`.

use serde::{Deserialize, Serialize};

/// Lifecycle of the single in-memory storage scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageScanStatus {
    /// `YARD_STORAGE_ROOTS` is unset, so nothing is scanned.
    NotConfigured,
    Running,
    Completed,
    Failed,
}

/// Recognized cleanup class. Each generated class needs an exact directory
/// name plus a sibling marker; `unknown` is report-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    RustTarget,
    NodeModules,
    ViteDist,
    GradleOutput,
    WorkspaceBuild,
    WorkspaceEnv,
    WorkspaceLogs,
    GitWorktree,
    Unknown,
}

impl StorageClass {
    /// Whether the class is generated output that a raw no-follow removal
    /// could reclaim (as opposed to a Git worktree or unknown data).
    #[must_use]
    pub const fn is_generated(self) -> bool {
        !matches!(self, Self::GitWorktree | Self::Unknown)
    }

    /// Whether the class lives at a marked workspace root.
    #[must_use]
    pub const fn is_workspace(self) -> bool {
        matches!(
            self,
            Self::WorkspaceBuild | Self::WorkspaceEnv | Self::WorkspaceLogs
        )
    }
}

/// Safety state of a candidate. Only `safe` may ever be preselected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageSafety {
    Safe,
    Review,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOwnerKind {
    Project,
    CoordinationNode,
}

/// The project or workstream whose recorded path contains the candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageOwner {
    pub kind: StorageOwnerKind,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageSourceStatus {
    Clean,
    NotClean,
    Unknown,
}

/// One read-only finding about a nested workspace package repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoragePackageIssue {
    UncommittedChanges,
    UntrackedFiles,
    UnpushedCommits,
    NoUpstream,
    UpstreamGone,
    Stash,
    DetachedHead,
    NotAGitRepository,
    DubiousOwnership,
    ProbeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePackageState {
    pub name: String,
    pub path: String,
    pub issues: Vec<StoragePackageIssue>,
    /// Commits ahead of the upstream, when an upstream exists.
    pub ahead: Option<u64>,
}

/// Source state of a marked workspace, derived from its `src/<Pkg>/.git`
/// repositories. Anything but `clean` keeps `build/` and `env/` in `review`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSourceState {
    pub status: StorageSourceStatus,
    pub package_count: u64,
    /// Human-readable lines, for example "1 package has unpushed commits".
    pub summary: Vec<String>,
    /// Only packages with at least one issue.
    pub packages: Vec<StoragePackageState>,
}

/// Git details of a registered linked worktree candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageWorktree {
    pub repository_path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageCandidate {
    /// Server-issued identifier; the browser never sends paths.
    pub id: String,
    /// Canonical path.
    pub path: String,
    pub class: StorageClass,
    /// Allocated bytes of files with a single link, excluding nested
    /// candidates, so every byte is counted once across the scan.
    pub bytes: u64,
    /// Allocated bytes of hard-linked files with a link outside this
    /// candidate (not reclaimable), each inode counted once.
    pub shared_bytes: u64,
    /// Bytes of nested candidates (only a worktree can contain candidates).
    pub nested_bytes: u64,
    pub file_count: u64,
    pub newest_mtime_unix_ms: Option<u64>,
    pub owner: Option<StorageOwner>,
    /// Worktrees only: whether the path equals a checkout Yard recorded.
    pub known_to_yard: Option<bool>,
    pub safety: StorageSafety,
    pub reasons: Vec<String>,
    /// A confirmation a later cleanup must require for this candidate.
    pub acknowledgement: Option<String>,
    pub source_state: Option<StorageSourceState>,
    pub estimate_truncated: bool,
    /// Workspace classes: the workspace root.
    pub workspace_path: Option<String>,
    pub worktree: Option<StorageWorktree>,
    /// The candidate that contains this one, if any.
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSafetyTotal {
    pub count: u64,
    pub bytes: u64,
    pub shared_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageTotals {
    pub safe: StorageSafetyTotal,
    pub review: StorageSafetyTotal,
    pub blocked: StorageSafetyTotal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageTruncationReason {
    EntryBudget,
    TimeBudget,
    CandidateBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageInUseCheck {
    pub available: bool,
    pub reason: Option<String>,
}

/// The single in-memory scan (the last one is kept for 15 minutes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageScan {
    /// `None` only when storage is not configured.
    pub id: Option<String>,
    pub status: StorageScanStatus,
    pub roots: Vec<String>,
    pub started_at_unix_ms: Option<u64>,
    pub completed_at_unix_ms: Option<u64>,
    pub expires_at_unix_ms: Option<u64>,
    pub estimate_truncated: bool,
    pub truncation: Vec<StorageTruncationReason>,
    pub entries_scanned: u64,
    pub in_use_check: Option<StorageInUseCheck>,
    /// Scan-level observations, for example unreadable directories.
    pub notes: Vec<String>,
    pub totals: StorageTotals,
    /// Largest first. Empty until the scan completes.
    pub candidates: Vec<StorageCandidate>,
    pub error: Option<String>,
}

impl StorageScan {
    /// The response when `YARD_STORAGE_ROOTS` is unset.
    #[must_use]
    pub fn not_configured() -> Self {
        Self {
            id: None,
            status: StorageScanStatus::NotConfigured,
            roots: Vec::new(),
            started_at_unix_ms: None,
            completed_at_unix_ms: None,
            expires_at_unix_ms: None,
            estimate_truncated: false,
            truncation: Vec::new(),
            entries_scanned: 0,
            in_use_check: None,
            notes: Vec::new(),
            totals: StorageTotals::default(),
            candidates: Vec::new(),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_classes_and_states_as_snake_case() {
        assert_eq!(
            serde_json::to_value(StorageClass::WorkspaceEnv).unwrap(),
            "workspace_env"
        );
        assert_eq!(
            serde_json::to_value(StorageScanStatus::NotConfigured).unwrap(),
            "not_configured"
        );
        assert_eq!(
            serde_json::to_value(StoragePackageIssue::UnpushedCommits).unwrap(),
            "unpushed_commits"
        );
        assert!(StorageClass::WorkspaceLogs.is_workspace());
        assert!(!StorageClass::GitWorktree.is_generated());
        assert!(!StorageClass::Unknown.is_generated());
        assert!(StorageClass::RustTarget.is_generated());
    }
}

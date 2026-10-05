use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

use crate::{AssignmentLifecycle, ObservedStatus, ProviderSessionRef, WorkerDesiredState};

const MAX_PROJECT_NAME_BYTES: usize = 120;
const MAX_PROJECT_COMMAND_BYTES: usize = 120;
const MAX_PROJECT_CWD_BYTES: usize = 4_096;
const MAX_PROJECT_REPOSITORY_PATH_BYTES: usize = 4_096;
const MAX_PROJECT_OBJECTIVE_BYTES: usize = 16_000;
/// Upper bound on the active assignments one archive may end.
const MAX_EXPECTED_ACTIVE_ASSIGNMENTS: usize = 500;
const MIN_PROJECT_WIDTH: f64 = 322.0;
const MAX_PROJECT_WIDTH: f64 = 2_400.0;
const MIN_PROJECT_HEIGHT: f64 = 180.0;
const MAX_PROJECT_HEIGHT: f64 = 2_400.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CanvasPlacement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl CanvasPlacement {
    /// Validate finite coordinates and usable project-region dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when coordinates are not finite or
    /// dimensions fall outside the supported canvas bounds.
    pub fn validate(&self) -> Result<(), ProjectValidationError> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err(ProjectValidationError::InvalidPosition);
        }
        if !self.width.is_finite()
            || !self.height.is_finite()
            || !(MIN_PROJECT_WIDTH..=MAX_PROJECT_WIDTH).contains(&self.width)
            || !(MIN_PROJECT_HEIGHT..=MAX_PROJECT_HEIGHT).contains(&self.height)
        {
            return Err(ProjectValidationError::InvalidSize);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRuntimeBinding {
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
}

impl ProjectRuntimeBinding {
    /// Normalize and validate a project-to-runtime binding.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required runtime reference is
    /// empty.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.adapter = required("runtime.adapter", &self.adapter)?;
        self.session = required("runtime.session", &self.session)?;
        self.workspace_id = required("runtime.workspace_id", &self.workspace_id)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRuntimeBinding {
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub provider_session: Option<ProviderSessionRef>,
    pub owns_tab: bool,
    pub observation_state: RuntimeObservationState,
    pub process_state: RuntimeProcessState,
    pub status: ObservedStatus,
    #[serde(with = "crate::serde_u64")]
    pub state_change_sequence: u64,
    #[serde(with = "crate::serde_u64")]
    pub revision: u64,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub last_observed_at_unix_ms: u64,
}

impl WorkerRuntimeBinding {
    /// Normalize and validate replaceable runtime references for a Yard worker.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required reference is empty.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.adapter = required("worker_runtime.adapter", &self.adapter)?;
        self.session = required("worker_runtime.session", &self.session)?;
        self.workspace_id = required("worker_runtime.workspace_id", &self.workspace_id)?;
        self.terminal_id = required("worker_runtime.terminal_id", &self.terminal_id)?;
        self.tab_id = self
            .tab_id
            .map(|tab_id| required("worker_runtime.tab_id", &tab_id))
            .transpose()?;
        self.pane_id = required("worker_runtime.pane_id", &self.pane_id)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeObservationState {
    Observed,
    Missing,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProcessState {
    Running,
    Exited,
    Unknown,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOwnershipKind {
    #[default]
    External,
    YardOwned,
    SystemEphemeral,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub profile_id: Option<String>,
    #[serde(default, with = "crate::serde_u64::option")]
    pub profile_version: Option<u64>,
    #[serde(default)]
    pub ownership_kind: WorkerOwnershipKind,
    /// A user-chosen label (see [`crate::normalize_worker_display_name`]).
    /// `None` means the worker shows its default profile or agent label.
    /// Older payloads without the field still parse.
    #[serde(default)]
    pub display_name: Option<String>,
    pub desired_state: WorkerDesiredState,
    pub runtime: Option<WorkerRuntimeBinding>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectPlacement {
    pub geometry: CanvasPlacement,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectWorkflowProfilePin {
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub profile_version: u64,
    pub pinned_by: String,
    pub pinned_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub runtime: ProjectRuntimeBinding,
    pub orchestrator: Worker,
    pub placement: ProjectPlacement,
    pub workflow_profile: ProjectWorkflowProfilePin,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projects {
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRepository {
    pub id: String,
    pub project_id: String,
    pub root_path: String,
    pub git_common_dir: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRepositories {
    pub repositories: Vec<ProjectRepository>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetProjectRepository {
    pub root_path: String,
}

impl SetProjectRepository {
    /// Normalize and validate a project repository checkout root.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when the path is blank, relative, or
    /// oversized.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.root_path = bounded_required(
            "root_path",
            &self.root_path,
            MAX_PROJECT_REPOSITORY_PATH_BYTES,
        )?;
        if !Path::new(&self.root_path).is_absolute() {
            return Err(ProjectValidationError::InvalidRepositoryRoot);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveProject {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub expected_orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_worker_version: u64,
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_orchestrator_runtime_version: Option<u64>,
    /// What to do with assignments that are still allocating, active, or
    /// handing off. Old clients omit it and keep the reject behavior.
    #[serde(default)]
    pub active_work: ProjectArchiveActiveWork,
    /// The exact active assignments the caller saw in the disposition
    /// preview. Required with `active_work = cancel`, rejected otherwise.
    #[serde(default)]
    pub expected_active_assignments: Option<Vec<ExpectedActiveAssignment>>,
}

impl ArchiveProject {
    /// Normalize and validate a durable project archive command.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required identifier is blank
    /// or an optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.command_id =
            bounded_required("command_id", &self.command_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
        self.expected_orchestrator_worker_id = bounded_required(
            "expected_orchestrator_worker_id",
            &self.expected_orchestrator_worker_id,
            MAX_PROJECT_COMMAND_BYTES,
        )?;
        if self.expected_project_version == 0
            || self.expected_orchestrator_worker_version == 0
            || self.expected_orchestrator_runtime_version == Some(0)
        {
            return Err(ProjectValidationError::InvalidVersion);
        }
        self.expected_active_assignments =
            normalize_active_work(self.active_work, self.expected_active_assignments)?;
        Ok(self)
    }

    #[must_use]
    pub fn preconditions(&self) -> ProjectArchivePreconditions {
        ProjectArchivePreconditions {
            expected_project_version: self.expected_project_version,
            expected_orchestrator_worker_id: self.expected_orchestrator_worker_id.clone(),
            expected_orchestrator_worker_version: self.expected_orchestrator_worker_version,
            expected_orchestrator_runtime_version: self.expected_orchestrator_runtime_version,
            active_work: self.active_work,
            expected_active_assignments: self.expected_active_assignments.clone(),
        }
    }
}

/// How an archive treats assignments that are still allocating, active, or
/// handing off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectArchiveActiveWork {
    /// Refuse the archive while any such assignment exists.
    #[default]
    Reject,
    /// Record exactly the previewed assignments as cancelled (never
    /// completed) and end their worker sessions in the archive transaction.
    Cancel,
}

impl ProjectArchiveActiveWork {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Cancel => "cancel",
        }
    }
}

/// One active assignment, at the version the caller saw, that an archive
/// with `active_work = cancel` may end.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExpectedActiveAssignment {
    pub assignment_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_assignment_version: u64,
}

/// Validate the active-work choice and put the expected set in canonical
/// (sorted) order, so replay compares sets rather than request order.
fn normalize_active_work(
    active_work: ProjectArchiveActiveWork,
    expected: Option<Vec<ExpectedActiveAssignment>>,
) -> Result<Option<Vec<ExpectedActiveAssignment>>, ProjectValidationError> {
    match (active_work, expected) {
        (ProjectArchiveActiveWork::Reject, None) => Ok(None),
        (ProjectArchiveActiveWork::Reject, Some(_)) => {
            Err(ProjectValidationError::UnexpectedActiveAssignments)
        }
        (ProjectArchiveActiveWork::Cancel, None) => Err(ProjectValidationError::Required(
            "expected_active_assignments",
        )),
        (ProjectArchiveActiveWork::Cancel, Some(expected)) => {
            if expected.len() > MAX_EXPECTED_ACTIVE_ASSIGNMENTS {
                return Err(ProjectValidationError::TooManyExpectedAssignments {
                    max: MAX_EXPECTED_ACTIVE_ASSIGNMENTS,
                });
            }
            let mut normalized = expected
                .into_iter()
                .map(|assignment| {
                    if assignment.expected_assignment_version == 0 {
                        return Err(ProjectValidationError::InvalidVersion);
                    }
                    Ok(ExpectedActiveAssignment {
                        assignment_id: bounded_required(
                            "expected_active_assignments.assignment_id",
                            &assignment.assignment_id,
                            MAX_PROJECT_COMMAND_BYTES,
                        )?,
                        expected_assignment_version: assignment.expected_assignment_version,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            normalized.sort();
            if normalized
                .windows(2)
                .any(|pair| pair[0].assignment_id == pair[1].assignment_id)
            {
                return Err(ProjectValidationError::DuplicateExpectedAssignment);
            }
            Ok(Some(normalized))
        }
    }
}

/// The optimistic versions a caller saw before archiving a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectArchivePreconditions {
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub expected_orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_worker_version: u64,
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_orchestrator_runtime_version: Option<u64>,
    #[serde(default)]
    pub active_work: ProjectArchiveActiveWork,
    #[serde(default)]
    pub expected_active_assignments: Option<Vec<ExpectedActiveAssignment>>,
}

impl ProjectArchivePreconditions {
    /// Normalize and validate archive preconditions.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when the orchestrator identifier is
    /// blank or oversized, an optimistic version is zero, or the expected
    /// active assignments do not fit the active-work choice.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.expected_orchestrator_worker_id = bounded_required(
            "expected_orchestrator_worker_id",
            &self.expected_orchestrator_worker_id,
            MAX_PROJECT_COMMAND_BYTES,
        )?;
        if self.expected_project_version == 0
            || self.expected_orchestrator_worker_version == 0
            || self.expected_orchestrator_runtime_version == Some(0)
        {
            return Err(ProjectValidationError::InvalidVersion);
        }
        self.expected_active_assignments =
            normalize_active_work(self.active_work, self.expected_active_assignments)?;
        Ok(self)
    }
}

/// What archiving or deleting an active project would do, read before the
/// confirmation. The versions are the ones an archive must send back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDispositionPreview {
    pub project_id: String,
    pub name: String,
    #[serde(with = "crate::serde_u64")]
    pub project_version: u64,
    pub orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub orchestrator_worker_version: u64,
    #[serde(default, with = "crate::serde_u64::option")]
    pub orchestrator_runtime_version: Option<u64>,
    /// Assignments still allocating, active, or handing off, other than
    /// summary workers. An archive with `active_work = cancel` must echo this
    /// set and `summary_worker_assignments` back together as
    /// `expected_active_assignments`.
    pub active_assignments: Vec<ProjectDispositionAssignment>,
    /// Active assignments of Yard's own ephemeral summary workers, listed
    /// separately. An archive with `active_work = cancel` ends them too and
    /// marks their summary command failed (`project_archived`).
    #[serde(default)]
    pub summary_worker_assignments: Vec<ProjectDispositionAssignment>,
}

/// One active assignment and the worker an archive would end with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDispositionAssignment {
    pub assignment_id: String,
    #[serde(with = "crate::serde_u64")]
    pub assignment_version: u64,
    pub lifecycle: AssignmentLifecycle,
    pub objective: String,
    pub role: String,
    pub profile_name: String,
    pub worker_id: String,
    /// The worker's user-chosen label, if any.
    #[serde(default)]
    pub worker_display_name: Option<String>,
    /// Whether the worker still has a bound Herdr runtime (its tab stays
    /// open after the archive until someone closes it).
    pub runtime_present: bool,
}

impl ProjectDispositionPreview {
    /// The expected set an archive that ends every listed assignment sends.
    #[must_use]
    pub fn expected_active_assignments(&self) -> Vec<ExpectedActiveAssignment> {
        let mut expected = self
            .active_assignments
            .iter()
            .chain(&self.summary_worker_assignments)
            .map(|assignment| ExpectedActiveAssignment {
                assignment_id: assignment.assignment_id.clone(),
                expected_assignment_version: assignment.assignment_version,
            })
            .collect::<Vec<_>>();
        expected.sort();
        expected
    }
}

/// Background work that continues after a project archive or delete commits.
/// It never blocks the disposition; runtime cleanup is reported separately by
/// the result's `cleanup_pending`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectBackgroundStatus {
    pub snapshots_pending: u64,
    pub snapshots_abandoned: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedProject {
    pub command_id: String,
    pub project_id: String,
    pub orchestrator_worker_id: String,
    pub archived_at_unix_ms: u64,
    pub cleanup_pending: bool,
    #[serde(default)]
    pub background: ProjectBackgroundStatus,
    /// Assignments this archive recorded as cancelled (`project_archived`).
    #[serde(default)]
    pub cancelled_assignment_ids: Vec<String>,
    /// Whether `restore` can currently undo this archive: the project is not
    /// deleted, this archive is its current one, and its Herdr workspace is
    /// not reserved by anything else. Herdr reachability is checked only
    /// when Restore runs.
    #[serde(default)]
    pub restorable: bool,
    /// The project's visibility now; a replay after Restore reports `active`.
    #[serde(default = "ProjectVisibility::archived")]
    pub visibility: ProjectVisibility,
    pub replayed: bool,
}

/// Derived from the tombstones, never stored separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectVisibility {
    Active,
    Archived,
    Deleted,
}

impl ProjectVisibility {
    const fn archived() -> Self {
        Self::Archived
    }
}

/// Undo a project archive while it is restorable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreProject {
    pub command_id: String,
    pub actor: String,
    /// The archive the caller saw; a newer archive (or none) refuses.
    pub expected_archive_command_id: String,
}

impl RestoreProject {
    /// Normalize and validate a project restore command.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required identifier is blank
    /// or oversized.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.command_id =
            bounded_required("command_id", &self.command_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
        self.expected_archive_command_id = bounded_required(
            "expected_archive_command_id",
            &self.expected_archive_command_id,
            MAX_PROJECT_COMMAND_BYTES,
        )?;
        Ok(self)
    }
}

/// What Restore did with the orchestrator's runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoredOrchestratorRuntime {
    /// The archived Herdr tab was still running and is bound again.
    Rebound,
    /// No runtime to re-bind (none at archive time, closed since, or its
    /// identity now belongs to another pane): the orchestrator is restored
    /// unbound, as after a crash.
    Unbound,
}

impl RestoredOrchestratorRuntime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rebound => "rebound",
            Self::Unbound => "unbound",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredProject {
    pub command_id: String,
    pub project_id: String,
    pub archive_command_id: String,
    pub orchestrator_worker_id: String,
    pub orchestrator_runtime: RestoredOrchestratorRuntime,
    /// Assignments the archive cancelled. Restore never reopens them.
    pub cancelled_assignment_ids: Vec<String>,
    pub restored_at_unix_ms: u64,
    /// The project's visibility now (a replay after a re-archive reports
    /// `archived`).
    pub visibility: ProjectVisibility,
    pub replayed: bool,
}

/// Why Restore cannot run right now. Every refusal is 409
/// `project_restore_unavailable` with one of these reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRestoreUnavailableReason {
    /// Herdr could not be inventoried, so a still-running orchestrator could
    /// be orphaned by restoring it unbound.
    HerdrUnreachable,
    /// Another project, claim, quarantine, or cleanup job holds the
    /// project's Herdr workspace.
    WorkspaceReserved,
    /// The orchestrator's archived runtime is bound, claimed, or reserved by
    /// something else.
    RuntimeReserved,
    /// The project's current archive is not the one the caller saw.
    ArchiveChanged,
    /// The project was deleted; deletion is permanent.
    ProjectDeleted,
    /// The orchestrator worker was deleted or is no longer the one the
    /// archive ended.
    OrchestratorUnavailable,
}

impl ProjectRestoreUnavailableReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HerdrUnreachable => "herdr_unreachable",
            Self::WorkspaceReserved => "workspace_reserved",
            Self::RuntimeReserved => "runtime_reserved",
            Self::ArchiveChanged => "archive_changed",
            Self::ProjectDeleted => "project_deleted",
            Self::OrchestratorUnavailable => "orchestrator_unavailable",
        }
    }
}

/// One archived (not deleted) project, for the Archived view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedProjectSummary {
    pub project_id: String,
    pub name: String,
    pub archive_command_id: String,
    pub orchestrator_worker_id: String,
    pub archived_at_unix_ms: u64,
    pub cancelled_assignment_ids: Vec<String>,
    pub cleanup_pending: bool,
    pub restorable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedProjects {
    pub projects: Vec<ArchivedProjectSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteProject {
    pub command_id: String,
    pub actor: String,
    /// Required when the project is still active: delete then archives it in
    /// the same transaction. Ignored when the project is already archived.
    #[serde(default)]
    pub archive: Option<ProjectArchivePreconditions>,
}

impl DeleteProject {
    /// Normalize and validate an irreversible project visibility deletion.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required identifier is blank
    /// or oversized, or an archive precondition is invalid.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.command_id =
            bounded_required("command_id", &self.command_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
        self.archive = self
            .archive
            .map(ProjectArchivePreconditions::normalize)
            .transpose()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedProject {
    pub command_id: String,
    pub project_id: String,
    pub orchestrator_worker_id: String,
    pub deleted_at_unix_ms: u64,
    pub cleanup_pending: bool,
    #[serde(default)]
    pub background: ProjectBackgroundStatus,
    /// Assignments the embedded archive recorded as cancelled.
    #[serde(default)]
    pub cancelled_assignment_ids: Vec<String>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub runtime: ProjectRuntimeBinding,
    pub orchestrator_observed_worker_id: String,
    pub placement: CanvasPlacement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateProjectFromProfile {
    pub command_id: String,
    pub actor: String,
    pub name: String,
    pub runtime: ProjectRuntimeBinding,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    pub orchestrator_objective: String,
    pub placement: CanvasPlacement,
}

impl CreateProjectFromProfile {
    /// Normalize and validate profile-backed project creation.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when identifiers, the project name,
    /// objective, runtime binding, placement, or optimistic profile version
    /// are invalid.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.command_id =
            bounded_required("command_id", &self.command_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
        self.name = bounded_required("name", &self.name, MAX_PROJECT_NAME_BYTES)?;
        self.runtime = self.runtime.normalize()?;
        self.profile_id =
            bounded_required("profile_id", &self.profile_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.orchestrator_objective = bounded_required(
            "orchestrator_objective",
            &self.orchestrator_objective,
            MAX_PROJECT_OBJECTIVE_BYTES,
        )?;
        if self.expected_profile_version == 0 {
            return Err(ProjectValidationError::InvalidVersion);
        }
        self.placement.validate()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateWorkspaceProjectFromProfile {
    pub command_id: String,
    pub actor: String,
    pub name: String,
    pub runtime_adapter: String,
    pub runtime_session: String,
    pub workspace_label: String,
    pub cwd: String,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    pub orchestrator_objective: String,
    pub placement: CanvasPlacement,
}

impl CreateWorkspaceProjectFromProfile {
    /// Normalize and validate workspace-backed profile project creation.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when required text, optimistic
    /// profile version, or canvas placement are invalid.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.command_id =
            bounded_required("command_id", &self.command_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
        self.name = bounded_required("name", &self.name, MAX_PROJECT_NAME_BYTES)?;
        self.runtime_adapter = bounded_required(
            "runtime_adapter",
            &self.runtime_adapter,
            MAX_PROJECT_COMMAND_BYTES,
        )?;
        self.runtime_session = bounded_required(
            "runtime_session",
            &self.runtime_session,
            MAX_PROJECT_COMMAND_BYTES,
        )?;
        self.workspace_label = bounded_required(
            "workspace_label",
            &self.workspace_label,
            MAX_PROJECT_NAME_BYTES,
        )?;
        self.cwd = bounded_required("cwd", &self.cwd, MAX_PROJECT_CWD_BYTES)?;
        self.profile_id =
            bounded_required("profile_id", &self.profile_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.orchestrator_objective = bounded_required(
            "orchestrator_objective",
            &self.orchestrator_objective,
            MAX_PROJECT_OBJECTIVE_BYTES,
        )?;
        if self.expected_profile_version == 0 {
            return Err(ProjectValidationError::InvalidVersion);
        }
        self.placement.validate()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedProjectCreation {
    pub command_id: String,
    pub project: Project,
    pub replayed: bool,
}

impl CreateProject {
    /// Normalize user-entered text and validate the project adoption command.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when required identifiers, the
    /// project name, or canvas placement are invalid.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.name = required("name", &self.name)?;
        if self.name.len() > MAX_PROJECT_NAME_BYTES {
            return Err(ProjectValidationError::NameTooLong);
        }
        self.runtime = self.runtime.normalize()?;
        self.orchestrator_observed_worker_id = required(
            "orchestrator_observed_worker_id",
            &self.orchestrator_observed_worker_id,
        )?;
        self.placement.validate()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UpdateProjectPlacement {
    pub placement: CanvasPlacement,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProjectWorkflowProfile {
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub profile_version: u64,
}

impl UpdateProjectWorkflowProfile {
    /// Normalize and validate an immutable workflow-profile revision pin.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required value is blank or an
    /// optimistic/profile version is zero.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
        self.profile_id =
            bounded_required("profile_id", &self.profile_id, MAX_PROJECT_COMMAND_BYTES)?;
        if self.expected_project_version == 0 || self.profile_version == 0 {
            return Err(ProjectValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

impl UpdateProjectPlacement {
    /// Validate a full placement replacement.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when the version is zero or the
    /// placement is invalid.
    pub fn validate(&self) -> Result<(), ProjectValidationError> {
        if self.expected_version == 0 {
            return Err(ProjectValidationError::InvalidVersion);
        }
        self.placement.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("project name must be at most {MAX_PROJECT_NAME_BYTES} bytes")]
    NameTooLong,
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("canvas coordinates must be finite numbers")]
    InvalidPosition,
    #[error(
        "canvas size must be between {MIN_PROJECT_WIDTH}x{MIN_PROJECT_HEIGHT} and \
         {MAX_PROJECT_WIDTH}x{MAX_PROJECT_HEIGHT}"
    )]
    InvalidSize,
    #[error("expected_version must be greater than zero")]
    InvalidVersion,
    #[error("worker runtime binding does not match the project workspace binding")]
    RuntimeBindingMismatch,
    #[error("repository root must be an absolute path")]
    InvalidRepositoryRoot,
    #[error("expected_active_assignments is accepted only with active_work = cancel")]
    UnexpectedActiveAssignments,
    #[error("expected_active_assignments lists an assignment more than once")]
    DuplicateExpectedAssignment,
    #[error("expected_active_assignments must list at most {max} assignments")]
    TooManyExpectedAssignments { max: usize },
}

fn required(field: &'static str, value: &str) -> Result<String, ProjectValidationError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(ProjectValidationError::Required(field))
    } else {
        Ok(value)
    }
}

fn bounded_required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, ProjectValidationError> {
    let value = required(field, value)?;
    if value.len() > max {
        return Err(if field == "name" {
            ProjectValidationError::NameTooLong
        } else {
            ProjectValidationError::TooLong { field, max }
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        ArchiveProject, ArchivedProject, CanvasPlacement, CreateProject, CreateProjectFromProfile,
        CreateWorkspaceProjectFromProfile, DeleteProject, ExpectedActiveAssignment,
        MAX_EXPECTED_ACTIVE_ASSIGNMENTS, MAX_PROJECT_CWD_BYTES, ProjectArchiveActiveWork,
        ProjectArchivePreconditions, ProjectRestoreUnavailableReason, ProjectRuntimeBinding,
        ProjectValidationError, ProjectVisibility, RestoreProject, SetProjectRepository,
        UpdateProjectPlacement, UpdateProjectWorkflowProfile, Worker, WorkerOwnershipKind,
    };

    fn placement() -> CanvasPlacement {
        CanvasPlacement {
            x: 40.0,
            y: 60.0,
            width: 322.0,
            height: 240.0,
        }
    }

    #[test]
    fn normalizes_project_archive_input() {
        let command = ArchiveProject {
            command_id: " archive-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_project_version: 3,
            expected_orchestrator_worker_id: " worker-1 ".to_owned(),
            expected_orchestrator_worker_version: 4,
            expected_orchestrator_runtime_version: Some(2),
            active_work: ProjectArchiveActiveWork::Reject,
            expected_active_assignments: None,
        }
        .normalize()
        .unwrap();

        assert_eq!(command.command_id, "archive-1");
        assert_eq!(command.actor, "local-user");
        assert_eq!(command.expected_orchestrator_worker_id, "worker-1");
    }

    fn expected(assignment_id: &str, version: u64) -> ExpectedActiveAssignment {
        ExpectedActiveAssignment {
            assignment_id: assignment_id.to_owned(),
            expected_assignment_version: version,
        }
    }

    fn cancel_archive(expected: Option<Vec<ExpectedActiveAssignment>>) -> ArchiveProject {
        ArchiveProject {
            command_id: "archive-1".to_owned(),
            actor: "local-user".to_owned(),
            expected_project_version: 3,
            expected_orchestrator_worker_id: "worker-1".to_owned(),
            expected_orchestrator_worker_version: 4,
            expected_orchestrator_runtime_version: None,
            active_work: ProjectArchiveActiveWork::Cancel,
            expected_active_assignments: expected,
        }
    }

    #[test]
    fn archive_active_work_defaults_to_reject_and_cancel_needs_the_previewed_set() {
        let legacy: ArchiveProject = serde_json::from_str(
            r#"{"command_id":"a","actor":"u","expected_project_version":"1",
                "expected_orchestrator_worker_id":"w",
                "expected_orchestrator_worker_version":"1"}"#,
        )
        .unwrap();
        let legacy = legacy.normalize().unwrap();
        assert_eq!(legacy.active_work, ProjectArchiveActiveWork::Reject);
        assert_eq!(legacy.expected_active_assignments, None);

        let parsed: ArchiveProject = serde_json::from_str(
            r#"{"command_id":"a","actor":"u","expected_project_version":"1",
                "expected_orchestrator_worker_id":"w",
                "expected_orchestrator_worker_version":"1","active_work":"cancel",
                "expected_active_assignments":[
                    {"assignment_id":" b ","expected_assignment_version":"2"},
                    {"assignment_id":"a","expected_assignment_version":"5"}]}"#,
        )
        .unwrap();
        let parsed = parsed.normalize().unwrap();
        assert_eq!(
            parsed.expected_active_assignments,
            Some(vec![expected("a", 5), expected("b", 2)]),
            "trimmed and sorted"
        );
        assert_eq!(
            parsed.preconditions().expected_active_assignments,
            parsed.expected_active_assignments
        );
        assert_eq!(
            cancel_archive(Some(Vec::new()))
                .normalize()
                .unwrap()
                .expected_active_assignments,
            Some(Vec::new())
        );

        assert_eq!(
            cancel_archive(None).normalize().unwrap_err(),
            ProjectValidationError::Required("expected_active_assignments")
        );
        let mut reject_with_set = cancel_archive(Some(Vec::new()));
        reject_with_set.active_work = ProjectArchiveActiveWork::Reject;
        assert_eq!(
            reject_with_set.normalize().unwrap_err(),
            ProjectValidationError::UnexpectedActiveAssignments
        );
        assert_eq!(
            cancel_archive(Some(vec![expected("a", 1), expected(" a", 2)]))
                .normalize()
                .unwrap_err(),
            ProjectValidationError::DuplicateExpectedAssignment
        );
        assert_eq!(
            cancel_archive(Some(vec![expected("a", 0)]))
                .normalize()
                .unwrap_err(),
            ProjectValidationError::InvalidVersion
        );
        assert_eq!(
            cancel_archive(Some(vec![expected(" ", 1)]))
                .normalize()
                .unwrap_err(),
            ProjectValidationError::Required("expected_active_assignments.assignment_id")
        );
        let too_many = (0..=MAX_EXPECTED_ACTIVE_ASSIGNMENTS)
            .map(|index| expected(&format!("assignment-{index}"), 1))
            .collect();
        assert_eq!(
            cancel_archive(Some(too_many)).normalize().unwrap_err(),
            ProjectValidationError::TooManyExpectedAssignments {
                max: MAX_EXPECTED_ACTIVE_ASSIGNMENTS
            }
        );

        // Delete carries the same choice inside its archive preconditions.
        let delete: DeleteProject = serde_json::from_str(
            r#"{"command_id":"d","actor":"u","archive":{"expected_project_version":"1",
                "expected_orchestrator_worker_id":"w",
                "expected_orchestrator_worker_version":"1","active_work":"cancel"}}"#,
        )
        .unwrap();
        assert_eq!(
            delete.normalize().unwrap_err(),
            ProjectValidationError::Required("expected_active_assignments")
        );
    }

    #[test]
    fn normalizes_project_delete_archive_preconditions() {
        let legacy: DeleteProject =
            serde_json::from_str(r#"{"command_id":"delete-1","actor":"local-user"}"#).unwrap();
        assert_eq!(legacy.normalize().unwrap().archive, None);

        let command = DeleteProject {
            command_id: " delete-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            archive: Some(ProjectArchivePreconditions {
                expected_project_version: 3,
                expected_orchestrator_worker_id: " worker-1 ".to_owned(),
                expected_orchestrator_worker_version: 4,
                expected_orchestrator_runtime_version: None,
                active_work: ProjectArchiveActiveWork::Reject,
                expected_active_assignments: None,
            }),
        }
        .normalize()
        .unwrap();
        assert_eq!(command.command_id, "delete-1");
        assert_eq!(
            command.archive.unwrap().expected_orchestrator_worker_id,
            "worker-1"
        );

        let zero_version = DeleteProject {
            command_id: "delete-2".to_owned(),
            actor: "local-user".to_owned(),
            archive: Some(ProjectArchivePreconditions {
                expected_project_version: 0,
                expected_orchestrator_worker_id: "worker-1".to_owned(),
                expected_orchestrator_worker_version: 4,
                expected_orchestrator_runtime_version: None,
                active_work: ProjectArchiveActiveWork::Reject,
                expected_active_assignments: None,
            }),
        };
        assert_eq!(
            zero_version.normalize().unwrap_err(),
            ProjectValidationError::InvalidVersion
        );
    }

    #[test]
    fn normalizes_project_restore_and_reads_pre_restore_archive_results() {
        let command = RestoreProject {
            command_id: " restore-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_archive_command_id: " archive-1 ".to_owned(),
        }
        .normalize()
        .unwrap();
        assert_eq!(command.command_id, "restore-1");
        assert_eq!(command.actor, "local-user");
        assert_eq!(command.expected_archive_command_id, "archive-1");
        let blank = RestoreProject {
            command_id: "restore-2".to_owned(),
            actor: "local-user".to_owned(),
            expected_archive_command_id: "  ".to_owned(),
        };
        assert_eq!(
            blank.normalize().unwrap_err(),
            ProjectValidationError::Required("expected_archive_command_id")
        );

        // An archive result stored or sent before Restore existed.
        let legacy: ArchivedProject = serde_json::from_str(
            r#"{"command_id":"archive-1","project_id":"project-1",
                "orchestrator_worker_id":"worker-1","archived_at_unix_ms":1,
                "cleanup_pending":false,"replayed":false}"#,
        )
        .unwrap();
        assert!(!legacy.restorable);
        assert_eq!(legacy.visibility, ProjectVisibility::Archived);
        assert_eq!(
            serde_json::to_value(ProjectRestoreUnavailableReason::HerdrUnreachable).unwrap(),
            serde_json::json!(ProjectRestoreUnavailableReason::HerdrUnreachable.as_str())
        );
    }

    #[test]
    fn normalizes_project_adoption_input() {
        let project = CreateProject {
            name: "  Runtime API  ".to_owned(),
            runtime: ProjectRuntimeBinding {
                adapter: " herdr ".to_owned(),
                session: " default ".to_owned(),
                workspace_id: " workspace-1 ".to_owned(),
            },
            orchestrator_observed_worker_id: " terminal-1 ".to_owned(),
            placement: placement(),
        }
        .normalize()
        .unwrap();

        assert_eq!(project.name, "Runtime API");
        assert_eq!(project.runtime.adapter, "herdr");
        assert_eq!(project.orchestrator_observed_worker_id, "terminal-1");
    }

    #[test]
    fn repository_roots_are_absolute_and_trimmed() {
        let repository = SetProjectRepository {
            root_path: " /tmp/yard ".to_owned(),
        }
        .normalize()
        .unwrap();
        assert_eq!(repository.root_path, "/tmp/yard");

        assert!(matches!(
            SetProjectRepository {
                root_path: "relative".to_owned(),
            }
            .normalize(),
            Err(ProjectValidationError::InvalidRepositoryRoot),
        ));
    }

    #[test]
    fn repository_roots_reject_caller_supplied_identity() {
        let error = serde_json::from_str::<SetProjectRepository>(
            r#"{"root_path":"/tmp/yard","git_common_dir":"/forged"}"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `git_common_dir`"));
    }

    #[test]
    fn normalizes_profile_backed_project_creation() {
        let project = CreateProjectFromProfile {
            command_id: " create-runtime-api ".to_owned(),
            actor: " local-user ".to_owned(),
            name: " Runtime API ".to_owned(),
            runtime: ProjectRuntimeBinding {
                adapter: " herdr ".to_owned(),
                session: " default ".to_owned(),
                workspace_id: " workspace-1 ".to_owned(),
            },
            profile_id: " profile-1 ".to_owned(),
            expected_profile_version: 1,
            orchestrator_objective: " Coordinate the Runtime API project. ".to_owned(),
            placement: placement(),
        }
        .normalize()
        .unwrap();

        assert_eq!(project.command_id, "create-runtime-api");
        assert_eq!(project.name, "Runtime API");
        assert_eq!(project.runtime.workspace_id, "workspace-1");
        assert_eq!(
            project.orchestrator_objective,
            "Coordinate the Runtime API project."
        );
    }

    #[test]
    fn profile_backed_project_creation_requires_a_profile_version() {
        let error = CreateProjectFromProfile {
            command_id: "create-runtime-api".to_owned(),
            actor: "local-user".to_owned(),
            name: "Runtime API".to_owned(),
            runtime: ProjectRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "default".to_owned(),
                workspace_id: "workspace-1".to_owned(),
            },
            profile_id: "profile-1".to_owned(),
            expected_profile_version: 0,
            orchestrator_objective: "Coordinate the Runtime API project.".to_owned(),
            placement: placement(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(error, ProjectValidationError::InvalidVersion);
    }

    #[test]
    fn normalizes_workspace_backed_project_creation() {
        let project = CreateWorkspaceProjectFromProfile {
            command_id: " create-runtime-api ".to_owned(),
            actor: " local-user ".to_owned(),
            name: " Runtime API ".to_owned(),
            runtime_adapter: " herdr ".to_owned(),
            runtime_session: " default ".to_owned(),
            workspace_label: " Runtime API workspace ".to_owned(),
            cwd: " /work/runtime-api ".to_owned(),
            profile_id: " profile-1 ".to_owned(),
            expected_profile_version: 1,
            orchestrator_objective: " Coordinate the Runtime API project. ".to_owned(),
            placement: placement(),
        }
        .normalize()
        .unwrap();

        assert_eq!(project.command_id, "create-runtime-api");
        assert_eq!(project.runtime_adapter, "herdr");
        assert_eq!(project.workspace_label, "Runtime API workspace");
        assert_eq!(project.cwd, "/work/runtime-api");
        assert_eq!(
            project.orchestrator_objective,
            "Coordinate the Runtime API project."
        );
    }

    #[test]
    fn workspace_backed_project_creation_enforces_cwd_bound() {
        let error = CreateWorkspaceProjectFromProfile {
            command_id: "create-runtime-api".to_owned(),
            actor: "local-user".to_owned(),
            name: "Runtime API".to_owned(),
            runtime_adapter: "herdr".to_owned(),
            runtime_session: "default".to_owned(),
            workspace_label: "Runtime API workspace".to_owned(),
            cwd: "x".repeat(MAX_PROJECT_CWD_BYTES + 1),
            profile_id: "profile-1".to_owned(),
            expected_profile_version: 1,
            orchestrator_objective: "Coordinate the Runtime API project.".to_owned(),
            placement: placement(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(
            error,
            ProjectValidationError::TooLong {
                field: "cwd",
                max: MAX_PROJECT_CWD_BYTES,
            }
        );
    }

    #[test]
    fn rejects_non_finite_placement() {
        let error = CanvasPlacement {
            x: f64::NAN,
            ..placement()
        }
        .validate()
        .unwrap_err();

        assert_eq!(error, ProjectValidationError::InvalidPosition);
    }

    #[test]
    fn rejects_zero_expected_version() {
        let error = UpdateProjectPlacement {
            placement: placement(),
            expected_version: 0,
        }
        .validate()
        .unwrap_err();

        assert_eq!(error, ProjectValidationError::InvalidVersion);
    }

    #[test]
    fn worker_ownership_serializes_and_defaults_to_external() {
        let worker = serde_json::from_value::<Worker>(serde_json::json!({
            "id": "worker-1",
            "profile_id": null,
            "profile_version": null,
            "desired_state": "running",
            "runtime": null,
            "version": "1",
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 1
        }))
        .unwrap();
        assert_eq!(worker.ownership_kind, WorkerOwnershipKind::External);

        let serialized = serde_json::to_value(Worker {
            ownership_kind: WorkerOwnershipKind::YardOwned,
            ..worker
        })
        .unwrap();
        assert_eq!(serialized["ownership_kind"], "yard_owned");
    }

    #[test]
    fn normalizes_project_workflow_revision_pin() {
        let pin = UpdateProjectWorkflowProfile {
            actor: " local-user ".to_owned(),
            expected_project_version: 2,
            profile_id: " yard:standard-orchestrator ".to_owned(),
            profile_version: 1,
        }
        .normalize()
        .unwrap();

        assert_eq!(pin.actor, "local-user");
        assert_eq!(pin.profile_id, "yard:standard-orchestrator");
    }
}

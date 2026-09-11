use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ObservedStatus, ProviderSessionRef, WorkerDesiredState};

const MAX_PROJECT_NAME_BYTES: usize = 120;
const MAX_PROJECT_COMMAND_BYTES: usize = 120;
const MAX_PROJECT_CWD_BYTES: usize = 4_096;
const MAX_PROJECT_OBJECTIVE_BYTES: usize = 16_000;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub profile_id: Option<String>,
    #[serde(default, with = "crate::serde_u64::option")]
    pub profile_version: Option<u64>,
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
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedProject {
    pub command_id: String,
    pub project_id: String,
    pub orchestrator_worker_id: String,
    pub archived_at_unix_ms: u64,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteProject {
    pub command_id: String,
    pub actor: String,
}

impl DeleteProject {
    /// Normalize and validate an irreversible project visibility deletion.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectValidationError`] when a required identifier is blank
    /// or oversized.
    pub fn normalize(mut self) -> Result<Self, ProjectValidationError> {
        self.command_id =
            bounded_required("command_id", &self.command_id, MAX_PROJECT_COMMAND_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_PROJECT_COMMAND_BYTES)?;
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
        ArchiveProject, CanvasPlacement, CreateProject, CreateProjectFromProfile,
        CreateWorkspaceProjectFromProfile, MAX_PROJECT_CWD_BYTES, ProjectRuntimeBinding,
        ProjectValidationError, UpdateProjectPlacement, UpdateProjectWorkflowProfile,
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
        }
        .normalize()
        .unwrap();

        assert_eq!(command.command_id, "archive-1");
        assert_eq!(command.actor, "local-user");
        assert_eq!(command.expected_orchestrator_worker_id, "worker-1");
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

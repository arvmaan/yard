use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{CanvasPlacement, OrchestratorStatusReport, Worker};

const MAX_NAME_BYTES: usize = 120;
const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_PROMPT_BYTES: usize = 16_000;
const MAX_ATTACHED_PROJECTS: usize = 256;
const MIN_COORDINATION_NODE_WIDTH: f64 = 116.0;
const MIN_COORDINATION_NODE_HEIGHT: f64 = 116.0;
const MAX_COORDINATION_NODE_WIDTH: f64 = 2_400.0;
const MAX_COORDINATION_NODE_HEIGHT: f64 = 2_400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationNodeKind {
    Workstream,
    KnowledgeStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoordinationNodePlacement {
    pub geometry: CanvasPlacement,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoordinationNode {
    pub id: String,
    pub name: String,
    pub kind: CoordinationNodeKind,
    pub placement: CoordinationNodePlacement,
    pub attached_project_ids: Vec<String>,
    pub worker: Option<Worker>,
    pub cwd: Option<String>,
    pub folder_path: Option<String>,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_by: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoordinationNodes {
    pub nodes: Vec<CoordinationNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateCoordinationNode {
    pub command_id: String,
    pub actor: String,
    pub name: String,
    pub kind: CoordinationNodeKind,
    pub placement: CanvasPlacement,
    #[serde(default)]
    pub attached_project_ids: Vec<String>,
}

impl CreateCoordinationNode {
    /// Normalize a coordination-node creation command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text, placement,
    /// duplicate projects, or project IDs that are not canonical UUIDs.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.name = required("name", &self.name, MAX_NAME_BYTES)?;
        validate_coordination_placement(&self.placement)?;
        self.attached_project_ids = normalize_project_ids(self.attached_project_ids)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateCoordinationNode {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub name: String,
    #[serde(default)]
    pub attached_project_ids: Vec<String>,
}

impl UpdateCoordinationNode {
    /// Normalize a coordination-node update command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text, versions,
    /// duplicate projects, or project IDs that are not canonical UUIDs.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.name = required("name", &self.name, MAX_NAME_BYTES)?;
        require_version(self.expected_version)?;
        self.attached_project_ids = normalize_project_ids(self.attached_project_ids)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateCoordinationNodePlacement {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
    pub placement: CanvasPlacement,
}

impl UpdateCoordinationNodePlacement {
    /// Normalize a versioned node-placement command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text, version,
    /// or geometry.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        require_version(self.expected_version)?;
        validate_coordination_placement(&self.placement)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoordinationNodeCommandResult {
    pub command_id: String,
    pub node: CoordinationNode,
    pub replayed: bool,
}

/// The optimistic versions a caller saw before archiving a coordination node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeArchivePreconditions {
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
    /// Checked only when present. The node version already pins which
    /// dedicated worker is attached, and a live worker's version moves with
    /// every runtime status change.
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_worker_version: Option<u64>,
}

impl CoordinationNodeArchivePreconditions {
    /// Normalize and validate node archive preconditions.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] when a version is zero.
    pub fn normalize(self) -> Result<Self, CoordinationNodeValidationError> {
        require_version(self.expected_node_version)?;
        if self.expected_worker_version == Some(0) {
            return Err(CoordinationNodeValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveCoordinationNode {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_worker_version: Option<u64>,
}

impl ArchiveCoordinationNode {
    /// Normalize a durable coordination-node archive command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text or a zero
    /// optimistic version.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.preconditions().normalize()?;
        Ok(self)
    }

    #[must_use]
    pub fn preconditions(&self) -> CoordinationNodeArchivePreconditions {
        CoordinationNodeArchivePreconditions {
            expected_node_version: self.expected_node_version,
            expected_worker_version: self.expected_worker_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedCoordinationNode {
    pub command_id: String,
    pub node_id: String,
    pub kind: CoordinationNodeKind,
    /// The dedicated worker this archive ended, if the node had one.
    pub worker_id: Option<String>,
    /// Node-scoped automations this archive paused.
    pub paused_automation_ids: Vec<String>,
    pub archived_at_unix_ms: u64,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteCoordinationNode {
    pub command_id: String,
    pub actor: String,
    /// Required when the node is still active: delete then archives it in the
    /// same transaction. Ignored when the node is already archived.
    #[serde(default)]
    pub archive: Option<CoordinationNodeArchivePreconditions>,
}

impl DeleteCoordinationNode {
    /// Normalize an irreversible coordination-node visibility deletion.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text or an
    /// invalid archive precondition.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.archive = self
            .archive
            .map(CoordinationNodeArchivePreconditions::normalize)
            .transpose()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedCoordinationNode {
    pub command_id: String,
    pub node_id: String,
    pub kind: CoordinationNodeKind,
    pub worker_id: Option<String>,
    /// Automations paused by the archive this delete performed; empty when
    /// the node was already archived.
    pub paused_automation_ids: Vec<String>,
    pub deleted_at_unix_ms: u64,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

/// What archiving or deleting a node would do, read before confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeDispositionPreview {
    pub node_id: String,
    pub name: String,
    pub kind: CoordinationNodeKind,
    #[serde(with = "crate::serde_u64")]
    pub node_version: u64,
    /// Whether this kind can be archived and deleted.
    pub supported: bool,
    pub worker: Option<CoordinationNodeDispositionWorker>,
    /// Active attached projects. Archive and delete leave them untouched.
    pub attached_projects: Vec<CoordinationNodeDispositionProject>,
    /// Automations scoped to the node; active ones are paused by archive.
    pub automations: Vec<CoordinationNodeDispositionAutomation>,
    /// In-flight commands that make archive and delete wait.
    pub blockers: Vec<CoordinationNodeDispositionBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeDispositionWorker {
    pub worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub worker_version: u64,
    pub profile_name: Option<String>,
    pub runtime_present: bool,
    pub will_end: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeDispositionProject {
    pub project_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeDispositionAutomation {
    pub automation_id: String,
    pub name: String,
    pub state: crate::AutomationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationNodeDispositionBlockerKind {
    Prompt,
    Route,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeDispositionBlocker {
    pub kind: CoordinationNodeDispositionBlockerKind,
    pub command_id: String,
    pub started_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionCoordinationNode {
    pub command_id: String,
    pub actor: String,
    pub profile_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_profile_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
}

impl ProvisionCoordinationNode {
    /// Normalize a workstream-node provision command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text or
    /// optimistic versions.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.profile_id = required("profile_id", &self.profile_id, MAX_COMMAND_ID_BYTES)?;
        require_version(self.expected_profile_version)?;
        require_version(self.expected_node_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendCoordinationNodePrompt {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
    pub worker_id: String,
    pub text: String,
}

impl SendCoordinationNodePrompt {
    /// Normalize a direct workstream prompt.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text or a zero
    /// optimistic version.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.worker_id = required("worker_id", &self.worker_id, MAX_COMMAND_ID_BYTES)?;
        self.text = required("text", &self.text, MAX_PROMPT_BYTES)?;
        require_version(self.expected_node_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodePromptAcknowledgement {
    pub command_id: String,
    pub node_id: String,
    pub worker_id: String,
    pub runtime_status: String,
    pub submitted_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeTerminalOutput {
    pub node_id: String,
    pub worker_id: String,
    pub pane_id: String,
    pub source: String,
    pub format: String,
    pub text: String,
    #[serde(with = "crate::serde_u64")]
    pub revision: u64,
    pub truncated: bool,
    pub status_report: Option<OrchestratorStatusReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendCoordinationNodeRoute {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
    pub worker_id: String,
    pub target_project_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub target_orchestrator_worker_id: String,
    pub text: String,
}

impl SendCoordinationNodeRoute {
    /// Normalize a workstream-to-project route command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text, project
    /// identity, or optimistic versions.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.worker_id = required("worker_id", &self.worker_id, MAX_COMMAND_ID_BYTES)?;
        self.target_project_id = canonical_uuid("target_project_id", &self.target_project_id)?;
        self.target_orchestrator_worker_id = required(
            "target_orchestrator_worker_id",
            &self.target_orchestrator_worker_id,
            MAX_COMMAND_ID_BYTES,
        )?;
        self.text = required("text", &self.text, MAX_PROMPT_BYTES)?;
        require_version(self.expected_node_version)?;
        require_version(self.expected_project_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationDeliveryStatus {
    Pending,
    Submitted,
    Failed,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeRoute {
    pub command_id: String,
    pub node_id: String,
    pub actor: String,
    pub worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
    pub target_project_id: String,
    pub target_orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub text: String,
    pub status: CoordinationDeliveryStatus,
    pub error_message: Option<String>,
    pub runtime_status: Option<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub submitted_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationNodeRoutes {
    pub routes: Vec<CoordinationNodeRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCoordinationSnapshot {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_node_version: u64,
}

impl RequestCoordinationSnapshot {
    /// Normalize a knowledge snapshot request.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationNodeValidationError`] for invalid text or a zero
    /// optimistic version.
    pub fn normalize(mut self) -> Result<Self, CoordinationNodeValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        require_version(self.expected_node_version)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotCollectionStatus {
    Pending,
    Collected,
    /// Yard stopped waiting for the project's files. The folder is still
    /// checked, so files that arrive later still mark it collected.
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotAbandonmentReason {
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotProjectCollection {
    pub project_id: String,
    #[serde(with = "crate::serde_u64")]
    pub project_version: u64,
    pub orchestrator_worker_id: String,
    pub folder_path: String,
    pub collection_status: SnapshotCollectionStatus,
    pub delivery_status: CoordinationDeliveryStatus,
    pub delivery_error: Option<String>,
    pub runtime_status: Option<String>,
    pub submitted_at_unix_ms: Option<u64>,
    pub collected_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub abandoned_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub abandoned_reason: Option<SnapshotAbandonmentReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCollectionProgress {
    pub completed: usize,
    pub total: usize,
    #[serde(default)]
    pub abandoned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationSnapshot {
    pub id: String,
    pub node_id: String,
    pub command_id: String,
    pub folder_path: String,
    pub projects: Vec<SnapshotProjectCollection>,
    pub progress: SnapshotCollectionProgress,
    pub requested_by: String,
    pub created_at_unix_ms: u64,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinationSnapshots {
    pub snapshots: Vec<CoordinationSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoordinationNodeValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("{field} must be a canonical UUID")]
    InvalidUuid { field: &'static str },
    #[error("attached_project_ids contains a duplicate project")]
    DuplicateProject,
    #[error("attached_project_ids contains more than {max} projects")]
    TooManyProjects { max: usize },
    #[error("optimistic versions must be greater than zero")]
    InvalidVersion,
    #[error("invalid placement: {0}")]
    InvalidPlacement(String),
}

fn validate_coordination_placement(
    placement: &CanvasPlacement,
) -> Result<(), CoordinationNodeValidationError> {
    if !placement.x.is_finite() || !placement.y.is_finite() {
        return Err(CoordinationNodeValidationError::InvalidPlacement(
            "canvas coordinates must be finite numbers".to_owned(),
        ));
    }
    if !placement.width.is_finite()
        || !placement.height.is_finite()
        || !(MIN_COORDINATION_NODE_WIDTH..=MAX_COORDINATION_NODE_WIDTH).contains(&placement.width)
        || !(MIN_COORDINATION_NODE_HEIGHT..=MAX_COORDINATION_NODE_HEIGHT)
            .contains(&placement.height)
    {
        return Err(CoordinationNodeValidationError::InvalidPlacement(format!(
            "canvas size must be between \
                 {MIN_COORDINATION_NODE_WIDTH}x{MIN_COORDINATION_NODE_HEIGHT} and \
                 {MAX_COORDINATION_NODE_WIDTH}x{MAX_COORDINATION_NODE_HEIGHT}"
        )));
    }
    Ok(())
}

/// Validate and canonicalize a UUID used in a URL or managed path.
///
/// # Errors
///
/// Returns [`CoordinationNodeValidationError`] unless the input is the
/// lowercase, hyphenated canonical UUID representation.
pub fn canonical_coordination_uuid(
    field: &'static str,
    value: &str,
) -> Result<String, CoordinationNodeValidationError> {
    canonical_uuid(field, value)
}

fn normalize_project_ids(
    mut project_ids: Vec<String>,
) -> Result<Vec<String>, CoordinationNodeValidationError> {
    if project_ids.len() > MAX_ATTACHED_PROJECTS {
        return Err(CoordinationNodeValidationError::TooManyProjects {
            max: MAX_ATTACHED_PROJECTS,
        });
    }
    let mut seen = HashSet::with_capacity(project_ids.len());
    for project_id in &mut project_ids {
        *project_id = canonical_uuid("attached_project_ids", project_id)?;
        if !seen.insert(project_id.clone()) {
            return Err(CoordinationNodeValidationError::DuplicateProject);
        }
    }
    project_ids.sort_unstable();
    Ok(project_ids)
}

fn canonical_uuid(
    field: &'static str,
    value: &str,
) -> Result<String, CoordinationNodeValidationError> {
    let value = value.trim();
    let parsed = Uuid::parse_str(value)
        .map_err(|_| CoordinationNodeValidationError::InvalidUuid { field })?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != value {
        return Err(CoordinationNodeValidationError::InvalidUuid { field });
    }
    Ok(canonical)
}

fn require_version(version: u64) -> Result<(), CoordinationNodeValidationError> {
    if version == 0 {
        Err(CoordinationNodeValidationError::InvalidVersion)
    } else {
        Ok(())
    }
}

fn required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, CoordinationNodeValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoordinationNodeValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(CoordinationNodeValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        ArchiveCoordinationNode, CoordinationNodeArchivePreconditions, CoordinationNodeKind,
        CoordinationNodeValidationError, CreateCoordinationNode, DeleteCoordinationNode,
        SendCoordinationNodeRoute, UpdateCoordinationNode,
    };
    use crate::CanvasPlacement;

    const PROJECT_A: &str = "0198a81c-3773-7c60-b7d2-ff795ad88ad1";
    const PROJECT_B: &str = "0198a81c-3773-7c60-b7d2-ff795ad88ad2";

    fn placement() -> CanvasPlacement {
        CanvasPlacement {
            x: 10.0,
            y: 20.0,
            width: 116.0,
            height: 116.0,
        }
    }

    #[test]
    fn normalizes_nodes_and_orders_canonical_attachments() {
        let command = CreateCoordinationNode {
            command_id: " create-node ".to_owned(),
            actor: " local-user ".to_owned(),
            name: " Release coordination ".to_owned(),
            kind: CoordinationNodeKind::Workstream,
            placement: placement(),
            attached_project_ids: vec![PROJECT_B.to_owned(), PROJECT_A.to_owned()],
        }
        .normalize()
        .unwrap();

        assert_eq!(command.name, "Release coordination");
        assert_eq!(command.attached_project_ids, [PROJECT_A, PROJECT_B]);
    }

    #[test]
    fn rejects_coordination_nodes_smaller_than_the_compact_map_marker() {
        let error = CreateCoordinationNode {
            command_id: "create-node".to_owned(),
            actor: "local-user".to_owned(),
            name: "Knowledge".to_owned(),
            kind: CoordinationNodeKind::KnowledgeStore,
            placement: CanvasPlacement {
                width: 115.0,
                ..placement()
            },
            attached_project_ids: Vec::new(),
        }
        .normalize()
        .unwrap_err();

        assert_eq!(
            error,
            CoordinationNodeValidationError::InvalidPlacement(
                "canvas size must be between 116x116 and 2400x2400".to_owned()
            )
        );
    }

    #[test]
    fn rejects_noncanonical_and_duplicate_project_ids() {
        let invalid = UpdateCoordinationNode {
            command_id: "update-node".to_owned(),
            actor: "local-user".to_owned(),
            expected_version: 1,
            name: "Knowledge".to_owned(),
            attached_project_ids: vec![PROJECT_A.to_uppercase()],
        }
        .normalize()
        .unwrap_err();
        assert_eq!(
            invalid,
            CoordinationNodeValidationError::InvalidUuid {
                field: "attached_project_ids"
            }
        );

        let duplicate = UpdateCoordinationNode {
            command_id: "update-node".to_owned(),
            actor: "local-user".to_owned(),
            expected_version: 1,
            name: "Knowledge".to_owned(),
            attached_project_ids: vec![PROJECT_A.to_owned(), PROJECT_A.to_owned()],
        }
        .normalize()
        .unwrap_err();
        assert_eq!(duplicate, CoordinationNodeValidationError::DuplicateProject);
    }

    #[test]
    fn validates_route_scope_identity_and_versions() {
        let route = SendCoordinationNodeRoute {
            command_id: "route".to_owned(),
            actor: "local-user".to_owned(),
            expected_node_version: 2,
            worker_id: "worker".to_owned(),
            target_project_id: PROJECT_A.to_owned(),
            expected_project_version: 3,
            target_orchestrator_worker_id: "orchestrator".to_owned(),
            text: "Coordinate the release.".to_owned(),
        }
        .normalize()
        .unwrap();
        assert_eq!(route.target_project_id, PROJECT_A);
    }

    #[test]
    fn normalizes_node_archive_and_delete_commands() {
        let archive: ArchiveCoordinationNode = serde_json::from_value(serde_json::json!({
            "command_id": " archive-node ",
            "actor": " local-user ",
            "expected_node_version": "3"
        }))
        .unwrap();
        let archive = archive.normalize().unwrap();
        assert_eq!(archive.command_id, "archive-node");
        assert_eq!(
            archive.preconditions(),
            CoordinationNodeArchivePreconditions {
                expected_node_version: 3,
                expected_worker_version: None,
            }
        );
        assert_eq!(
            ArchiveCoordinationNode {
                expected_worker_version: Some(0),
                ..archive
            }
            .normalize()
            .unwrap_err(),
            CoordinationNodeValidationError::InvalidVersion
        );

        let legacy: DeleteCoordinationNode = serde_json::from_value(serde_json::json!({
            "command_id": "delete-node",
            "actor": "local-user"
        }))
        .unwrap();
        assert_eq!(legacy.normalize().unwrap().archive, None);
        let zero = DeleteCoordinationNode {
            command_id: "delete-node".to_owned(),
            actor: "local-user".to_owned(),
            archive: Some(CoordinationNodeArchivePreconditions {
                expected_node_version: 0,
                expected_worker_version: None,
            }),
        };
        assert_eq!(
            zero.normalize().unwrap_err(),
            CoordinationNodeValidationError::InvalidVersion
        );
    }
}

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCollectionProgress {
    pub completed: usize,
    pub total: usize,
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
        CoordinationNodeKind, CoordinationNodeValidationError, CreateCoordinationNode,
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
}

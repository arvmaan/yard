use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;
const MAX_PROMPT_BYTES: usize = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRelationshipKind {
    DependsOn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRelationship {
    pub id: String,
    pub source_project_id: String,
    pub target_project_id: String,
    pub kind: ProjectRelationshipKind,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_by: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRelationships {
    pub relationships: Vec<ProjectRelationship>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProjectRelationship {
    pub command_id: String,
    pub actor: String,
    pub relationship_id: String,
    pub source_project_id: String,
    pub target_project_id: String,
    pub kind: ProjectRelationshipKind,
}

impl CreateProjectRelationship {
    /// Normalize and validate a project relationship creation command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationValidationError`] when a required identifier is
    /// blank or oversized, or when a project is related to itself.
    pub fn normalize(mut self) -> Result<Self, CoordinationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.relationship_id = required("relationship_id", &self.relationship_id, MAX_ID_BYTES)?;
        self.source_project_id =
            required("source_project_id", &self.source_project_id, MAX_ID_BYTES)?;
        self.target_project_id =
            required("target_project_id", &self.target_project_id, MAX_ID_BYTES)?;
        if self.source_project_id == self.target_project_id {
            return Err(CoordinationValidationError::SelfRelationship);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedProjectRelationship {
    pub command_id: String,
    pub relationship: ProjectRelationship,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteProjectRelationship {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

impl DeleteProjectRelationship {
    /// Normalize and validate a project relationship deletion command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationValidationError`] when a required value is blank
    /// or oversized, or when the optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, CoordinationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_version == 0 {
            return Err(CoordinationValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedProjectRelationship {
    pub command_id: String,
    pub relationship_id: String,
    pub deleted_at_unix_ms: u64,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendYardOrchestratorRoute {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_version: u64,
    pub orchestrator_worker_id: String,
    pub target_project_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub target_orchestrator_worker_id: String,
    pub text: String,
}

impl SendYardOrchestratorRoute {
    /// Normalize and validate a Yard orchestrator route command.
    ///
    /// # Errors
    ///
    /// Returns [`CoordinationValidationError`] when required text is blank or
    /// oversized, or when an optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, CoordinationValidationError> {
        self.command_id = required("command_id", &self.command_id, MAX_ID_BYTES)?;
        self.actor = required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.orchestrator_worker_id = required(
            "orchestrator_worker_id",
            &self.orchestrator_worker_id,
            MAX_ID_BYTES,
        )?;
        self.target_project_id =
            required("target_project_id", &self.target_project_id, MAX_ID_BYTES)?;
        self.target_orchestrator_worker_id = required(
            "target_orchestrator_worker_id",
            &self.target_orchestrator_worker_id,
            MAX_ID_BYTES,
        )?;
        self.text = required("text", &self.text, MAX_PROMPT_BYTES)?;
        if self.expected_orchestrator_version == 0 || self.expected_project_version == 0 {
            return Err(CoordinationValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationCommandStatus {
    Pending,
    Submitted,
    Failed,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardOrchestratorRoute {
    pub command_id: String,
    pub actor: String,
    pub orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_orchestrator_version: u64,
    pub target_project_id: String,
    pub target_orchestrator_worker_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_project_version: u64,
    pub text: String,
    pub status: CoordinationCommandStatus,
    pub error_message: Option<String>,
    pub runtime_status: Option<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub submitted_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardOrchestratorRoutes {
    pub routes: Vec<YardOrchestratorRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoordinationValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("a project cannot have a relationship to itself")]
    SelfRelationship,
    #[error("expected versions must be greater than zero")]
    InvalidVersion,
}

fn required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, CoordinationValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoordinationValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(CoordinationValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        CoordinationValidationError, CreateProjectRelationship, DeleteProjectRelationship,
        ProjectRelationshipKind, SendYardOrchestratorRoute,
    };

    #[test]
    fn normalizes_relationship_and_route_commands() {
        let relationship = CreateProjectRelationship {
            command_id: " relationship-create ".to_owned(),
            actor: " local-user ".to_owned(),
            relationship_id: " relationship-1 ".to_owned(),
            source_project_id: " project-source ".to_owned(),
            target_project_id: " project-target ".to_owned(),
            kind: ProjectRelationshipKind::DependsOn,
        }
        .normalize()
        .unwrap();
        assert_eq!(relationship.relationship_id, "relationship-1");

        let route = SendYardOrchestratorRoute {
            command_id: " route-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_orchestrator_version: 2,
            orchestrator_worker_id: " yard-worker ".to_owned(),
            target_project_id: " project-target ".to_owned(),
            expected_project_version: 3,
            target_orchestrator_worker_id: " project-worker ".to_owned(),
            text: " Coordinate this dependency. ".to_owned(),
        }
        .normalize()
        .unwrap();
        assert_eq!(route.text, "Coordinate this dependency.");
    }

    #[test]
    fn rejects_self_relationships_and_zero_versions() {
        let relationship = CreateProjectRelationship {
            command_id: "relationship-create".to_owned(),
            actor: "local-user".to_owned(),
            relationship_id: "relationship-1".to_owned(),
            source_project_id: "project-1".to_owned(),
            target_project_id: "project-1".to_owned(),
            kind: ProjectRelationshipKind::DependsOn,
        }
        .normalize()
        .unwrap_err();
        assert_eq!(relationship, CoordinationValidationError::SelfRelationship);

        let deletion = DeleteProjectRelationship {
            command_id: "relationship-delete".to_owned(),
            actor: "local-user".to_owned(),
            expected_version: 0,
        }
        .normalize()
        .unwrap_err();
        assert_eq!(deletion, CoordinationValidationError::InvalidVersion);

        let route = SendYardOrchestratorRoute {
            command_id: "route-1".to_owned(),
            actor: "local-user".to_owned(),
            expected_orchestrator_version: 2,
            orchestrator_worker_id: "yard-worker".to_owned(),
            target_project_id: "project-target".to_owned(),
            expected_project_version: 0,
            target_orchestrator_worker_id: "project-worker".to_owned(),
            text: "Coordinate this dependency.".to_owned(),
        }
        .normalize()
        .unwrap_err();
        assert_eq!(route, CoordinationValidationError::InvalidVersion);
    }

    #[test]
    fn relationship_kind_uses_snake_case_json() {
        assert_eq!(
            serde_json::to_string(&ProjectRelationshipKind::DependsOn).unwrap(),
            "\"depends_on\""
        );
        assert_eq!(
            serde_json::to_string(&super::CoordinationCommandStatus::Submitted).unwrap(),
            "\"submitted\""
        );
    }
}

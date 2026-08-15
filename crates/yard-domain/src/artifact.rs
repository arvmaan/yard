use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_ARTIFACT_CONTENT_BYTES: usize = 1_048_576;

const MAX_ARTIFACT_ACTOR_BYTES: usize = 120;
const MAX_ARTIFACT_DISPLAY_NAME_BYTES: usize = 240;
const MAX_ARTIFACT_ID_BYTES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Markdown,
    Html,
}

impl ArtifactKind {
    #[must_use]
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::Html => "text/html",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSource {
    Upload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub project_id: String,
    pub assignment_id: String,
    pub attempt_id: String,
    pub worker_id: String,
    pub kind: ArtifactKind,
    pub media_type: String,
    pub display_name: String,
    pub byte_size: u64,
    pub sha256: String,
    pub source: ArtifactSource,
    pub created_by: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactContent {
    pub artifact: Artifact,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadArtifact {
    pub actor: String,
    pub attempt_id: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_assignment_version: u64,
    #[serde(with = "crate::serde_u64")]
    pub expected_attempt_version: u64,
    pub kind: ArtifactKind,
    pub display_name: String,
    pub content: String,
}

impl UploadArtifact {
    /// Normalize and validate a user-supplied Markdown or HTML artifact.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactValidationError`] for blank or oversized values,
    /// invalid display names, empty content, or zero optimistic versions.
    pub fn normalize(mut self) -> Result<Self, ArtifactValidationError> {
        self.actor = required_bounded("actor", &self.actor, MAX_ARTIFACT_ACTOR_BYTES)?;
        self.attempt_id = required_bounded("attempt_id", &self.attempt_id, MAX_ARTIFACT_ID_BYTES)?;
        self.display_name = required_bounded(
            "display_name",
            &self.display_name,
            MAX_ARTIFACT_DISPLAY_NAME_BYTES,
        )?;
        if self.display_name == "."
            || self.display_name == ".."
            || self.display_name.contains(['/', '\\', '\0'])
        {
            return Err(ArtifactValidationError::InvalidDisplayName);
        }
        if self.expected_assignment_version == 0 || self.expected_attempt_version == 0 {
            return Err(ArtifactValidationError::InvalidVersion);
        }
        if self.content.is_empty() {
            return Err(ArtifactValidationError::EmptyContent);
        }
        if self.content.len() > MAX_ARTIFACT_CONTENT_BYTES {
            return Err(ArtifactValidationError::ContentTooLarge);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRegistration {
    pub actor: String,
    pub attempt_id: String,
    pub expected_assignment_version: u64,
    pub expected_attempt_version: u64,
    pub kind: ArtifactKind,
    pub display_name: String,
    pub byte_size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArtifactValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("artifact display names must be a single file name")]
    InvalidDisplayName,
    #[error("artifact versions must be greater than zero")]
    InvalidVersion,
    #[error("artifact content must not be empty")]
    EmptyContent,
    #[error("artifact content must be at most {MAX_ARTIFACT_CONTENT_BYTES} bytes")]
    ContentTooLarge,
}

fn required_bounded(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, ArtifactValidationError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(ArtifactValidationError::Required(field));
    }
    if value.len() > max {
        return Err(ArtifactValidationError::TooLong { field, max });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactKind, ArtifactValidationError, MAX_ARTIFACT_CONTENT_BYTES, UploadArtifact,
    };

    fn upload() -> UploadArtifact {
        UploadArtifact {
            actor: " local-user ".to_owned(),
            attempt_id: " attempt-1 ".to_owned(),
            expected_assignment_version: 2,
            expected_attempt_version: 3,
            kind: ArtifactKind::Markdown,
            display_name: " report.md ".to_owned(),
            content: "# Result".to_owned(),
        }
    }

    #[test]
    fn normalizes_valid_uploads() {
        let upload = upload().normalize().unwrap();

        assert_eq!(upload.actor, "local-user");
        assert_eq!(upload.attempt_id, "attempt-1");
        assert_eq!(upload.display_name, "report.md");
        assert_eq!(upload.kind.media_type(), "text/markdown");
    }

    #[test]
    fn rejects_paths_and_oversized_content() {
        let mut path = upload();
        path.display_name = "../report.md".to_owned();
        assert_eq!(
            path.normalize().unwrap_err(),
            ArtifactValidationError::InvalidDisplayName
        );

        let mut oversized = upload();
        oversized.content = "x".repeat(MAX_ARTIFACT_CONTENT_BYTES + 1);
        assert_eq!(
            oversized.normalize().unwrap_err(),
            ArtifactValidationError::ContentTooLarge
        );
    }
}

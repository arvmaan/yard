use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;
use yard_domain::{
    Artifact, ArtifactContent, ArtifactRegistration, ArtifactValidationError,
    MAX_ARTIFACT_CONTENT_BYTES, UploadArtifact,
};
use yard_store::{ProjectStoreError, YardStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    pub artifact: Artifact,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct ArtifactService {
    root: Arc<PathBuf>,
    store: Arc<dyn YardStore>,
}

impl ArtifactService {
    #[must_use]
    pub fn new(root: PathBuf, store: Arc<dyn YardStore>) -> Self {
        Self {
            root: Arc::new(root),
            store,
        }
    }

    /// Persist content and register immutable artifact metadata.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactServiceError`] for invalid input, stale assignment
    /// state, object collisions, storage failures, or metadata conflicts.
    pub async fn put(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
        upload: UploadArtifact,
    ) -> Result<StoredArtifact, ArtifactServiceError> {
        let artifact_id = canonical_artifact_id(artifact_id)?;
        let upload = upload.normalize()?;
        let sha256 = sha256_hex(upload.content.as_bytes());
        let byte_size = u64::try_from(upload.content.len())
            .map_err(|_| ArtifactServiceError::ContentInvalid)?;
        let registration = ArtifactRegistration {
            actor: upload.actor,
            attempt_id: upload.attempt_id,
            expected_assignment_version: upload.expected_assignment_version,
            expected_attempt_version: upload.expected_attempt_version,
            kind: upload.kind,
            display_name: upload.display_name,
            byte_size,
            sha256: sha256.clone(),
        };

        match self
            .store
            .get_artifact(project_id, assignment_id, &artifact_id)
            .await
        {
            Ok(artifact) => {
                ensure_registration_match(&artifact, &registration)?;
                self.verify_content(&artifact, Some(upload.content.as_bytes()))
                    .await?;
                return Ok(StoredArtifact {
                    artifact,
                    replayed: true,
                });
            }
            Err(ProjectStoreError::ArtifactNotFound) => {}
            Err(error) => return Err(error.into()),
        }

        let path = self.object_path(&artifact_id);
        let created = write_object(&path, upload.content.as_bytes(), &sha256).await?;
        match self
            .store
            .register_artifact(project_id, assignment_id, &artifact_id, registration)
            .await
        {
            Ok(artifact) => Ok(StoredArtifact {
                artifact,
                replayed: false,
            }),
            Err(error) => {
                if created
                    && self
                        .store
                        .get_artifact(project_id, assignment_id, &artifact_id)
                        .await
                        .is_err()
                {
                    let _ = fs::remove_file(path).await;
                }
                Err(error.into())
            }
        }
    }

    /// Read metadata for one assignment-scoped artifact.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactServiceError`] when the artifact does not exist or
    /// storage is unavailable.
    pub async fn get(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
    ) -> Result<Artifact, ArtifactServiceError> {
        let artifact_id = canonical_artifact_id(artifact_id)?;
        self.store
            .get_artifact(project_id, assignment_id, &artifact_id)
            .await
            .map_err(Into::into)
    }

    /// Read and verify one assignment-scoped artifact's UTF-8 content.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactServiceError`] when metadata or managed content is
    /// missing, malformed, oversized, or hash-mismatched.
    pub async fn content(
        &self,
        project_id: &str,
        assignment_id: &str,
        artifact_id: &str,
    ) -> Result<ArtifactContent, ArtifactServiceError> {
        let artifact = self.get(project_id, assignment_id, artifact_id).await?;
        let bytes = self.verify_content(&artifact, None).await?;
        let content = String::from_utf8(bytes).map_err(|_| ArtifactServiceError::ContentNotUtf8)?;
        Ok(ArtifactContent { artifact, content })
    }

    async fn verify_content(
        &self,
        artifact: &Artifact,
        expected: Option<&[u8]>,
    ) -> Result<Vec<u8>, ArtifactServiceError> {
        let bytes = fs::read(self.object_path(&artifact.id))
            .await
            .map_err(|error| {
                if error.kind() == ErrorKind::NotFound {
                    ArtifactServiceError::ContentMissing
                } else {
                    ArtifactServiceError::Io(error)
                }
            })?;
        if bytes.is_empty() || bytes.len() > MAX_ARTIFACT_CONTENT_BYTES {
            return Err(ArtifactServiceError::ContentInvalid);
        }
        if u64::try_from(bytes.len()).ok() != Some(artifact.byte_size)
            || sha256_hex(&bytes) != artifact.sha256
            || expected.is_some_and(|expected| expected != bytes)
        {
            return Err(ArtifactServiceError::ContentMismatch);
        }
        Ok(bytes)
    }

    fn object_path(&self, artifact_id: &str) -> PathBuf {
        self.root.join(&artifact_id[..2]).join(artifact_id)
    }
}

fn canonical_artifact_id(value: &str) -> Result<String, ArtifactServiceError> {
    let parsed = Uuid::parse_str(value).map_err(|_| ArtifactServiceError::InvalidArtifactId)?;
    let canonical = parsed.to_string();
    if value != canonical {
        return Err(ArtifactServiceError::InvalidArtifactId);
    }
    Ok(canonical)
}

fn ensure_registration_match(
    artifact: &Artifact,
    registration: &ArtifactRegistration,
) -> Result<(), ArtifactServiceError> {
    if artifact.attempt_id != registration.attempt_id
        || artifact.kind != registration.kind
        || artifact.media_type != registration.kind.media_type()
        || artifact.display_name != registration.display_name
        || artifact.byte_size != registration.byte_size
        || artifact.sha256 != registration.sha256
        || artifact.created_by != registration.actor
    {
        return Err(ArtifactServiceError::ArtifactIdConflict);
    }
    Ok(())
}

async fn write_object(
    path: &Path,
    content: &[u8],
    expected_sha256: &str,
) -> Result<bool, ArtifactServiceError> {
    let parent = path
        .parent()
        .ok_or_else(|| ArtifactServiceError::Io(std::io::Error::other("invalid object path")))?;
    fs::create_dir_all(parent).await?;
    if fs::try_exists(path).await? {
        verify_existing_object(path, content, expected_sha256).await?;
        return Ok(false);
    }

    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        Uuid::now_v7()
    ));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        file.write_all(content).await?;
        file.sync_all().await?;
        match fs::hard_link(&temporary, path).await {
            Ok(()) => {
                // Metadata is committed after this returns, so make the new
                // directory entry durable before SQLite can reference it.
                sync_directory(parent).await?;
                Ok(true)
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                verify_existing_object(path, content, expected_sha256).await?;
                Ok(false)
            }
            Err(error) => Err(ArtifactServiceError::Io(error)),
        }
    }
    .await;
    let _ = fs::remove_file(&temporary).await;
    result
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> Result<(), ArtifactServiceError> {
    let directory = fs::File::open(path).await?;
    directory.sync_all().await?;
    Ok(())
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> Result<(), ArtifactServiceError> {
    Ok(())
}

async fn verify_existing_object(
    path: &Path,
    content: &[u8],
    expected_sha256: &str,
) -> Result<(), ArtifactServiceError> {
    let existing = fs::read(path).await?;
    if existing != content || sha256_hex(&existing) != expected_sha256 {
        return Err(ArtifactServiceError::ArtifactIdConflict);
    }
    Ok(())
}

fn sha256_hex(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

#[derive(Debug, Error)]
pub enum ArtifactServiceError {
    #[error(transparent)]
    InvalidArtifact(#[from] ArtifactValidationError),
    #[error("artifact IDs must be canonical UUIDs")]
    InvalidArtifactId,
    #[error("artifact ID is already associated with different content or provenance")]
    ArtifactIdConflict,
    #[error("artifact content is missing from managed storage")]
    ContentMissing,
    #[error("artifact content is invalid")]
    ContentInvalid,
    #[error("artifact content does not match its durable metadata")]
    ContentMismatch,
    #[error("artifact content is not UTF-8")]
    ContentNotUtf8,
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

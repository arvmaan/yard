use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Worker;

const MAX_COMMAND_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerAvailability {
    YardOrchestrator,
    CoordinationNode,
    Orchestrator,
    Assigned,
    UnassignedLive,
    Resumable,
    Unavailable,
    Ambiguous,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerCandidate {
    pub worker: Worker,
    pub profile_name: Option<String>,
    pub default_role: Option<String>,
    pub availability: WorkerAvailability,
    pub project_id: Option<String>,
    pub assignment_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerCandidates {
    pub workers: Vec<WorkerCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletedRuntimeRetentionReason {
    CleanupPolicyUnavailable,
    ApprovalAuthorityUnavailable,
    TerminalLeaseFenceUnavailable,
    ObservationGenerationFenceUnavailable,
    AtomicCloseUnavailable,
    GracePolicyUnavailable,
    NoLinkedArtifacts,
    UnresolvedCompletionBlockers,
    PendingAssignmentIntervention,
    NewActiveAssignment,
    ProtectedOrchestrator,
    NotYardOwned,
    ProjectOwnershipMismatch,
    ParentOwnershipMismatch,
    GraceNotElapsed,
    HandoffInProgress,
    CoordinationNode,
    Pinned,
    RuntimeConflict,
    ObservationFailure,
    CleanupAdvisorArtifactMissing,
    CleanupAdvisorRecursionPrevented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedRuntimeCleanupCandidate {
    pub worker_id: String,
    pub profile_name: String,
    /// The user-chosen worker name, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub project_id: String,
    pub project_name: String,
    pub assignment_id: String,
    pub role: String,
    pub completion_receipt_id: String,
    pub completed_at_unix_ms: u64,
    pub linked_artifact_count: usize,
    pub close_eligible: bool,
    pub retained_reasons: Vec<CompletedRuntimeRetentionReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedRuntimeCleanupPreview {
    pub candidate_count: usize,
    pub close_ready_count: usize,
    pub limit: usize,
    pub truncated: bool,
    pub candidates: Vec<CompletedRuntimeCleanupCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndWorkerSession {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
    #[serde(default, with = "crate::serde_u64::option")]
    pub expected_runtime_version: Option<u64>,
}

impl EndWorkerSession {
    /// Normalize and validate an explicit worker session termination command.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerSessionValidationError`] when a required value is blank
    /// or oversized, or an optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, WorkerSessionValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_worker_version == 0 || self.expected_runtime_version == Some(0) {
            return Err(WorkerSessionValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndedWorkerSession {
    pub command_id: String,
    pub worker: Worker,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteWorker {
    pub command_id: String,
    pub actor: String,
    #[serde(with = "crate::serde_u64")]
    pub expected_worker_version: u64,
}

impl DeleteWorker {
    /// Normalize and validate an irreversible worker visibility deletion.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerSessionValidationError`] when a required value is
    /// blank or oversized, or the optimistic version is zero.
    pub fn normalize(mut self) -> Result<Self, WorkerSessionValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        if self.expected_worker_version == 0 {
            return Err(WorkerSessionValidationError::InvalidVersion);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedWorker {
    pub command_id: String,
    pub worker_id: String,
    pub deleted_at_unix_ms: u64,
    pub cleanup_pending: bool,
    pub replayed: bool,
}

/// The longest worker display name, counted in characters (not bytes).
pub const MAX_WORKER_DISPLAY_NAME_CHARS: usize = 64;

/// Rename (or reset) a worker's user-chosen display name.
///
/// `expected_display_name` is the name the caller saw; the rename applies only
/// while the stored name still equals it. `display_name = None` (or blank)
/// clears the name so the worker shows its default label again. A rename is
/// cosmetic and never changes the worker's optimistic version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameWorker {
    pub command_id: String,
    pub actor: String,
    #[serde(default)]
    pub expected_display_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl RenameWorker {
    /// Normalize and validate a worker rename command.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerNameValidationError`] when the command id or actor is
    /// blank or oversized, or either name fails
    /// [`normalize_worker_display_name`].
    pub fn normalize(mut self) -> Result<Self, WorkerNameValidationError> {
        self.command_id = bounded_required("command_id", &self.command_id, MAX_COMMAND_ID_BYTES)?;
        self.actor = bounded_required("actor", &self.actor, MAX_ACTOR_BYTES)?;
        self.expected_display_name = normalize_display_name_field(
            "expected_display_name",
            self.expected_display_name.as_deref(),
        )?;
        self.display_name =
            normalize_display_name_field("display_name", self.display_name.as_deref())?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenamedWorker {
    pub command_id: String,
    pub worker: Worker,
    pub replayed: bool,
}

/// Normalize a user-chosen worker display name.
///
/// Trims surrounding whitespace; a blank value becomes `None` (reset to the
/// default label). Rejects names longer than
/// [`MAX_WORKER_DISPLAY_NAME_CHARS`] characters, control characters
/// (including newlines and tabs), bidirectional override or isolate
/// characters (U+202A–U+202E, U+2066–U+2069), other invisible format
/// characters (general category Cf, e.g. U+200B, U+FEFF, U+200F) and the
/// line/paragraph separators U+2028/U+2029.
///
/// # Errors
///
/// Returns [`WorkerNameValidationError`] when the trimmed name is too long or
/// contains a rejected character.
pub fn normalize_worker_display_name(
    value: Option<&str>,
) -> Result<Option<String>, WorkerNameValidationError> {
    normalize_display_name_field("display_name", value)
}

fn normalize_display_name_field(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<String>, WorkerNameValidationError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > MAX_WORKER_DISPLAY_NAME_CHARS {
        return Err(WorkerNameValidationError::TooLong {
            field,
            max_chars: MAX_WORKER_DISPLAY_NAME_CHARS,
        });
    }
    if value
        .chars()
        .any(|character| character.is_control() || is_invisible_format(character))
    {
        return Err(WorkerNameValidationError::InvalidCharacter { field });
    }
    Ok(Some(value.to_owned()))
}

/// Unicode general categories Cf (format; includes the bidi embedding,
/// override, isolate and mark characters), Zl and Zp. They render invisibly
/// or as forced line breaks, so a name made of them looks blank and a name
/// containing them can impersonate another or break onto two lines.
const fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkerNameValidationError {
    #[error(transparent)]
    Command(#[from] WorkerSessionValidationError),
    #[error("{field} must be at most {max_chars} characters")]
    TooLong {
        field: &'static str,
        max_chars: usize,
    },
    #[error(
        "{field} must not contain control, invisible formatting or bidirectional override characters"
    )]
    InvalidCharacter { field: &'static str },
}

impl WorkerNameValidationError {
    /// Whether the error is about a name (rather than the command envelope).
    #[must_use]
    pub const fn is_name_error(&self) -> bool {
        matches!(self, Self::TooLong { .. } | Self::InvalidCharacter { .. })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkerSessionValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },
    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },
    #[error("optimistic versions must be greater than zero")]
    InvalidVersion,
}

fn bounded_required(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, WorkerSessionValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(WorkerSessionValidationError::Required { field });
    }
    if value.len() > max_bytes {
        return Err(WorkerSessionValidationError::TooLong { field, max_bytes });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_WORKER_DISPLAY_NAME_CHARS, RenameWorker, WorkerNameValidationError,
        WorkerSessionValidationError, normalize_worker_display_name,
    };

    fn rename(display_name: Option<&str>) -> RenameWorker {
        RenameWorker {
            command_id: " rename-1 ".to_owned(),
            actor: " local-user ".to_owned(),
            expected_display_name: Some("  ".to_owned()),
            display_name: display_name.map(str::to_owned),
        }
    }

    #[test]
    fn display_name_is_trimmed_and_blank_clears() {
        assert_eq!(
            normalize_worker_display_name(Some("  BAR CDK  ")).unwrap(),
            Some("BAR CDK".to_owned())
        );
        assert_eq!(normalize_worker_display_name(Some(" \t ")).unwrap(), None);
        assert_eq!(normalize_worker_display_name(Some("")).unwrap(), None);
        assert_eq!(normalize_worker_display_name(None).unwrap(), None);
    }

    #[test]
    fn display_name_limit_counts_characters_not_bytes() {
        let multibyte = "é".repeat(MAX_WORKER_DISPLAY_NAME_CHARS);
        assert!(multibyte.len() > MAX_WORKER_DISPLAY_NAME_CHARS);
        assert_eq!(
            normalize_worker_display_name(Some(&multibyte)).unwrap(),
            Some(multibyte.clone())
        );
        let emoji = "\u{1F980}".repeat(MAX_WORKER_DISPLAY_NAME_CHARS);
        assert!(normalize_worker_display_name(Some(&emoji)).is_ok());
        let too_long = format!("{multibyte}a");
        assert_eq!(
            normalize_worker_display_name(Some(&too_long)),
            Err(WorkerNameValidationError::TooLong {
                field: "display_name",
                max_chars: MAX_WORKER_DISPLAY_NAME_CHARS,
            })
        );
    }

    #[test]
    fn display_name_rejects_control_and_bidi_characters() {
        for value in [
            "BAR\nCDK",
            "BAR\tCDK",
            "BAR\u{7}CDK",
            "BAR\u{7F}CDK",
            "BAR\u{85}CDK",
            "BAR\u{202A}CDK",
            "BAR\u{202E}CDK",
            "BAR\u{2066}CDK",
            "BAR\u{2069}CDK",
            // Invisible format characters (Cf) and line/paragraph separators.
            "\u{200B}",
            "\u{200B}\u{2060}",
            "\u{200F}",
            "BAR\u{200B}CDK",
            "BAR\u{200E}CDK",
            "BAR\u{061C}CDK",
            "BAR\u{180E}CDK",
            "BAR\u{2060}CDK",
            "BAR\u{2028}CDK",
            "BAR\u{2029}CDK",
            "BAR\u{FEFF}",
            "\u{FEFF}BAR",
            "BAR\u{E0041}",
        ] {
            let error = normalize_worker_display_name(Some(value)).unwrap_err();
            assert_eq!(
                error,
                WorkerNameValidationError::InvalidCharacter {
                    field: "display_name"
                },
                "{value:?}"
            );
            assert!(error.is_name_error());
        }
        // Ordinary punctuation and non-Latin scripts are fine.
        assert!(normalize_worker_display_name(Some("Généraliste · 検索 (#2)")).is_ok());
    }

    #[test]
    fn rename_command_normalizes_both_names_and_the_envelope() {
        let command = rename(Some("  Reviewer  ")).normalize().unwrap();
        assert_eq!(command.command_id, "rename-1");
        assert_eq!(command.actor, "local-user");
        assert_eq!(command.expected_display_name, None);
        assert_eq!(command.display_name, Some("Reviewer".to_owned()));

        let cleared = rename(Some("   ")).normalize().unwrap();
        assert_eq!(cleared.display_name, None);

        let mut blank_actor = rename(None);
        blank_actor.actor = " ".to_owned();
        let error = blank_actor.normalize().unwrap_err();
        assert_eq!(
            error,
            WorkerNameValidationError::Command(WorkerSessionValidationError::Required {
                field: "actor"
            })
        );
        assert!(!error.is_name_error());

        let mut bad_expected = rename(None);
        bad_expected.expected_display_name = Some("a\nb".to_owned());
        assert_eq!(
            bad_expected.normalize(),
            Err(WorkerNameValidationError::InvalidCharacter {
                field: "expected_display_name"
            })
        );
    }

    #[test]
    fn rename_command_names_default_to_none_when_omitted() {
        let command: RenameWorker =
            serde_json::from_str(r#"{"command_id":"rename-1","actor":"local-user"}"#).unwrap();
        assert_eq!(command.expected_display_name, None);
        assert_eq!(command.display_name, None);
    }

    #[test]
    fn worker_payloads_without_display_name_still_parse() {
        let worker: crate::Worker = serde_json::from_value(serde_json::json!({
            "id": "worker-1",
            "profile_id": null,
            "desired_state": "running",
            "runtime": null,
            "version": "3",
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
        }))
        .unwrap();
        assert_eq!(worker.display_name, None);
        let serialized = serde_json::to_value(&worker).unwrap();
        assert!(serialized["display_name"].is_null());
        assert!(serialized.as_object().unwrap().contains_key("display_name"));
    }
}

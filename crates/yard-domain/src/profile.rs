use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_NAME_BYTES: usize = 120;
const MAX_VALUE_BYTES: usize = 512;
const MAX_LIST_ITEMS: usize = 64;
const MAX_HERDR_AGENT_NAME_BYTES: usize = 29;
const HERDR_AGENT_NAME_PREFIX: &str = "yard-";
const HERDR_AGENT_NAME_HASH_CHARS: usize = 16;

#[must_use]
pub fn herdr_agent_name(command_id: &str) -> String {
    let readable_bytes = MAX_HERDR_AGENT_NAME_BYTES
        - HERDR_AGENT_NAME_PREFIX.len()
        - HERDR_AGENT_NAME_HASH_CHARS
        - 1;
    let readable: String = command_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .take(readable_bytes)
        .collect();
    let readable = if readable.is_empty() {
        "worker"
    } else {
        readable.as_str()
    };
    let hash = stable_command_id_hash(command_id);
    format!("{HERDR_AGENT_NAME_PREFIX}{readable}-{hash:016x}")
}

fn stable_command_id_hash(command_id: &str) -> u64 {
    command_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerProfileSpec {
    pub name: String,
    pub runtime_adapter: String,
    pub provider: String,
    pub model: Option<String>,
    pub default_role: String,
    pub instructions_ref: Option<String>,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub sandbox_policy: String,
    pub worktree_policy: String,
    pub permission_policy: String,
    pub completion_contract: String,
}

impl WorkerProfileSpec {
    /// Normalize user-entered profile values and validate bounded list fields.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileValidationError`] for blank required values, values
    /// that are too long, or oversized lists.
    pub fn normalize(mut self) -> Result<Self, ProfileValidationError> {
        self.name = required("name", &self.name, MAX_NAME_BYTES)?;
        self.runtime_adapter = required("runtime_adapter", &self.runtime_adapter, MAX_VALUE_BYTES)?;
        self.provider = required("provider", &self.provider, MAX_VALUE_BYTES)?;
        self.model = optional("model", self.model, MAX_VALUE_BYTES)?;
        self.default_role = required("default_role", &self.default_role, MAX_VALUE_BYTES)?;
        self.instructions_ref =
            optional("instructions_ref", self.instructions_ref, MAX_VALUE_BYTES)?;
        self.tools = normalize_list("tools", self.tools)?;
        self.skills = normalize_list("skills", self.skills)?;
        self.mcp_servers = normalize_list("mcp_servers", self.mcp_servers)?;
        self.sandbox_policy = required("sandbox_policy", &self.sandbox_policy, MAX_VALUE_BYTES)?;
        self.worktree_policy = required("worktree_policy", &self.worktree_policy, MAX_VALUE_BYTES)?;
        self.permission_policy = required(
            "permission_policy",
            &self.permission_policy,
            MAX_VALUE_BYTES,
        )?;
        self.completion_contract = required(
            "completion_contract",
            &self.completion_contract,
            MAX_VALUE_BYTES,
        )?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerProfile {
    pub id: String,
    #[serde(flatten)]
    pub spec: WorkerProfileSpec,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerProfiles {
    pub profiles: Vec<WorkerProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkerProfile {
    #[serde(flatten)]
    pub spec: WorkerProfileSpec,
}

impl CreateWorkerProfile {
    /// Normalize a new worker profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileValidationError`] when the profile is invalid.
    pub fn normalize(mut self) -> Result<Self, ProfileValidationError> {
        self.spec = self.spec.normalize()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateWorkerProfile {
    #[serde(flatten)]
    pub spec: WorkerProfileSpec,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

impl UpdateWorkerProfile {
    /// Normalize a profile replacement and validate its optimistic version.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileValidationError`] when the version or profile is
    /// invalid.
    pub fn normalize(mut self) -> Result<Self, ProfileValidationError> {
        if self.expected_version == 0 {
            return Err(ProfileValidationError::InvalidVersion);
        }
        self.spec = self.spec.normalize()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProfileValidationError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("{field} must contain at most {max} values")]
    TooManyValues { field: &'static str, max: usize },
    #[error("expected_version must be greater than zero")]
    InvalidVersion,
}

fn required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, ProfileValidationError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(ProfileValidationError::Required(field));
    }
    if value.len() > max {
        return Err(ProfileValidationError::TooLong { field, max });
    }
    Ok(value)
}

fn optional(
    field: &'static str,
    value: Option<String>,
    max: usize,
) -> Result<Option<String>, ProfileValidationError> {
    value.map(|value| required(field, &value, max)).transpose()
}

fn normalize_list(
    field: &'static str,
    values: Vec<String>,
) -> Result<Vec<String>, ProfileValidationError> {
    if values.len() > MAX_LIST_ITEMS {
        return Err(ProfileValidationError::TooManyValues {
            field,
            max: MAX_LIST_ITEMS,
        });
    }
    values
        .into_iter()
        .map(|value| required(field, &value, MAX_VALUE_BYTES))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CreateWorkerProfile, ProfileValidationError, UpdateWorkerProfile, WorkerProfileSpec,
        herdr_agent_name,
    };

    fn spec() -> WorkerProfileSpec {
        WorkerProfileSpec {
            name: " Implementer ".to_owned(),
            runtime_adapter: " herdr ".to_owned(),
            provider: " codex ".to_owned(),
            model: Some(" gpt-5.4 ".to_owned()),
            default_role: " implementation ".to_owned(),
            instructions_ref: None,
            tools: vec![" shell ".to_owned()],
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            sandbox_policy: " runtime_default ".to_owned(),
            worktree_policy: " project_workspace ".to_owned(),
            permission_policy: " runtime_default ".to_owned(),
            completion_contract: " manual_receipt ".to_owned(),
        }
    }

    #[test]
    fn normalizes_profile_fields() {
        let profile = CreateWorkerProfile { spec: spec() }.normalize().unwrap();

        assert_eq!(profile.spec.name, "Implementer");
        assert_eq!(profile.spec.provider, "codex");
        assert_eq!(profile.spec.tools, ["shell"]);
    }

    #[test]
    fn rejects_zero_update_version() {
        let error = UpdateWorkerProfile {
            spec: spec(),
            expected_version: 0,
        }
        .normalize()
        .unwrap_err();

        assert_eq!(error, ProfileValidationError::InvalidVersion);
    }

    #[test]
    fn derives_bounded_herdr_agent_name_from_command_id() {
        assert_eq!(
            herdr_agent_name("B7204888-76C8-4159-BB53-5F24FEAF7AC5"),
            "yard-b720488-b9b3f88531b588d8"
        );
        assert_eq!(herdr_agent_name("---"), "yard-worker-de7cc417de1b3246");
    }

    #[test]
    fn distinguishes_command_ids_with_shared_normalized_prefixes() {
        let first = herdr_agent_name("allocation-command-with-a-shared-prefix-0001");
        let second = herdr_agent_name("allocation-command-with-a-shared-prefix-0002");

        assert_ne!(first, second);
        assert!(first.len() <= super::MAX_HERDR_AGENT_NAME_BYTES);
        assert!(second.len() <= super::MAX_HERDR_AGENT_NAME_BYTES);
    }

    #[test]
    fn distinguishes_command_ids_that_normalize_to_the_same_text() {
        let names = [
            herdr_agent_name("Command-ID"),
            herdr_agent_name("command_id"),
            herdr_agent_name("COMMAND ID"),
        ];

        assert_eq!(
            names.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
        assert!(
            names
                .iter()
                .all(|name| name.len() <= super::MAX_HERDR_AGENT_NAME_BYTES)
        );
    }
}

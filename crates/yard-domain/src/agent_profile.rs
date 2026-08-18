use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::WorkerProfileSpec;

pub const AGENT_PROFILE_API_VERSION: &str = "yard.dev/agent-profile/v1alpha1";
pub const AGENT_PROFILE_KIND: &str = "AgentProfile";
pub const HERDR_EXTENSION_KEY: &str = "dev.yard.herdr";
pub const WORKER_PROFILE_EXTENSION_KEY: &str = "dev.yard.worker-profile";

const HERDR_EXTENSION_API_VERSION: &str = "yard.dev/adapters/herdr/v1alpha1";
const WORKER_PROFILE_EXTENSION_API_VERSION: &str = "yard.dev/compatibility/worker-profile/v1alpha1";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_NAME_BYTES: usize = 120;
const MAX_VALUE_BYTES: usize = 512;
const MAX_LIST_ITEMS: usize = 128;
const MAX_ARTIFACTS: usize = 256;
const MAX_EXTENSIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileManifest {
    pub api_version: String,
    pub kind: String,
    pub metadata: AgentProfileMetadata,
    pub spec: AgentProfileSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<AgentProfileArtifact>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileMetadata {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provenance: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileSpec {
    pub role: AgentProfileRole,
    #[serde(default)]
    pub instructions: Vec<AgentProfileInstruction>,
    #[serde(default)]
    pub capabilities: AgentProfileCapabilities,
    #[serde(default)]
    pub components: AgentProfileComponents,
    pub policies: AgentProfilePolicies,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfileRole {
    pub default: String,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfileInstruction {
    pub id: String,
    pub source: AgentProfileSource,
    #[serde(default)]
    pub required: bool,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfileSource {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentProfileCapabilities {
    #[serde(default)]
    pub required: Vec<CapabilityRequest>,
    #[serde(default)]
    pub optional: Vec<CapabilityRequest>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequest {
    pub id: String,
    pub version: u32,
    #[serde(default)]
    pub allow_degraded: bool,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileComponents {
    #[serde(default)]
    pub skills: Vec<AgentProfileComponent>,
    #[serde(default)]
    pub tools: Vec<AgentProfileComponent>,
    #[serde(default)]
    pub mcp_servers: Vec<AgentProfileComponent>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileComponent {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AgentProfileSource>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_slots: Vec<CredentialSlot>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialSlot {
    pub id: String,
    pub kind: CredentialSlotKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialSlotKind {
    EnvironmentVariable,
    LocalSecretStore,
    OAuthBinding,
    CommandHelper,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfilePolicies {
    pub isolation: String,
    pub sandbox: String,
    pub permissions: String,
    pub completion: String,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileArtifact {
    pub path: String,
    pub media_type: String,
    pub sha256: String,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterDescriptor {
    pub adapter_id: String,
    pub adapter_version: String,
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    pub runtime_surface: String,
    #[serde(default)]
    pub capabilities: Vec<AdapterCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterCapability {
    pub id: String,
    pub min_version: u32,
    pub max_version: u32,
    pub status: CapabilitySupportStatus,
    pub reason: String,
    pub surface: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupportStatus {
    Supported,
    Degraded,
    ApprovalRequired,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequirement {
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityNegotiation {
    pub id: String,
    pub version: u32,
    pub requirement: CapabilityRequirement,
    pub allow_degraded: bool,
    pub status: CapabilitySupportStatus,
    pub reason: String,
    pub surface: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityNegotiationReport {
    pub adapter_id: String,
    pub adapter_version: String,
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    pub runtime_surface: String,
    pub compatible: bool,
    pub results: Vec<CapabilityNegotiation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateAgentProfile {
    pub manifest: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateAgentProfile {
    pub manifest: Value,
    #[serde(with = "crate::serde_u64")]
    pub expected_version: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: String,
    #[serde(with = "crate::serde_u64")]
    pub version: u64,
    pub manifest: Value,
    pub validation: CapabilityNegotiationReport,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfiles {
    pub profiles: Vec<AgentProfile>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedAgentProfile {
    pub original_manifest: Value,
    pub canonical_manifest: AgentProfileManifest,
    pub worker_profile: WorkerProfileSpec,
    pub validation: CapabilityNegotiationReport,
}

impl PreparedAgentProfile {
    #[must_use]
    pub fn from_worker_profile(spec: &WorkerProfileSpec) -> Self {
        Self::from_compatible_manifest(
            AgentProfileManifest::from_worker_profile(spec),
            spec.clone(),
        )
    }

    /// Overlay a legacy profile edit while preserving portable-only content.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileValidationError`] when the legacy edit makes a
    /// preserved required capability incompatible with the selected adapter.
    pub fn preserving_unknown(
        current: &AgentProfileManifest,
        spec: &WorkerProfileSpec,
    ) -> Result<Self, AgentProfileValidationError> {
        let prepared =
            Self::from_compatible_manifest(current.with_worker_profile(spec), spec.clone());
        ensure_required_capabilities_supported(&prepared.validation)?;
        Ok(prepared)
    }

    fn from_compatible_manifest(
        canonical_manifest: AgentProfileManifest,
        worker_profile: WorkerProfileSpec,
    ) -> Self {
        let validation =
            canonical_manifest.negotiate(&compatibility_adapter_descriptor(&worker_profile));
        let original_manifest = serde_json::to_value(&canonical_manifest)
            .expect("AgentProfileManifest serialization is infallible");
        Self {
            original_manifest,
            canonical_manifest,
            worker_profile,
            validation,
        }
    }
}

impl CreateAgentProfile {
    /// Validate an imported manifest and build its legacy compatibility projection.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileValidationError`] when the known core is invalid,
    /// a secret value is present, no `WorkerProfile` projection can be built, or
    /// an adapter cannot satisfy a required capability.
    pub fn prepare(self) -> Result<PreparedAgentProfile, AgentProfileValidationError> {
        prepare_manifest(self.manifest)
    }
}

impl UpdateAgentProfile {
    /// Validate a replacement manifest and its optimistic revision.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileValidationError`] for a zero expected revision or
    /// any invalid or incompatible manifest.
    pub fn prepare(self) -> Result<PreparedAgentProfile, AgentProfileValidationError> {
        if self.expected_version == 0 {
            return Err(AgentProfileValidationError::InvalidVersion);
        }
        prepare_manifest(self.manifest)
    }
}

impl AgentProfileManifest {
    #[must_use]
    pub fn from_worker_profile(spec: &WorkerProfileSpec) -> Self {
        let instructions = spec
            .instructions_ref
            .as_ref()
            .map_or_else(Vec::new, |reference| {
                vec![AgentProfileInstruction {
                    id: "worker-profile-instructions".to_owned(),
                    source: AgentProfileSource {
                        kind: "workspace".to_owned(),
                        path: Some(reference.clone()),
                        reference: None,
                        unknown: BTreeMap::new(),
                    },
                    required: false,
                    unknown: BTreeMap::new(),
                }]
            });
        let mut optional = Vec::new();
        if !spec.skills.is_empty() {
            optional.push(capability_request("agent.skills"));
        }
        if !spec.tools.is_empty() {
            optional.push(capability_request("agent.tools"));
        }
        if !spec.mcp_servers.is_empty() {
            optional.push(capability_request("tool.mcp.client"));
        }

        Self {
            api_version: AGENT_PROFILE_API_VERSION.to_owned(),
            kind: AGENT_PROFILE_KIND.to_owned(),
            metadata: AgentProfileMetadata {
                name: spec.name.clone(),
                package_version: None,
                provenance: BTreeMap::from([(
                    "importedFrom".to_owned(),
                    Value::String("yard-worker-profile".to_owned()),
                )]),
                unknown: BTreeMap::new(),
            },
            spec: AgentProfileSpec {
                role: AgentProfileRole {
                    default: spec.default_role.clone(),
                    unknown: BTreeMap::new(),
                },
                instructions,
                capabilities: AgentProfileCapabilities {
                    required: Vec::new(),
                    optional,
                    unknown: BTreeMap::new(),
                },
                components: AgentProfileComponents {
                    skills: legacy_components(&spec.skills),
                    tools: legacy_components(&spec.tools),
                    mcp_servers: legacy_components(&spec.mcp_servers),
                    unknown: BTreeMap::new(),
                },
                policies: AgentProfilePolicies {
                    isolation: spec.worktree_policy.clone(),
                    sandbox: spec.sandbox_policy.clone(),
                    permissions: spec.permission_policy.clone(),
                    completion: spec.completion_contract.clone(),
                    unknown: BTreeMap::new(),
                },
                extensions: worker_profile_extensions(spec),
                unknown: BTreeMap::new(),
            },
            artifacts: Vec::new(),
            unknown: BTreeMap::new(),
        }
    }

    /// Build the existing flat `WorkerProfile` projection used by v1 APIs and pins.
    ///
    /// # Errors
    ///
    /// Returns [`AgentProfileValidationError`] when no compatible runtime and
    /// provider selection is present or a flat field cannot be represented.
    pub fn worker_profile_projection(
        &self,
    ) -> Result<WorkerProfileSpec, AgentProfileValidationError> {
        let compatibility = self
            .spec
            .extensions
            .get(WORKER_PROFILE_EXTENSION_KEY)
            .map(parse_worker_profile_extension)
            .transpose()?;
        let (runtime_adapter, provider, model) = if let Some(compatibility) = compatibility {
            validate_compatibility_extension_consistency(&self.spec.extensions, &compatibility)?;
            compatibility
        } else {
            parse_herdr_projection(&self.spec.extensions)?
        };
        let instructions_ref = self.spec.instructions.first().and_then(|instruction| {
            instruction
                .source
                .path
                .clone()
                .or_else(|| instruction.source.reference.clone())
        });
        let projection = WorkerProfileSpec {
            name: self.metadata.name.clone(),
            runtime_adapter,
            provider,
            model,
            default_role: self.spec.role.default.clone(),
            instructions_ref,
            tools: component_ids(&self.spec.components.tools),
            skills: component_ids(&self.spec.components.skills),
            mcp_servers: component_ids(&self.spec.components.mcp_servers),
            sandbox_policy: legacy_policy(&self.spec.policies.sandbox),
            worktree_policy: legacy_policy(&self.spec.policies.isolation),
            permission_policy: legacy_permission_policy(&self.spec.policies.permissions),
            completion_contract: legacy_completion_policy(&self.spec.policies.completion),
        };
        projection.normalize().map_err(|error| {
            AgentProfileValidationError::InvalidWorkerProjection(error.to_string())
        })
    }

    #[must_use]
    pub fn with_worker_profile(&self, spec: &WorkerProfileSpec) -> Self {
        let generated = Self::from_worker_profile(spec);
        let mut updated = self.clone();
        updated.metadata.name = generated.metadata.name;
        updated.spec.role.default = generated.spec.role.default;
        updated.spec.instructions =
            merge_instructions(&updated.spec.instructions, generated.spec.instructions);
        for capability in generated.spec.capabilities.optional {
            if !updated
                .spec
                .capabilities
                .required
                .iter()
                .chain(&updated.spec.capabilities.optional)
                .any(|current| current.id == capability.id)
            {
                updated.spec.capabilities.optional.push(capability);
            }
        }
        updated.spec.components.skills = merge_components(
            &updated.spec.components.skills,
            generated.spec.components.skills,
        );
        updated.spec.components.tools = merge_components(
            &updated.spec.components.tools,
            generated.spec.components.tools,
        );
        updated.spec.components.mcp_servers = merge_components(
            &updated.spec.components.mcp_servers,
            generated.spec.components.mcp_servers,
        );
        updated.spec.policies.isolation = generated.spec.policies.isolation;
        updated.spec.policies.sandbox = generated.spec.policies.sandbox;
        updated.spec.policies.permissions = generated.spec.policies.permissions;
        updated.spec.policies.completion = generated.spec.policies.completion;
        for (key, value) in generated.spec.extensions {
            merge_extension(&mut updated.spec.extensions, key, value);
        }
        updated
    }

    /// Negotiate requested capabilities against a concrete adapter descriptor.
    #[must_use]
    pub fn negotiate(&self, descriptor: &AdapterDescriptor) -> CapabilityNegotiationReport {
        let mut results = Vec::new();
        let mut required_ids = BTreeSet::new();
        for request in &self.spec.capabilities.required {
            required_ids.insert(request.id.clone());
            results.push(negotiate_request(
                request,
                CapabilityRequirement::Required,
                descriptor,
            ));
        }
        add_required_component_capability(
            &mut results,
            &mut required_ids,
            &self.spec.components.skills,
            "agent.skills",
            descriptor,
        );
        add_required_component_capability(
            &mut results,
            &mut required_ids,
            &self.spec.components.tools,
            "agent.tools",
            descriptor,
        );
        add_required_component_capability(
            &mut results,
            &mut required_ids,
            &self.spec.components.mcp_servers,
            "tool.mcp.client",
            descriptor,
        );
        for instruction in self
            .spec
            .instructions
            .iter()
            .filter(|instruction| instruction.required)
        {
            let capability_id = format!("agent.instructions.{}", instruction.source.kind);
            if required_ids.insert(capability_id.clone()) {
                results.push(negotiate_request(
                    &CapabilityRequest {
                        id: capability_id,
                        version: 1,
                        allow_degraded: false,
                        unknown: BTreeMap::new(),
                    },
                    CapabilityRequirement::Required,
                    descriptor,
                ));
            }
        }
        for request in &self.spec.capabilities.optional {
            results.push(negotiate_request(
                request,
                CapabilityRequirement::Optional,
                descriptor,
            ));
        }
        let compatible = results.iter().all(|result| match result.requirement {
            CapabilityRequirement::Optional => true,
            CapabilityRequirement::Required => match result.status {
                CapabilitySupportStatus::Supported => true,
                CapabilitySupportStatus::Degraded => result.allow_degraded,
                CapabilitySupportStatus::ApprovalRequired
                | CapabilitySupportStatus::Unsupported => false,
            },
        });
        let warnings = results
            .iter()
            .filter(|result| {
                result.requirement == CapabilityRequirement::Optional
                    && result.status != CapabilitySupportStatus::Supported
            })
            .map(|result| {
                format!(
                    "optional capability {} v{} is {}: {}",
                    result.id,
                    result.version,
                    support_status_name(result.status),
                    result.reason
                )
            })
            .collect();
        CapabilityNegotiationReport {
            adapter_id: descriptor.adapter_id.clone(),
            adapter_version: descriptor.adapter_version.clone(),
            provider_id: descriptor.provider_id.clone(),
            provider_version: descriptor.provider_version.clone(),
            runtime_surface: descriptor.runtime_surface.clone(),
            compatible,
            results,
            warnings,
            redactions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentProfileValidationError {
    #[error("agent profile manifest must contain at most {max} bytes")]
    ManifestTooLarge { max: usize },
    #[error("agent profile manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("apiVersion must be {AGENT_PROFILE_API_VERSION}")]
    UnsupportedApiVersion,
    #[error("kind must be {AGENT_PROFILE_KIND}")]
    InvalidKind,
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{field} must be at most {max} bytes")]
    TooLong { field: &'static str, max: usize },
    #[error("{field} must contain at most {max} values")]
    TooManyValues { field: &'static str, max: usize },
    #[error("{field} contains duplicate ID {id}")]
    DuplicateId { field: &'static str, id: String },
    #[error("capability {0} must request a version greater than zero")]
    InvalidCapabilityVersion(String),
    #[error("extension key {0} must use reverse-DNS ownership")]
    InvalidExtensionKey(String),
    #[error("extension {0} must be an object with an apiVersion")]
    InvalidExtension(String),
    #[error("path {0} must be relative and contained")]
    UnsafePath(String),
    #[error("artifact sha256 must contain 64 hexadecimal characters")]
    InvalidArtifactDigest,
    #[error("resolved secret value is forbidden at {0}")]
    SecretValueForbidden(String),
    #[error("agent profile cannot be represented by the WorkerProfile compatibility API: {0}")]
    InvalidWorkerProjection(String),
    #[error("required capabilities are unsupported: {0:?}")]
    UnsupportedRequiredCapabilities(Vec<String>),
    #[error("expected_version must be greater than zero")]
    InvalidVersion,
}

fn prepare_manifest(manifest: Value) -> Result<PreparedAgentProfile, AgentProfileValidationError> {
    let serialized = serde_json::to_vec(&manifest)
        .map_err(|error| AgentProfileValidationError::InvalidManifest(error.to_string()))?;
    if serialized.len() > MAX_MANIFEST_BYTES {
        return Err(AgentProfileValidationError::ManifestTooLarge {
            max: MAX_MANIFEST_BYTES,
        });
    }
    reject_secret_values(&manifest, "$")?;
    let canonical_manifest: AgentProfileManifest = serde_json::from_value(manifest.clone())
        .map_err(|error| AgentProfileValidationError::InvalidManifest(error.to_string()))?;
    validate_manifest(&canonical_manifest)?;
    let worker_profile = canonical_manifest.worker_profile_projection()?;
    let descriptor = compatibility_adapter_descriptor(&worker_profile);
    let validation = canonical_manifest.negotiate(&descriptor);
    ensure_required_capabilities_supported(&validation)?;
    Ok(PreparedAgentProfile {
        original_manifest: manifest,
        canonical_manifest,
        worker_profile,
        validation,
    })
}

fn ensure_required_capabilities_supported(
    validation: &CapabilityNegotiationReport,
) -> Result<(), AgentProfileValidationError> {
    if validation.compatible {
        return Ok(());
    }
    let unsupported = validation
        .results
        .iter()
        .filter(|result| {
            result.requirement == CapabilityRequirement::Required
                && !matches!(result.status, CapabilitySupportStatus::Supported)
                && !(result.status == CapabilitySupportStatus::Degraded && result.allow_degraded)
        })
        .map(|result| format!("{} v{} ({})", result.id, result.version, result.reason))
        .collect();
    Err(AgentProfileValidationError::UnsupportedRequiredCapabilities(unsupported))
}

fn validate_manifest(manifest: &AgentProfileManifest) -> Result<(), AgentProfileValidationError> {
    if manifest.api_version != AGENT_PROFILE_API_VERSION {
        return Err(AgentProfileValidationError::UnsupportedApiVersion);
    }
    if manifest.kind != AGENT_PROFILE_KIND {
        return Err(AgentProfileValidationError::InvalidKind);
    }
    required("metadata.name", &manifest.metadata.name, MAX_NAME_BYTES)?;
    if let Some(version) = manifest.metadata.package_version.as_deref() {
        required("metadata.packageVersion", version, MAX_VALUE_BYTES)?;
    }
    required(
        "spec.role.default",
        &manifest.spec.role.default,
        MAX_VALUE_BYTES,
    )?;
    validate_instructions(&manifest.spec.instructions)?;
    validate_capabilities(&manifest.spec.capabilities)?;
    validate_components(&manifest.spec.components)?;
    validate_policies(&manifest.spec.policies)?;
    validate_extensions(&manifest.spec.extensions)?;
    validate_artifacts(&manifest.artifacts)
}

fn validate_instructions(
    instructions: &[AgentProfileInstruction],
) -> Result<(), AgentProfileValidationError> {
    check_count("spec.instructions", instructions.len(), MAX_LIST_ITEMS)?;
    check_unique_ids(
        "spec.instructions",
        instructions.iter().map(|instruction| &instruction.id),
    )?;
    for instruction in instructions {
        required("instruction.id", &instruction.id, MAX_VALUE_BYTES)?;
        validate_source(&instruction.source)?;
    }
    Ok(())
}

fn validate_capabilities(
    capabilities: &AgentProfileCapabilities,
) -> Result<(), AgentProfileValidationError> {
    check_count(
        "spec.capabilities.required",
        capabilities.required.len(),
        MAX_LIST_ITEMS,
    )?;
    check_count(
        "spec.capabilities.optional",
        capabilities.optional.len(),
        MAX_LIST_ITEMS,
    )?;
    check_unique_ids(
        "spec.capabilities",
        capabilities
            .required
            .iter()
            .chain(&capabilities.optional)
            .map(|capability| &capability.id),
    )?;
    for capability in capabilities.required.iter().chain(&capabilities.optional) {
        required("capability.id", &capability.id, MAX_VALUE_BYTES)?;
        if capability.version == 0 {
            return Err(AgentProfileValidationError::InvalidCapabilityVersion(
                capability.id.clone(),
            ));
        }
    }
    Ok(())
}

fn validate_components(
    components: &AgentProfileComponents,
) -> Result<(), AgentProfileValidationError> {
    validate_component_list("spec.components.skills", &components.skills)?;
    validate_component_list("spec.components.tools", &components.tools)?;
    validate_component_list("spec.components.mcpServers", &components.mcp_servers)
}

fn validate_component_list(
    field: &'static str,
    components: &[AgentProfileComponent],
) -> Result<(), AgentProfileValidationError> {
    check_count(field, components.len(), MAX_LIST_ITEMS)?;
    check_unique_ids(field, components.iter().map(|component| &component.id))?;
    for component in components {
        required("component.id", &component.id, MAX_VALUE_BYTES)?;
        if let Some(source) = component.source.as_ref() {
            validate_source(source)?;
        }
        check_count(
            "component.credentialSlots",
            component.credential_slots.len(),
            MAX_LIST_ITEMS,
        )?;
        check_unique_ids(
            "component.credentialSlots",
            component.credential_slots.iter().map(|slot| &slot.id),
        )?;
        for slot in &component.credential_slots {
            required("credentialSlot.id", &slot.id, MAX_VALUE_BYTES)?;
            required("credentialSlot.name", &slot.name, MAX_VALUE_BYTES)?;
        }
    }
    Ok(())
}

fn validate_source(source: &AgentProfileSource) -> Result<(), AgentProfileValidationError> {
    required("source.kind", &source.kind, MAX_VALUE_BYTES)?;
    match source.kind.as_str() {
        "workspace" | "bundle" => {
            let path = source
                .path
                .as_deref()
                .ok_or(AgentProfileValidationError::Required("source.path"))?;
            validate_relative_path(path)?;
        }
        "registry" | "legacy" => {
            let reference = source
                .reference
                .as_deref()
                .ok_or(AgentProfileValidationError::Required("source.reference"))?;
            required("source.reference", reference, MAX_VALUE_BYTES)?;
        }
        _ => {
            return Err(AgentProfileValidationError::InvalidManifest(format!(
                "unsupported source kind {}",
                source.kind
            )));
        }
    }
    Ok(())
}

fn validate_policies(policies: &AgentProfilePolicies) -> Result<(), AgentProfileValidationError> {
    required(
        "spec.policies.isolation",
        &policies.isolation,
        MAX_VALUE_BYTES,
    )?;
    required("spec.policies.sandbox", &policies.sandbox, MAX_VALUE_BYTES)?;
    required(
        "spec.policies.permissions",
        &policies.permissions,
        MAX_VALUE_BYTES,
    )?;
    required(
        "spec.policies.completion",
        &policies.completion,
        MAX_VALUE_BYTES,
    )?;
    Ok(())
}

fn validate_extensions(
    extensions: &BTreeMap<String, Value>,
) -> Result<(), AgentProfileValidationError> {
    check_count("spec.extensions", extensions.len(), MAX_EXTENSIONS)?;
    for (key, value) in extensions {
        if !is_reverse_dns(key) {
            return Err(AgentProfileValidationError::InvalidExtensionKey(
                key.clone(),
            ));
        }
        let Some(object) = value.as_object() else {
            return Err(AgentProfileValidationError::InvalidExtension(key.clone()));
        };
        if object
            .get("apiVersion")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(AgentProfileValidationError::InvalidExtension(key.clone()));
        }
    }
    Ok(())
}

fn validate_artifacts(
    artifacts: &[AgentProfileArtifact],
) -> Result<(), AgentProfileValidationError> {
    check_count("artifacts", artifacts.len(), MAX_ARTIFACTS)?;
    let mut paths = BTreeSet::new();
    for artifact in artifacts {
        validate_relative_path(&artifact.path)?;
        if !paths.insert(&artifact.path) {
            return Err(AgentProfileValidationError::DuplicateId {
                field: "artifacts",
                id: artifact.path.clone(),
            });
        }
        required("artifact.mediaType", &artifact.media_type, MAX_VALUE_BYTES)?;
        if artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AgentProfileValidationError::InvalidArtifactDigest);
        }
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), AgentProfileValidationError> {
    required("path", value, MAX_VALUE_BYTES)?;
    let path = Path::new(value);
    if path.is_absolute()
        || value.contains('\\')
        || value.as_bytes().get(1) == Some(&b':')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AgentProfileValidationError::UnsafePath(value.to_owned()));
    }
    Ok(())
}

fn reject_secret_values(value: &Value, path: &str) -> Result<(), AgentProfileValidationError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}.{key}");
                if secret_key(key) && !child.is_null() {
                    return Err(AgentProfileValidationError::SecretValueForbidden(
                        child_path,
                    ));
                }
                reject_secret_values(child, &child_path)?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                reject_secret_values(child, &format!("{path}[{index}]"))?;
            }
        }
        Value::String(value) if secret_value(value) => {
            return Err(AgentProfileValidationError::SecretValueForbidden(
                path.to_owned(),
            ));
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn secret_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "authorization"
            | "proxyauthorization"
            | "authheader"
            | "credential"
            | "credentials"
            | "keychaincontents"
            | "privatekey"
            | "privatekeydata"
            | "privatekeyvalue"
            | "privateenvironment"
            | "privateenvironmentvalues"
            | "secretaccesskey"
            | "secretkey"
            | "environmentvalues"
            | "credentialvalue"
    ) || normalized.ends_with("apikey")
        || normalized.ends_with("token")
        || normalized.ends_with("password")
        || normalized.ends_with("secret")
        || normalized.ends_with("cookie")
        || normalized.ends_with("cookies")
}

fn secret_value(value: &str) -> bool {
    let value = value.trim();
    value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        || value
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("basic "))
        || (value.contains("-----BEGIN ") && value.contains(" PRIVATE KEY-----"))
        || value.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
}

fn compatibility_adapter_descriptor(spec: &WorkerProfileSpec) -> AdapterDescriptor {
    let (adapter_version, runtime_surface, capabilities) = if spec.runtime_adapter == "herdr" {
        (
            "v0.8-compatible".to_owned(),
            "cli".to_owned(),
            vec![
                supported_capability("filesystem.workspace.read"),
                supported_capability("filesystem.workspace.write"),
                supported_capability("process.shell"),
            ],
        )
    } else {
        (
            "unavailable".to_owned(),
            "unavailable".to_owned(),
            Vec::new(),
        )
    };
    AdapterDescriptor {
        adapter_id: spec.runtime_adapter.clone(),
        adapter_version,
        provider_id: spec.provider.clone(),
        provider_version: None,
        runtime_surface,
        capabilities,
    }
}

fn supported_capability(id: &str) -> AdapterCapability {
    AdapterCapability {
        id: id.to_owned(),
        min_version: 1,
        max_version: 1,
        status: CapabilitySupportStatus::Supported,
        reason: "supported by the current workspace-bound Herdr runtime".to_owned(),
        surface: "herdr-cli".to_owned(),
    }
}

fn negotiate_request(
    request: &CapabilityRequest,
    requirement: CapabilityRequirement,
    descriptor: &AdapterDescriptor,
) -> CapabilityNegotiation {
    let support = descriptor.capabilities.iter().find(|capability| {
        capability.id == request.id
            && request.version >= capability.min_version
            && request.version <= capability.max_version
    });
    support.map_or_else(
        || CapabilityNegotiation {
            id: request.id.clone(),
            version: request.version,
            requirement,
            allow_degraded: request.allow_degraded,
            status: CapabilitySupportStatus::Unsupported,
            reason: format!(
                "{} does not advertise capability {} v{}",
                descriptor.adapter_id, request.id, request.version
            ),
            surface: descriptor.runtime_surface.clone(),
        },
        |support| CapabilityNegotiation {
            id: request.id.clone(),
            version: request.version,
            requirement,
            allow_degraded: request.allow_degraded,
            status: support.status,
            reason: support.reason.clone(),
            surface: support.surface.clone(),
        },
    )
}

fn add_required_component_capability(
    results: &mut Vec<CapabilityNegotiation>,
    required_ids: &mut BTreeSet<String>,
    components: &[AgentProfileComponent],
    capability_id: &str,
    descriptor: &AdapterDescriptor,
) {
    if components.iter().any(|component| component.required)
        && required_ids.insert(capability_id.to_owned())
    {
        results.push(negotiate_request(
            &CapabilityRequest {
                id: capability_id.to_owned(),
                version: 1,
                allow_degraded: false,
                unknown: BTreeMap::new(),
            },
            CapabilityRequirement::Required,
            descriptor,
        ));
    }
}

fn parse_worker_profile_extension(
    value: &Value,
) -> Result<(String, String, Option<String>), AgentProfileValidationError> {
    let object = value.as_object().ok_or_else(|| {
        AgentProfileValidationError::InvalidExtension(WORKER_PROFILE_EXTENSION_KEY.to_owned())
    })?;
    if object.get("apiVersion").and_then(Value::as_str)
        != Some(WORKER_PROFILE_EXTENSION_API_VERSION)
    {
        return Err(AgentProfileValidationError::InvalidWorkerProjection(
            "unsupported dev.yard.worker-profile apiVersion".to_owned(),
        ));
    }
    let runtime_adapter = extension_string(object, "runtimeAdapter")?;
    let provider = extension_string(object, "provider")?;
    let model = match object.get("model") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str().map(str::to_owned).ok_or_else(|| {
            AgentProfileValidationError::InvalidWorkerProjection(
                "dev.yard.worker-profile model must be a string or null".to_owned(),
            )
        })?),
    };
    Ok((runtime_adapter, provider, model))
}

fn validate_compatibility_extension_consistency(
    extensions: &BTreeMap<String, Value>,
    compatibility: &(String, String, Option<String>),
) -> Result<(), AgentProfileValidationError> {
    let (runtime_adapter, provider, model) = compatibility;
    if extensions.contains_key(HERDR_EXTENSION_KEY) {
        if runtime_adapter != "herdr" {
            return Ok(());
        }
        let (_, herdr_provider, herdr_model) = parse_herdr_projection(extensions)?;
        if &herdr_provider != provider
            || (provider_extension_key(provider).is_some() && &herdr_model != model)
        {
            return Err(AgentProfileValidationError::InvalidWorkerProjection(
                "runtime/provider extensions conflict with dev.yard.worker-profile".to_owned(),
            ));
        }
    }
    Ok(())
}

fn parse_herdr_projection(
    extensions: &BTreeMap<String, Value>,
) -> Result<(String, String, Option<String>), AgentProfileValidationError> {
    let extension = extensions.get(HERDR_EXTENSION_KEY).ok_or_else(|| {
        AgentProfileValidationError::InvalidWorkerProjection(
            "a dev.yard.herdr or dev.yard.worker-profile extension is required".to_owned(),
        )
    })?;
    let object = extension.as_object().ok_or_else(|| {
        AgentProfileValidationError::InvalidExtension(HERDR_EXTENSION_KEY.to_owned())
    })?;
    if object.get("apiVersion").and_then(Value::as_str) != Some(HERDR_EXTENSION_API_VERSION) {
        return Err(AgentProfileValidationError::InvalidWorkerProjection(
            "unsupported dev.yard.herdr apiVersion".to_owned(),
        ));
    }
    let provider = extension_string(object, "agentKind")?;
    let model = if let Some(extension) =
        provider_extension_key(&provider).and_then(|key| extensions.get(key))
    {
        let object = extension.as_object().ok_or_else(|| {
            AgentProfileValidationError::InvalidExtension(
                provider_extension_key(&provider)
                    .expect("selected provider key exists")
                    .to_owned(),
            )
        })?;
        if object.get("apiVersion").and_then(Value::as_str) != Some(provider_api_version(&provider))
        {
            return Err(AgentProfileValidationError::InvalidWorkerProjection(
                "unsupported selected provider extension apiVersion".to_owned(),
            ));
        }
        match object.get("model") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str().map(str::to_owned).ok_or_else(|| {
                AgentProfileValidationError::InvalidWorkerProjection(
                    "provider extension model must be a string or null".to_owned(),
                )
            })?),
        }
    } else {
        None
    };
    Ok(("herdr".to_owned(), provider, model))
}

fn extension_string(
    object: &Map<String, Value>,
    key: &'static str,
) -> Result<String, AgentProfileValidationError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            AgentProfileValidationError::InvalidWorkerProjection(format!(
                "extension field {key} is required"
            ))
        })
}

fn provider_extension_key(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("com.openai.codex"),
        "claude" => Some("com.anthropic.claude-code"),
        "kiro" => Some("dev.kiro"),
        "hermes" => Some("com.nousresearch.hermes"),
        _ => None,
    }
}

fn provider_api_version(provider: &str) -> &'static str {
    match provider {
        "codex" => "yard.dev/providers/codex/v1alpha1",
        "claude" => "yard.dev/providers/claude-code/v1alpha1",
        "kiro" => "yard.dev/providers/kiro/v1alpha1",
        "hermes" => "yard.dev/providers/hermes/v1alpha1",
        _ => "yard.dev/providers/unknown/v1alpha1",
    }
}

fn worker_profile_extensions(spec: &WorkerProfileSpec) -> BTreeMap<String, Value> {
    let mut extensions = BTreeMap::from([(
        WORKER_PROFILE_EXTENSION_KEY.to_owned(),
        json!({
            "apiVersion": WORKER_PROFILE_EXTENSION_API_VERSION,
            "runtimeAdapter": spec.runtime_adapter,
            "provider": spec.provider,
            "model": spec.model,
        }),
    )]);
    if spec.runtime_adapter != "herdr" {
        return extensions;
    }
    extensions.insert(
        HERDR_EXTENSION_KEY.to_owned(),
        json!({
            "apiVersion": HERDR_EXTENSION_API_VERSION,
            "agentKind": spec.provider,
        }),
    );
    if let Some(provider_key) = provider_extension_key(&spec.provider) {
        let mut extension = Map::new();
        extension.insert(
            "apiVersion".to_owned(),
            Value::String(provider_api_version(&spec.provider).to_owned()),
        );
        extension.insert(
            "model".to_owned(),
            spec.model
                .as_ref()
                .map_or(Value::Null, |model| Value::String(model.clone())),
        );
        extensions.insert(provider_key.to_owned(), Value::Object(extension));
    }
    extensions
}

fn merge_extension(extensions: &mut BTreeMap<String, Value>, key: String, value: Value) {
    let Some(new_object) = value.as_object() else {
        extensions.insert(key, value);
        return;
    };
    if let Some(existing) = extensions.get_mut(&key).and_then(Value::as_object_mut) {
        for (field, value) in new_object {
            existing.insert(field.clone(), value.clone());
        }
    } else {
        extensions.insert(key, value);
    }
}

fn merge_instructions(
    current: &[AgentProfileInstruction],
    replacement: Vec<AgentProfileInstruction>,
) -> Vec<AgentProfileInstruction> {
    let Some(mut replacement) = replacement.into_iter().next() else {
        return Vec::new();
    };
    let Some(existing) = current.first() else {
        return vec![replacement];
    };
    replacement.id.clone_from(&existing.id);
    replacement.required = existing.required;
    replacement.unknown.clone_from(&existing.unknown);
    replacement
        .source
        .unknown
        .clone_from(&existing.source.unknown);
    let mut merged = vec![replacement];
    merged.extend(current.iter().skip(1).cloned());
    merged
}

fn merge_components(
    current: &[AgentProfileComponent],
    replacement: Vec<AgentProfileComponent>,
) -> Vec<AgentProfileComponent> {
    replacement
        .into_iter()
        .map(|component| {
            current
                .iter()
                .find(|existing| existing.id == component.id)
                .cloned()
                .unwrap_or(component)
        })
        .collect()
}

fn legacy_components(values: &[String]) -> Vec<AgentProfileComponent> {
    values
        .iter()
        .map(|value| AgentProfileComponent {
            id: value.clone(),
            source: Some(AgentProfileSource {
                kind: "legacy".to_owned(),
                path: None,
                reference: Some(value.clone()),
                unknown: BTreeMap::new(),
            }),
            required: false,
            credential_slots: Vec::new(),
            unknown: BTreeMap::new(),
        })
        .collect()
}

fn component_ids(components: &[AgentProfileComponent]) -> Vec<String> {
    components
        .iter()
        .map(|component| component.id.clone())
        .collect()
}

fn capability_request(id: &str) -> CapabilityRequest {
    CapabilityRequest {
        id: id.to_owned(),
        version: 1,
        allow_degraded: false,
        unknown: BTreeMap::new(),
    }
}

fn legacy_policy(value: &str) -> String {
    match value {
        "project-workspace" => "project_workspace".to_owned(),
        "runtime-default" => "runtime_default".to_owned(),
        value => value.to_owned(),
    }
}

fn legacy_permission_policy(value: &str) -> String {
    match value {
        "runtime-default" => "runtime_default".to_owned(),
        "full-access" => "yolo".to_owned(),
        value => value.to_owned(),
    }
}

fn legacy_completion_policy(value: &str) -> String {
    match value {
        "explicit-receipt" => "manual_receipt".to_owned(),
        value => value.to_owned(),
    }
}

fn required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), AgentProfileValidationError> {
    if value.trim().is_empty() {
        return Err(AgentProfileValidationError::Required(field));
    }
    if value.len() > max {
        return Err(AgentProfileValidationError::TooLong { field, max });
    }
    Ok(())
}

fn check_count(
    field: &'static str,
    count: usize,
    max: usize,
) -> Result<(), AgentProfileValidationError> {
    if count > max {
        return Err(AgentProfileValidationError::TooManyValues { field, max });
    }
    Ok(())
}

fn check_unique_ids<'a>(
    field: &'static str,
    ids: impl Iterator<Item = &'a String>,
) -> Result<(), AgentProfileValidationError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(AgentProfileValidationError::DuplicateId {
                field,
                id: id.clone(),
            });
        }
    }
    Ok(())
}

fn is_reverse_dns(value: &str) -> bool {
    let labels = value.split('.').collect::<Vec<_>>();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphabetic)
        })
}

fn support_status_name(status: CapabilitySupportStatus) -> &'static str {
    match status {
        CapabilitySupportStatus::Supported => "supported",
        CapabilitySupportStatus::Degraded => "degraded",
        CapabilitySupportStatus::ApprovalRequired => "approval_required",
        CapabilitySupportStatus::Unsupported => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        AgentProfileManifest, AgentProfileValidationError, CapabilitySupportStatus,
        CreateAgentProfile, PreparedAgentProfile,
    };

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/agent-profile-v1alpha1.json"
        ))
        .unwrap()
    }

    #[test]
    fn fixture_preserves_unknown_content_and_projects_to_worker_profile() {
        let original = fixture();
        let prepared = CreateAgentProfile {
            manifest: original.clone(),
        }
        .prepare()
        .unwrap();

        assert_eq!(prepared.original_manifest, original);
        assert_eq!(prepared.worker_profile.runtime_adapter, "herdr");
        assert_eq!(prepared.worker_profile.provider, "codex");
        assert_eq!(prepared.worker_profile.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(
            prepared.canonical_manifest.unknown["x-roundTrip"]["owner"],
            "fixture"
        );
        assert_eq!(
            prepared.canonical_manifest.spec.extensions["io.example.provider"]["futureSetting"],
            17
        );
        assert!(prepared.validation.compatible);
        assert!(prepared.validation.results.iter().any(|result| {
            result.id == "tool.mcp.client" && result.status == CapabilitySupportStatus::Unsupported
        }));
    }

    #[test]
    fn rejects_unsupported_required_capability() {
        let mut manifest = fixture();
        manifest["spec"]["capabilities"]["required"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "agent.skills", "version": 1}));

        let error = CreateAgentProfile { manifest }.prepare().unwrap_err();

        assert!(matches!(
            error,
            AgentProfileValidationError::UnsupportedRequiredCapabilities(capabilities)
                if capabilities[0].contains("agent.skills")
        ));
    }

    #[test]
    fn rejects_required_component_without_adapter_support() {
        let mut manifest = fixture();
        manifest["spec"]["components"]["mcpServers"][0]["required"] = Value::Bool(true);

        let error = CreateAgentProfile { manifest }.prepare().unwrap_err();

        assert!(matches!(
            error,
            AgentProfileValidationError::UnsupportedRequiredCapabilities(capabilities)
                if capabilities[0].contains("tool.mcp.client")
        ));
    }

    #[test]
    fn rejects_secret_values_in_unknown_extensions() {
        for (field, value) in [
            ("apiKey", json!("do-not-store")),
            ("Authorization", json!("Bearer do-not-store")),
            ("credentials", json!({"value": "do-not-store"})),
            ("privateKey", json!({"d": "do-not-store"})),
            ("secretAccessKey", json!("do-not-store")),
            ("opaqueHeader", json!("Basic do-not-store")),
            (
                "opaqueKey",
                json!("-----BEGIN EC PRIVATE KEY-----\ndo-not-store"),
            ),
        ] {
            let mut manifest = fixture();
            manifest["spec"]["extensions"]["io.example.provider"][field] = value;

            assert!(matches!(
                CreateAgentProfile { manifest }.prepare(),
                Err(AgentProfileValidationError::SecretValueForbidden(_))
            ));
        }
    }

    #[test]
    fn credential_slots_reject_resolved_values() {
        let mut manifest = fixture();
        manifest["spec"]["components"]["mcpServers"][0]["credentialSlots"][0]["value"] =
            json!("do-not-store");

        assert!(matches!(
            CreateAgentProfile { manifest }.prepare(),
            Err(AgentProfileValidationError::InvalidManifest(_))
        ));
    }

    #[test]
    fn worker_profile_overlay_retains_unknown_extension_content() {
        let prepared = CreateAgentProfile {
            manifest: fixture(),
        }
        .prepare()
        .unwrap();
        let mut worker = prepared.worker_profile;
        worker.name = "Updated".to_owned();
        worker.model = Some("gpt-6".to_owned());

        let updated = prepared.canonical_manifest.with_worker_profile(&worker);

        assert_eq!(updated.metadata.name, "Updated");
        assert_eq!(
            updated.spec.extensions["io.example.provider"]["futureSetting"],
            17
        );
        assert_eq!(
            updated.spec.extensions["com.openai.codex"]["model"],
            "gpt-6"
        );

        worker.model = None;
        let cleared = updated.with_worker_profile(&worker);
        assert!(cleared.spec.extensions["com.openai.codex"]["model"].is_null());
        assert_eq!(cleared.worker_profile_projection().unwrap().model, None);

        worker.runtime_adapter = "other-runtime".to_owned();
        let changed_runtime = cleared.with_worker_profile(&worker);
        assert_eq!(
            changed_runtime
                .worker_profile_projection()
                .unwrap()
                .runtime_adapter,
            "other-runtime"
        );
    }

    #[test]
    fn worker_profile_overlay_fails_closed_after_incompatible_runtime_change() {
        let prepared = CreateAgentProfile {
            manifest: fixture(),
        }
        .prepare()
        .unwrap();
        let mut worker = prepared.worker_profile;
        worker.runtime_adapter = "other-runtime".to_owned();

        let error = PreparedAgentProfile::preserving_unknown(&prepared.canonical_manifest, &worker)
            .unwrap_err();

        assert!(matches!(
            error,
            AgentProfileValidationError::UnsupportedRequiredCapabilities(capabilities)
                if capabilities.iter().any(|capability| {
                    capability.contains("filesystem.workspace.read")
                })
        ));
    }

    #[test]
    fn worker_profile_mapping_round_trips_legacy_fields() {
        let worker = crate::WorkerProfileSpec {
            name: "Implementer".to_owned(),
            runtime_adapter: "herdr".to_owned(),
            provider: "claude".to_owned(),
            model: Some("sonnet".to_owned()),
            default_role: "implementation".to_owned(),
            instructions_ref: Some("AGENTS.md".to_owned()),
            tools: vec!["shell".to_owned()],
            skills: vec!["review".to_owned()],
            mcp_servers: vec!["docs".to_owned()],
            sandbox_policy: "runtime_default".to_owned(),
            worktree_policy: "project_workspace".to_owned(),
            permission_policy: "runtime_default".to_owned(),
            completion_contract: "manual_receipt".to_owned(),
        };

        let projected = AgentProfileManifest::from_worker_profile(&worker)
            .worker_profile_projection()
            .unwrap();

        assert_eq!(projected, worker);
    }

    #[test]
    fn worker_profile_mapping_round_trips_null_model() {
        let mut worker = crate::WorkerProfileSpec {
            name: "Implementer".to_owned(),
            runtime_adapter: "herdr".to_owned(),
            provider: "codex".to_owned(),
            model: None,
            default_role: "implementation".to_owned(),
            instructions_ref: None,
            tools: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            sandbox_policy: "runtime_default".to_owned(),
            worktree_policy: "project_workspace".to_owned(),
            permission_policy: "runtime_default".to_owned(),
            completion_contract: "manual_receipt".to_owned(),
        };
        let manifest = AgentProfileManifest::from_worker_profile(&worker);

        assert_eq!(manifest.worker_profile_projection().unwrap(), worker);

        worker.provider = "claude".to_owned();
        assert_eq!(
            AgentProfileManifest::from_worker_profile(&worker)
                .worker_profile_projection()
                .unwrap(),
            worker
        );
    }

    #[test]
    fn worker_profile_mapping_round_trips_custom_provider_model() {
        let worker = crate::WorkerProfileSpec {
            name: "Implementer".to_owned(),
            runtime_adapter: "herdr".to_owned(),
            provider: "custom-provider".to_owned(),
            model: Some("custom-model".to_owned()),
            default_role: "implementation".to_owned(),
            instructions_ref: None,
            tools: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            sandbox_policy: "runtime_default".to_owned(),
            worktree_policy: "project_workspace".to_owned(),
            permission_policy: "runtime_default".to_owned(),
            completion_contract: "manual_receipt".to_owned(),
        };

        assert_eq!(
            AgentProfileManifest::from_worker_profile(&worker)
                .worker_profile_projection()
                .unwrap(),
            worker
        );
    }
}

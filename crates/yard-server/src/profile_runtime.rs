use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use yard_domain::{
    AdapterCapability, AdapterDescriptor, AgentProfile, AgentProfileComponent,
    AgentProfileManifest, CapabilityNegotiationReport, CapabilitySupportStatus, WorkerProfile,
};

const HERDR_ORCHESTRATION: &str = include_str!("../../../skills/herdr-orchestration/SKILL.md");
const HERDR_CLI: &str = include_str!("../../../skills/herdr-cli/SKILL.md");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLaunchPlan {
    pub command_id: String,
    pub profile_id: String,
    #[serde(with = "yard_domain::serde_u64")]
    pub profile_version: u64,
    pub bundle_api_version: String,
    pub adapter_id: String,
    pub provider_id: String,
    pub runtime_surface: String,
    pub args: Vec<String>,
    pub negotiation: CapabilityNegotiationReport,
    pub components: Vec<ProfileComponentPlan>,
    pub generated_files: Vec<GeneratedProfileFile>,
    pub permissions: Vec<String>,
    pub missing_capabilities: Vec<String>,
    pub approvals: Vec<String>,
    pub warnings: Vec<String>,
    pub compatible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileComponentPlan {
    pub kind: String,
    pub id: String,
    pub required: bool,
    pub status: CapabilitySupportStatus,
    pub reason: String,
    pub surface: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedProfileFile {
    pub path: String,
    pub media_type: String,
    pub sha256: String,
    pub provenance: String,
}

#[derive(Debug, Clone)]
pub struct CompiledProfileLaunch {
    pub plan: ProfileLaunchPlan,
    files: Vec<MaterializedProfileFile>,
}

#[derive(Debug, Clone)]
struct MaterializedProfileFile {
    path: String,
    media_type: String,
    content: String,
}

#[derive(Debug, Error)]
pub enum ProfileRuntimeError {
    #[error("portable profile revision does not match the worker profile pin")]
    RevisionMismatch,
    #[error("portable profile manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("unsupported worker profile: {0}")]
    Unsupported(String),
    #[error("profile materialization failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("managed profile path {0} escapes through a symlink")]
    SymlinkEscape(String),
    #[error("managed profile path {0} is not a regular file or directory")]
    UnsupportedFileType(String),
    #[error("managed profile file {0} already exists with different content")]
    FileConflict(String),
}

/// Compiles one immutable profile revision for the selected runtime.
///
/// # Errors
///
/// Returns an error when the revision does not match its worker-profile pin or
/// when the selected runtime constraints cannot be represented.
#[allow(clippy::too_many_lines)]
pub fn compile_profile_launch(
    worker: &WorkerProfile,
    portable: &AgentProfile,
    command_id: &str,
) -> Result<CompiledProfileLaunch, ProfileRuntimeError> {
    if worker.id != portable.id || worker.version != portable.version {
        return Err(ProfileRuntimeError::RevisionMismatch);
    }
    let manifest: AgentProfileManifest = serde_json::from_value(portable.manifest.clone())
        .map_err(|error| ProfileRuntimeError::InvalidManifest(error.to_string()))?;
    let descriptor = adapter_descriptor(worker);
    let negotiation = manifest.negotiate(&descriptor);
    let mut args = provider_args(worker)?;
    let mut components = Vec::new();
    let mut files = Vec::new();
    let mut missing_capabilities = Vec::new();
    let mut approvals = Vec::new();
    let mut warnings = negotiation.warnings.clone();

    compile_instructions(&manifest, worker, &mut components);
    compile_components(
        "skill",
        &manifest.spec.components.skills,
        worker,
        portable,
        &mut components,
        &mut files,
    );
    unsupported_components(
        "tool",
        &manifest.spec.components.tools,
        "tool injection is not implemented by the Herdr adapter",
        &mut components,
    );
    unsupported_components(
        "mcp_server",
        &manifest.spec.components.mcp_servers,
        "MCP lowering and credential resolution are not implemented by the Herdr adapter",
        &mut components,
    );

    if worker.spec.permission_policy == "yolo" {
        approvals.push("permission bypass requires a separate launch approval".to_owned());
        components.push(ProfileComponentPlan {
            kind: "policy".to_owned(),
            id: "permissions.full_access".to_owned(),
            required: true,
            status: CapabilitySupportStatus::ApprovalRequired,
            reason: "permission bypass is never inferred from profile import or allocation"
                .to_owned(),
            surface: "launch approval".to_owned(),
        });
        args.retain(|arg| {
            arg != "--yolo"
                && arg != "--dangerously-skip-permissions"
                && arg != "--dangerously-bypass-approvals-and-sandbox"
        });
    }
    for result in &negotiation.results {
        if result.status == CapabilitySupportStatus::Unsupported
            && result.requirement == yard_domain::CapabilityRequirement::Required
        {
            missing_capabilities.push(format!("{} v{}", result.id, result.version));
        }
        if result.status == CapabilitySupportStatus::ApprovalRequired {
            approvals.push(format!(
                "{} v{}: {}",
                result.id, result.version, result.reason
            ));
        }
    }
    for component in &components {
        if component.status == CapabilitySupportStatus::Unsupported {
            if component.required || component.reason.contains("legacy component") {
                missing_capabilities.push(format!("{} {}", component.kind, component.id));
            } else {
                warnings.push(format!(
                    "optional {} {} is unsupported: {}",
                    component.kind, component.id, component.reason
                ));
            }
        }
    }

    let generated_files = files
        .iter()
        .map(|file| GeneratedProfileFile {
            path: file.path.clone(),
            media_type: file.media_type.clone(),
            sha256: sha256_hex(file.content.as_bytes()),
            provenance: if is_builtin_skill_path(&file.path) {
                "yard:embedded-orchestrator-kit/v1".to_owned()
            } else {
                format!("agent-profile:{}@{}", worker.id, worker.version)
            },
        })
        .collect::<Vec<_>>();
    let compatible = negotiation.compatible
        && missing_capabilities.is_empty()
        && approvals.is_empty()
        && components
            .iter()
            .filter(|component| component.required)
            .all(|component| component.status == CapabilitySupportStatus::Supported);
    let plan = ProfileLaunchPlan {
        command_id: command_id.to_owned(),
        profile_id: worker.id.clone(),
        profile_version: worker.version,
        bundle_api_version: manifest.api_version,
        adapter_id: descriptor.adapter_id,
        provider_id: descriptor.provider_id,
        runtime_surface: descriptor.runtime_surface,
        args,
        negotiation,
        components,
        generated_files,
        permissions: vec![worker.spec.permission_policy.clone()],
        missing_capabilities,
        approvals,
        warnings,
        compatible,
    };
    Ok(CompiledProfileLaunch { plan, files })
}

fn compile_instructions(
    manifest: &AgentProfileManifest,
    worker: &WorkerProfile,
    plans: &mut Vec<ProfileComponentPlan>,
) {
    plans.extend(manifest.spec.instructions.iter().map(|instruction| {
        let (status, reason, surface) = match (
            worker.spec.provider.as_str(),
            instruction.source.kind.as_str(),
        ) {
            ("codex", "workspace") => (
                CapabilitySupportStatus::Supported,
                "Codex discovers workspace instructions natively".to_owned(),
                "AGENTS.md".to_owned(),
            ),
            ("claude", "workspace") => (
                CapabilitySupportStatus::Degraded,
                "workspace instruction reference is composed into the launch prompt".to_owned(),
                "prompt".to_owned(),
            ),
            (_, kind) => (
                CapabilitySupportStatus::Unsupported,
                format!("instruction source kind {kind} is not lowered for this provider"),
                "unavailable".to_owned(),
            ),
        };
        ProfileComponentPlan {
            kind: "instruction".to_owned(),
            id: instruction.id.clone(),
            required: instruction.required,
            status,
            reason,
            surface,
        }
    }));
}

/// Writes the compiled managed files beneath the selected working directory.
///
/// # Errors
///
/// Returns an error when the root is unavailable, a path escapes through a
/// link, an unsupported file type is encountered, or existing content differs.
pub fn materialize_profile_launch(
    compiled: &CompiledProfileLaunch,
    cwd: &Path,
    command_id: &str,
) -> Result<PathBuf, ProfileRuntimeError> {
    let root = cwd.canonicalize()?;
    if !root.is_dir() {
        return Err(ProfileRuntimeError::UnsupportedFileType(
            root.display().to_string(),
        ));
    }
    for file in &compiled.files {
        write_managed_file(&root, file)?;
    }
    let receipt = PathBuf::from(".yard")
        .join("runs")
        .join(run_key(command_id))
        .join("profile-launch.json");
    let content = serde_json::to_string_pretty(&compiled.plan)
        .map_err(|error| ProfileRuntimeError::InvalidManifest(error.to_string()))?;
    write_managed_file(
        &root,
        &MaterializedProfileFile {
            path: path_string(&receipt),
            media_type: "application/json".to_owned(),
            content,
        },
    )?;
    Ok(root.join(receipt))
}

/// Removes unchanged generated files while preserving user-modified content.
///
/// # Errors
///
/// Returns an error when cleanup encounters a link, unsupported file type, or
/// filesystem failure.
pub fn cleanup_profile_launch(
    plan: &ProfileLaunchPlan,
    cwd: &Path,
) -> Result<(), ProfileRuntimeError> {
    let root = cwd.canonicalize()?;
    for file in &plan.generated_files {
        let path = root.join(&file.path);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(ProfileRuntimeError::SymlinkEscape(file.path.clone()));
        }
        if !metadata.is_file() {
            return Err(ProfileRuntimeError::UnsupportedFileType(file.path.clone()));
        }
        if sha256_hex(&fs::read(&path)?) == file.sha256 {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn compile_components(
    kind: &str,
    components: &[AgentProfileComponent],
    worker: &WorkerProfile,
    portable: &AgentProfile,
    plans: &mut Vec<ProfileComponentPlan>,
    files: &mut Vec<MaterializedProfileFile>,
) {
    for component in components {
        let surface = skill_surface(&worker.spec.provider);
        let mut status = if surface.is_some() {
            CapabilitySupportStatus::Supported
        } else {
            CapabilitySupportStatus::Unsupported
        };
        let mut reason = surface.map_or_else(
            || {
                format!(
                    "{} does not expose a tested project skill directory",
                    worker.spec.provider
                )
            },
            |_| "skill lowers to the provider project discovery directory".to_owned(),
        );
        if status == CapabilitySupportStatus::Unsupported
            && component
                .source
                .as_ref()
                .is_some_and(|source| source.kind == "legacy")
        {
            reason = format!("legacy component cannot be silently dropped; {reason}");
        }
        if status == CapabilitySupportStatus::Supported {
            match component_files(component, portable, surface.expect("checked surface")) {
                Ok(component_files) => files.extend(component_files),
                Err(message) => {
                    status = CapabilitySupportStatus::Unsupported;
                    reason = message;
                }
            }
        }
        plans.push(ProfileComponentPlan {
            kind: kind.to_owned(),
            id: component.id.clone(),
            required: component.required,
            status,
            reason,
            surface: surface.unwrap_or("unavailable").to_owned(),
        });
    }
}

fn component_files(
    component: &AgentProfileComponent,
    portable: &AgentProfile,
    surface: &str,
) -> Result<Vec<MaterializedProfileFile>, String> {
    if let Some(content) = builtin_skill(&component.id) {
        return Ok(vec![MaterializedProfileFile {
            path: format!("{surface}/{}/SKILL.md", component.id),
            media_type: "text/markdown".to_owned(),
            content: content.to_owned(),
        }]);
    }
    let Some(source) = component.source.as_ref() else {
        return Err("skill source is missing".to_owned());
    };
    if source.kind == "legacy" {
        return Err("legacy component is not a packaged Yard skill".to_owned());
    }
    if source.kind != "bundle" {
        return Err(format!(
            "skill source kind {} cannot be materialized",
            source.kind
        ));
    }
    let Some(prefix) = source.path.as_deref() else {
        return Err("bundle skill path is missing".to_owned());
    };
    let prefix = format!("{}/", prefix.trim_end_matches('/'));
    let packaged = portable
        .files
        .iter()
        .filter(|file| file.path.starts_with(&prefix))
        .collect::<Vec<_>>();
    if !packaged
        .iter()
        .any(|file| file.path == format!("{prefix}SKILL.md"))
    {
        return Err("bundle skill is missing SKILL.md".to_owned());
    }
    Ok(packaged
        .into_iter()
        .map(|file| MaterializedProfileFile {
            path: format!(
                "{surface}/{}/{}",
                component.id,
                file.path.trim_start_matches(&prefix)
            ),
            media_type: file.media_type.clone(),
            content: file.content.clone(),
        })
        .collect())
}

fn unsupported_components(
    kind: &str,
    components: &[AgentProfileComponent],
    reason: &str,
    plans: &mut Vec<ProfileComponentPlan>,
) {
    plans.extend(components.iter().map(|component| {
        ProfileComponentPlan {
            kind: kind.to_owned(),
            id: component.id.clone(),
            required: component.required,
            status: CapabilitySupportStatus::Unsupported,
            reason: if component
                .source
                .as_ref()
                .is_some_and(|source| source.kind == "legacy")
            {
                format!("legacy component cannot be silently dropped; {reason}")
            } else {
                reason.to_owned()
            },
            surface: "herdr-cli".to_owned(),
        }
    }));
}

fn adapter_descriptor(worker: &WorkerProfile) -> AdapterDescriptor {
    let mut capabilities = vec![
        supported("filesystem.workspace.read", "project workspace"),
        supported("filesystem.workspace.write", "project workspace"),
        supported("process.shell", "herdr-cli"),
    ];
    match worker.spec.provider.as_str() {
        "codex" => {
            capabilities.push(supported("agent.skills", ".agents/skills"));
            capabilities.push(supported("agent.instructions.workspace", "AGENTS.md"));
        }
        "claude" => {
            capabilities.push(supported("agent.skills", ".claude/skills"));
            capabilities.push(AdapterCapability {
                id: "agent.instructions.workspace".to_owned(),
                min_version: 1,
                max_version: 1,
                status: CapabilitySupportStatus::Degraded,
                reason: "workspace instruction references are composed into the launch prompt"
                    .to_owned(),
                surface: "prompt".to_owned(),
            });
        }
        _ => {}
    }
    AdapterDescriptor {
        adapter_id: worker.spec.runtime_adapter.clone(),
        adapter_version: "yard-herdr/v1".to_owned(),
        provider_id: worker.spec.provider.clone(),
        provider_version: None,
        runtime_surface: "cli".to_owned(),
        capabilities,
    }
}

fn supported(id: &str, surface: &str) -> AdapterCapability {
    AdapterCapability {
        id: id.to_owned(),
        min_version: 1,
        max_version: 1,
        status: CapabilitySupportStatus::Supported,
        reason: "supported by the selected project-scoped runtime surface".to_owned(),
        surface: surface.to_owned(),
    }
}

fn provider_args(profile: &WorkerProfile) -> Result<Vec<String>, ProfileRuntimeError> {
    if profile.spec.runtime_adapter != "herdr" {
        return Err(ProfileRuntimeError::Unsupported(format!(
            "runtime adapter '{}' is not supported",
            profile.spec.runtime_adapter
        )));
    }
    if profile.spec.worktree_policy != "project_workspace"
        || profile.spec.sandbox_policy != "runtime_default"
        || profile.spec.completion_contract != "manual_receipt"
    {
        return Err(ProfileRuntimeError::Unsupported(
            "only project_workspace, runtime_default sandbox, and manual_receipt are supported"
                .to_owned(),
        ));
    }
    let mut args = Vec::new();
    if profile.spec.provider == "codex" {
        args.push("--no-alt-screen".to_owned());
    }
    match profile.spec.permission_policy.as_str() {
        "runtime_default" => {}
        "auto" if profile.spec.provider == "claude" => {
            args.extend(["--permission-mode".to_owned(), "auto".to_owned()]);
        }
        "yolo" => match profile.spec.provider.as_str() {
            "codex" => args.push("--yolo".to_owned()),
            "claude" => args.push("--dangerously-skip-permissions".to_owned()),
            provider => {
                return Err(ProfileRuntimeError::Unsupported(format!(
                    "full access is not mapped for provider {provider}"
                )));
            }
        },
        policy => {
            return Err(ProfileRuntimeError::Unsupported(format!(
                "permission policy {policy} is not supported"
            )));
        }
    }
    if let Some(model) = profile.spec.model.as_ref() {
        match profile.spec.provider.as_str() {
            "codex" => args.extend(["-m".to_owned(), model.clone()]),
            "claude" => args.extend(["--model".to_owned(), model.clone()]),
            provider => {
                return Err(ProfileRuntimeError::Unsupported(format!(
                    "model selection is not mapped for provider {provider}"
                )));
            }
        }
    }
    Ok(args)
}

fn skill_surface(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some(".agents/skills"),
        "claude" => Some(".claude/skills"),
        _ => None,
    }
}

fn builtin_skill(id: &str) -> Option<&'static str> {
    match id {
        "herdr-orchestration" => Some(HERDR_ORCHESTRATION),
        "herdr-cli" => Some(HERDR_CLI),
        _ => None,
    }
}

fn is_builtin_skill_path(path: &str) -> bool {
    path.contains("/herdr-orchestration/") || path.contains("/herdr-cli/")
}

fn write_managed_file(
    root: &Path,
    file: &MaterializedProfileFile,
) -> Result<(), ProfileRuntimeError> {
    let relative = Path::new(&file.path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProfileRuntimeError::UnsupportedFileType(file.path.clone()));
    }
    let parent = relative
        .parent()
        .ok_or_else(|| ProfileRuntimeError::UnsupportedFileType(file.path.clone()))?;
    ensure_directories(root, parent)?;
    let target = root.join(relative);
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            return Err(ProfileRuntimeError::SymlinkEscape(file.path.clone()));
        }
        if !metadata.is_file() {
            return Err(ProfileRuntimeError::UnsupportedFileType(file.path.clone()));
        }
        if fs::read(&target)? == file.content.as_bytes() {
            return Ok(());
        }
        return Err(ProfileRuntimeError::FileConflict(file.path.clone()));
    }
    let temp = target.with_extension(format!("yard-tmp-{}", run_key(&file.path)));
    fs::write(&temp, file.content.as_bytes())?;
    fs::rename(temp, target)?;
    Ok(())
}

fn ensure_directories(root: &Path, relative: &Path) -> Result<(), ProfileRuntimeError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ProfileRuntimeError::UnsupportedFileType(
                relative.display().to_string(),
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ProfileRuntimeError::SymlinkEscape(
                    current.display().to_string(),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(ProfileRuntimeError::UnsupportedFileType(
                    current.display().to_string(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn run_key(value: &str) -> String {
    sha256_hex(value.as_bytes())[..16].to_owned()
}

fn sha256_hex(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn path_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::TempDir;
    use yard_domain::{
        AgentProfile, AgentProfileFile, CapabilitySupportStatus, WorkerProfile, WorkerProfileSpec,
    };

    use super::{cleanup_profile_launch, compile_profile_launch, materialize_profile_launch};

    fn worker(provider: &str, skills: Vec<&str>) -> WorkerProfile {
        WorkerProfile {
            id: "profile-1".to_owned(),
            spec: WorkerProfileSpec {
                name: "Orchestrator".to_owned(),
                runtime_adapter: "herdr".to_owned(),
                provider: provider.to_owned(),
                model: None,
                default_role: "orchestrator".to_owned(),
                instructions_ref: None,
                tools: Vec::new(),
                skills: skills.into_iter().map(str::to_owned).collect(),
                mcp_servers: Vec::new(),
                sandbox_policy: "runtime_default".to_owned(),
                worktree_policy: "project_workspace".to_owned(),
                permission_policy: "runtime_default".to_owned(),
                completion_contract: "manual_receipt".to_owned(),
            },
            version: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn portable(worker: &WorkerProfile) -> AgentProfile {
        AgentProfile {
            id: worker.id.clone(),
            version: worker.version,
            manifest: serde_json::to_value(yard_domain::AgentProfileManifest::from_worker_profile(
                &worker.spec,
            ))
            .unwrap(),
            files: Vec::new(),
            validation: yard_domain::CapabilityNegotiationReport {
                adapter_id: "herdr".to_owned(),
                adapter_version: "test".to_owned(),
                provider_id: worker.spec.provider.clone(),
                provider_version: None,
                runtime_surface: "cli".to_owned(),
                compatible: true,
                results: Vec::new(),
                warnings: Vec::new(),
                redactions: Vec::new(),
            },
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn codex_and_claude_lower_skills_to_distinct_project_paths() {
        let codex = worker("codex", vec!["herdr-orchestration", "herdr-cli"]);
        let claude = worker("claude", vec!["herdr-orchestration"]);

        let codex_plan = compile_profile_launch(&codex, &portable(&codex), "command").unwrap();
        let claude_plan = compile_profile_launch(&claude, &portable(&claude), "command").unwrap();

        assert!(codex_plan.plan.compatible);
        assert!(
            codex_plan
                .plan
                .generated_files
                .iter()
                .all(|file| file.path.starts_with(".agents/skills/"))
        );
        assert!(
            claude_plan
                .plan
                .generated_files
                .iter()
                .all(|file| file.path.starts_with(".claude/skills/"))
        );
    }

    #[test]
    fn unsupported_provider_and_missing_bundle_skill_fail_honestly() {
        let kiro = worker("kiro", vec!["herdr-cli"]);
        let kiro_plan = compile_profile_launch(&kiro, &portable(&kiro), "command").unwrap();
        assert!(!kiro_plan.plan.compatible);
        assert_eq!(
            kiro_plan.plan.components[0].status,
            CapabilitySupportStatus::Unsupported
        );

        let custom = worker("codex", vec!["custom"]);
        let custom_plan = compile_profile_launch(&custom, &portable(&custom), "command").unwrap();
        assert!(!custom_plan.plan.compatible);
        assert!(
            custom_plan.plan.components[0]
                .reason
                .contains("legacy component")
        );
    }

    #[test]
    fn imported_skill_requires_skill_file_and_lowers_support_files() {
        let worker = worker("codex", vec!["custom"]);
        let mut portable = portable(&worker);
        portable.manifest["spec"]["components"]["skills"][0]["source"] =
            serde_json::json!({"kind": "bundle", "path": "skills/custom"});

        let missing = compile_profile_launch(&worker, &portable, "command").unwrap();
        assert!(!missing.plan.compatible);
        assert!(
            missing.plan.components[0]
                .reason
                .contains("missing SKILL.md")
        );

        portable.files = vec![
            AgentProfileFile {
                path: "skills/custom/SKILL.md".to_owned(),
                media_type: "text/markdown".to_owned(),
                content: "# Custom".to_owned(),
            },
            AgentProfileFile {
                path: "skills/custom/references/example.md".to_owned(),
                media_type: "text/markdown".to_owned(),
                content: "# Example".to_owned(),
            },
        ];
        let compiled = compile_profile_launch(&worker, &portable, "command").unwrap();
        assert!(compiled.plan.compatible);
        assert_eq!(compiled.plan.generated_files.len(), 2);
        assert!(compiled.plan.generated_files.iter().all(|file| {
            file.path.starts_with(".agents/skills/custom/")
                && file.provenance == "agent-profile:profile-1@1"
        }));
    }

    #[test]
    fn full_access_requires_approval_and_never_emits_bypass_args() {
        let mut worker = worker("codex", Vec::new());
        worker.spec.permission_policy = "yolo".to_owned();
        let compiled = compile_profile_launch(&worker, &portable(&worker), "command").unwrap();

        assert!(!compiled.plan.compatible);
        assert!(!compiled.plan.approvals.is_empty());
        assert!(!compiled.plan.args.iter().any(|arg| arg == "--yolo"));
        assert!(
            compiled
                .plan
                .components
                .iter()
                .any(|component| { component.status == CapabilitySupportStatus::ApprovalRequired })
        );
    }

    #[test]
    fn materialization_is_contained_and_cleanup_preserves_changed_files() {
        let temp = TempDir::new().unwrap();
        let worker = worker("codex", vec!["herdr-cli"]);
        let compiled = compile_profile_launch(&worker, &portable(&worker), "command").unwrap();

        materialize_profile_launch(&compiled, temp.path(), "command").unwrap();
        let skill = temp.path().join(".agents/skills/herdr-cli/SKILL.md");
        assert!(skill.is_file());
        assert!(!temp.path().join("CLAUDE.md").exists());

        fs::write(&skill, "user replacement").unwrap();
        cleanup_profile_launch(&compiled.plan, temp.path()).unwrap();
        assert_eq!(fs::read_to_string(skill).unwrap(), "user replacement");
    }

    #[test]
    fn materialization_rejects_symlink_escape() {
        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), temp.path().join(".agents")).unwrap();
        let worker = worker("codex", vec!["herdr-cli"]);
        let compiled = compile_profile_launch(&worker, &portable(&worker), "command").unwrap();

        let error = materialize_profile_launch(&compiled, temp.path(), "command").unwrap_err();

        assert!(error.to_string().contains("symlink"));
        assert!(!outside.path().join("skills/herdr-cli/SKILL.md").exists());
    }
}

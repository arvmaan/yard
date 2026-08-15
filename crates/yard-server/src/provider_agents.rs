use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use yard_domain::{ObservedChildAgent, ObservedStatus, ObservedWorker, ProviderSessionRef};

const CACHE_TTL: Duration = Duration::from_secs(2);
const ACTIVE_WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_METADATA_BYTES: u64 = 128 * 1024;
const MAX_TAIL_BYTES: u64 = 128 * 1024;
const MAX_PROVIDER_FILES: usize = 5_000;

#[derive(Debug)]
pub(crate) struct ProviderAgentObserver {
    roots: ProviderRoots,
    cache: Mutex<ObservationCache>,
}

#[derive(Debug, Clone)]
struct ProviderRoots {
    codex_sessions: Option<PathBuf>,
    claude_projects: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct ObservationCache {
    parent_key: Vec<String>,
    observed_at: Option<Instant>,
    children: Vec<ObservedChildAgent>,
    codex_metadata: BTreeMap<PathBuf, CodexAgentMetadata>,
    statuses: BTreeMap<PathBuf, CachedStatus>,
}

#[derive(Debug, Clone)]
struct CodexAgentMetadata {
    id: String,
    direct_parent_id: String,
    nickname: Option<String>,
    role: Option<String>,
    depth: u32,
    path: PathBuf,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSignature {
    length: u64,
    modified_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
struct CachedStatus {
    signature: FileSignature,
    status: ObservedStatus,
}

impl ProviderAgentObserver {
    pub(crate) fn from_env() -> Self {
        let home = env::var_os("HOME").map(PathBuf::from);
        let codex_home = env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|path| path.join(".codex")));
        let claude_config = env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home.map(|path| path.join(".claude")));
        Self {
            roots: ProviderRoots {
                codex_sessions: codex_home.map(|path| path.join("sessions")),
                claude_projects: claude_config.map(|path| path.join("projects")),
            },
            cache: Mutex::new(ObservationCache::default()),
        }
    }

    pub(crate) fn observe(&self, workers: &[ObservedWorker]) -> Vec<ObservedChildAgent> {
        let parents = provider_parents(workers);
        let parent_key = parents
            .values()
            .map(|parent| format!("{}\0{}\0{}", parent.provider, parent.kind, parent.value))
            .collect::<Vec<_>>();

        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.parent_key == parent_key
                && cache
                    .observed_at
                    .is_some_and(|observed_at| observed_at.elapsed() < CACHE_TTL)
            {
                return cache.children.clone();
            }
        }

        let (mut codex_metadata, mut statuses) = {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (cache.codex_metadata.clone(), cache.statuses.clone())
        };
        let mut children = Vec::new();
        if let Some(root) = self.roots.codex_sessions.as_deref() {
            children.extend(observe_codex(
                root,
                &parents,
                &mut codex_metadata,
                &mut statuses,
            ));
        }
        if let Some(root) = self.roots.claude_projects.as_deref() {
            children.extend(observe_claude(root, &parents, &mut statuses));
        }
        children.retain(|child| child.status == ObservedStatus::Working);
        children.sort_by(|left, right| {
            (
                left.parent_provider_session.value.as_str(),
                left.depth,
                left.name.as_deref().unwrap_or_default(),
                left.provider_agent_id.as_str(),
            )
                .cmp(&(
                    right.parent_provider_session.value.as_str(),
                    right.depth,
                    right.name.as_deref().unwrap_or_default(),
                    right.provider_agent_id.as_str(),
                ))
        });

        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.parent_key = parent_key;
        cache.observed_at = Some(Instant::now());
        cache.children.clone_from(&children);
        cache.codex_metadata = codex_metadata;
        cache.statuses = statuses;
        children
    }

    #[cfg(test)]
    fn with_roots(codex_sessions: PathBuf, claude_projects: PathBuf) -> Self {
        Self {
            roots: ProviderRoots {
                codex_sessions: Some(codex_sessions),
                claude_projects: Some(claude_projects),
            },
            cache: Mutex::new(ObservationCache::default()),
        }
    }
}

fn provider_parents(workers: &[ObservedWorker]) -> BTreeMap<String, ProviderSessionRef> {
    workers
        .iter()
        .filter_map(|worker| worker.provider_session.as_ref())
        .filter(|session| {
            matches!(
                session.provider.to_ascii_lowercase().as_str(),
                "codex" | "claude"
            ) && safe_session_component(&session.value)
        })
        .map(|session| {
            (
                format!(
                    "{}\0{}",
                    session.provider.to_ascii_lowercase(),
                    session.value
                ),
                session.clone(),
            )
        })
        .collect()
}

fn observe_codex(
    root: &Path,
    parents: &BTreeMap<String, ProviderSessionRef>,
    metadata_cache: &mut BTreeMap<PathBuf, CodexAgentMetadata>,
    status_cache: &mut BTreeMap<PathBuf, CachedStatus>,
) -> Vec<ObservedChildAgent> {
    let roots = parents
        .iter()
        .filter(|(key, _)| key.starts_with("codex\0"))
        .map(|(_, session)| (session.value.clone(), session.clone()))
        .collect::<BTreeMap<_, _>>();
    if roots.is_empty() {
        return Vec::new();
    }

    let previous_metadata = std::mem::take(metadata_cache);
    let mut metadata = BTreeMap::new();
    for path in codex_session_files(root)
        .into_iter()
        .take(MAX_PROVIDER_FILES)
    {
        let Some(mut agent) = previous_metadata
            .get(&path)
            .cloned()
            .or_else(|| read_codex_metadata(&path))
        else {
            continue;
        };
        agent.updated_at_unix_ms = modified_at_unix_ms(&path);
        let replace = metadata
            .get(&agent.id)
            .is_none_or(|current: &CodexAgentMetadata| {
                current.updated_at_unix_ms <= agent.updated_at_unix_ms
            });
        if replace {
            metadata.insert(agent.id.clone(), agent);
        }
    }
    *metadata_cache = metadata
        .values()
        .map(|agent| (agent.path.clone(), agent.clone()))
        .collect();

    let mut children_by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for agent in metadata.values() {
        children_by_parent
            .entry(agent.direct_parent_id.clone())
            .or_default()
            .push(agent.id.clone());
    }

    let mut observed = Vec::new();
    let mut visited = BTreeSet::new();
    let mut queue = roots
        .iter()
        .map(|(id, session)| (id.clone(), session.clone()))
        .collect::<VecDeque<_>>();
    while let Some((parent_id, root_session)) = queue.pop_front() {
        let Some(child_ids) = children_by_parent.get(&parent_id) else {
            continue;
        };
        for child_id in child_ids {
            if !visited.insert(child_id.clone()) {
                continue;
            }
            let Some(agent) = metadata.get(child_id) else {
                continue;
            };
            observed.push(ObservedChildAgent {
                runtime_id: format!("provider-child:codex:{}:{}", root_session.value, agent.id),
                parent_provider_session: root_session.clone(),
                parent_agent_id: (!roots.contains_key(&agent.direct_parent_id))
                    .then(|| agent.direct_parent_id.clone()),
                provider: "codex".to_owned(),
                provider_agent_id: agent.id.clone(),
                name: agent.nickname.clone(),
                description: None,
                role: agent.role.clone(),
                status: cached_status(&agent.path, status_cache, parse_codex_status),
                depth: agent.depth.max(1),
                updated_at_unix_ms: agent.updated_at_unix_ms,
            });
            queue.push_back((agent.id.clone(), root_session.clone()));
        }
    }
    observed
}

fn codex_session_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for year in directory_paths(root) {
        for month in directory_paths(&year) {
            for day in directory_paths(&month) {
                for path in file_paths(&day) {
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
                    {
                        files.push(path);
                        if files.len() >= MAX_PROVIDER_FILES {
                            return files;
                        }
                    }
                }
            }
        }
    }
    files
}

fn read_codex_metadata(path: &Path) -> Option<CodexAgentMetadata> {
    let prefix = read_prefix(path, MAX_METADATA_BYTES)?;
    let first_line = prefix.split(|byte| *byte == b'\n').next()?;
    let record: Value = serde_json::from_slice(first_line).ok()?;
    if record.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = record.get("payload")?;
    let thread_spawn = payload.pointer("/source/subagent/thread_spawn")?;
    let id = json_string(payload, &["id"])?;
    let direct_parent_id = json_string(thread_spawn, &["parent_thread_id"])?;
    if json_string(payload, &["parent_thread_id"])
        .is_some_and(|parent_id| parent_id != direct_parent_id)
    {
        return None;
    }
    let depth = json_u64(thread_spawn, &["depth"])
        .unwrap_or(1)
        .try_into()
        .unwrap_or(u32::MAX);
    Some(CodexAgentMetadata {
        id,
        direct_parent_id,
        nickname: json_string(payload, &["agent_nickname"])
            .or_else(|| json_string(thread_spawn, &["agent_nickname"])),
        role: json_string(payload, &["agent_role"])
            .or_else(|| json_string(thread_spawn, &["agent_role"])),
        depth,
        path: path.to_path_buf(),
        updated_at_unix_ms: modified_at_unix_ms(path),
    })
}

fn parse_codex_status(path: &Path, updated_at_unix_ms: u64) -> ObservedStatus {
    let mut status = None;
    if let Some(tail) = read_tail(path, MAX_TAIL_BYTES) {
        for line in tail.split(|byte| *byte == b'\n') {
            let Ok(record) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if record.get("type").and_then(Value::as_str) != Some("event_msg") {
                continue;
            }
            status = match record.pointer("/payload/type").and_then(Value::as_str) {
                Some("task_started" | "user_message") => Some(ObservedStatus::Working),
                Some("task_complete")
                    if record
                        .pointer("/payload/error")
                        .is_some_and(|error| !error.is_null()) =>
                {
                    Some(ObservedStatus::Blocked)
                }
                Some("task_complete") => Some(ObservedStatus::Done),
                Some("turn_aborted") => Some(ObservedStatus::Idle),
                _ => status,
            };
        }
    }
    let status = status.unwrap_or_else(|| {
        if is_recent(updated_at_unix_ms) {
            ObservedStatus::Working
        } else {
            ObservedStatus::Unknown
        }
    });
    settle_stale_activity(status, updated_at_unix_ms)
}

fn observe_claude(
    root: &Path,
    parents: &BTreeMap<String, ProviderSessionRef>,
    status_cache: &mut BTreeMap<PathBuf, CachedStatus>,
) -> Vec<ObservedChildAgent> {
    let roots = parents
        .iter()
        .filter(|(key, _)| key.starts_with("claude\0"))
        .map(|(_, session)| session)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Vec::new();
    }

    let project_paths = directory_paths(root);
    let mut observed = Vec::new();
    for root_session in roots {
        for project_path in &project_paths {
            let subagents = project_path.join(&root_session.value).join("subagents");
            for metadata_path in file_paths(&subagents) {
                if observed.len() >= MAX_PROVIDER_FILES {
                    return observed;
                }
                let Some(file_name) = metadata_path.file_name().and_then(|name| name.to_str())
                else {
                    continue;
                };
                let Some(agent_id) = file_name
                    .strip_prefix("agent-")
                    .and_then(|name| name.strip_suffix(".meta.json"))
                else {
                    continue;
                };
                let Some(metadata) = read_json_value(&metadata_path) else {
                    continue;
                };
                let transcript_path =
                    metadata_path.with_file_name(format!("agent-{agent_id}.jsonl"));
                let updated_at_unix_ms =
                    modified_at_unix_ms(&metadata_path).max(modified_at_unix_ms(&transcript_path));
                let depth = metadata
                    .get("spawnDepth")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .try_into()
                    .unwrap_or(u32::MAX);
                observed.push(ObservedChildAgent {
                    runtime_id: format!("provider-child:claude:{}:{agent_id}", root_session.value),
                    parent_provider_session: (*root_session).clone(),
                    parent_agent_id: None,
                    provider: "claude".to_owned(),
                    provider_agent_id: agent_id.to_owned(),
                    name: metadata
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    description: metadata
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    role: metadata
                        .get("agentType")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    status: if metadata
                        .get("stoppedByUser")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        ObservedStatus::Done
                    } else {
                        cached_status(&transcript_path, status_cache, parse_claude_status)
                    },
                    depth: depth.max(1),
                    updated_at_unix_ms,
                });
            }
        }
    }
    observed
}

fn parse_claude_status(path: &Path, updated_at_unix_ms: u64) -> ObservedStatus {
    let mut status = None;
    if let Some(tail) = read_tail(path, MAX_TAIL_BYTES) {
        for line in tail.split(|byte| *byte == b'\n') {
            let Ok(record) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let role = record.pointer("/message/role").and_then(Value::as_str);
            let stop_reason = record
                .pointer("/message/stop_reason")
                .and_then(Value::as_str);
            let is_api_error = record
                .get("isApiErrorMessage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            status = match (role, stop_reason, is_api_error) {
                (_, _, true) => Some(ObservedStatus::Blocked),
                (Some("assistant"), Some("end_turn" | "stop_sequence"), false) => {
                    Some(ObservedStatus::Done)
                }
                (Some("assistant" | "user"), _, false) => Some(ObservedStatus::Working),
                _ => status,
            };
        }
    }
    let status = status.unwrap_or_else(|| {
        if is_recent(updated_at_unix_ms) {
            ObservedStatus::Working
        } else {
            ObservedStatus::Unknown
        }
    });
    settle_stale_activity(status, updated_at_unix_ms)
}

fn cached_status(
    path: &Path,
    cache: &mut BTreeMap<PathBuf, CachedStatus>,
    parse: impl FnOnce(&Path, u64) -> ObservedStatus,
) -> ObservedStatus {
    let Some(signature) = file_signature(path) else {
        return ObservedStatus::Unknown;
    };
    if let Some(cached) = cache.get(path)
        && cached.signature == signature
    {
        return settle_stale_activity(cached.status, signature.modified_at_unix_ms);
    }
    let status = parse(path, signature.modified_at_unix_ms);
    cache.insert(path.to_path_buf(), CachedStatus { signature, status });
    status
}

fn settle_stale_activity(status: ObservedStatus, updated_at_unix_ms: u64) -> ObservedStatus {
    if status == ObservedStatus::Working && !is_recent(updated_at_unix_ms) {
        ObservedStatus::Unknown
    } else {
        status
    }
}

fn is_recent(updated_at_unix_ms: u64) -> bool {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| now.checked_sub(Duration::from_millis(updated_at_unix_ms)))
        .is_some_and(|age| age <= ACTIVE_WINDOW)
}

fn safe_session_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn directory_paths(path: &Path) -> Vec<PathBuf> {
    read_paths(path, true)
}

fn file_paths(path: &Path) -> Vec<PathBuf> {
    read_paths(path, false)
}

fn read_paths(path: &Path, directories: bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            ((directories && file_type.is_dir()) || (!directories && file_type.is_file()))
                .then(|| entry.path())
        })
        .collect()
}

fn read_json_value(path: &Path) -> Option<Value> {
    serde_json::from_slice(&read_prefix(path, MAX_METADATA_BYTES)?).ok()
}

fn read_prefix(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn read_tail(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    if start > 0 {
        let first_newline = bytes.iter().position(|byte| *byte == b'\n')?;
        bytes.drain(..=first_newline);
    }
    Some(bytes)
}

fn modified_at_unix_ms(path: &Path) -> u64 {
    file_signature(path).map_or(0, |signature| signature.modified_at_unix_ms)
}

fn file_signature(path: &Path) -> Option<FileSignature> {
    let metadata = fs::metadata(path).ok()?;
    let modified_at_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| duration.as_millis().try_into().ok())
        .unwrap_or(0);
    Some(FileSignature {
        length: metadata.len(),
        modified_at_unix_ms,
    })
}

fn json_string(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn json_u64(value: &Value, path: &[&str]) -> Option<u64> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use tempfile::TempDir;
    use yard_domain::{ObservedStatus, ObservedWorker, ProviderSessionRef};

    use super::ProviderAgentObserver;

    fn worker(provider: &str, session_id: &str) -> ObservedWorker {
        ObservedWorker {
            runtime_id: format!("terminal-{provider}"),
            terminal_id: format!("terminal-{provider}"),
            workspace_id: "workspace".to_owned(),
            tab_id: "tab".to_owned(),
            pane_id: "pane".to_owned(),
            name: None,
            provider: Some(provider.to_owned()),
            display_provider: None,
            status: ObservedStatus::Idle,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: 1,
            cwd: None,
            foreground_cwd: None,
            tokens: BTreeMap::new(),
            provider_session: Some(ProviderSessionRef {
                source: format!("herdr:{provider}"),
                provider: provider.to_owned(),
                kind: "id".to_owned(),
                value: session_id.to_owned(),
            }),
            revision: 1,
        }
    }

    #[test]
    fn observes_active_children_and_hides_completed_children() {
        let temp = TempDir::new().unwrap();
        let codex = temp.path().join("codex/2026/08/12");
        let claude = temp.path().join("claude/project/claude-parent/subagents");
        fs::create_dir_all(&codex).unwrap();
        fs::create_dir_all(&claude).unwrap();

        fs::write(
            codex.join("rollout-child.jsonl"),
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-child\",",
                "\"parent_thread_id\":\"codex-parent\",\"agent_nickname\":\"Rawls\",",
                "\"agent_role\":\"explorer\",\"source\":{\"subagent\":{\"thread_spawn\":",
                "{\"parent_thread_id\":\"codex-parent\",\"depth\":1}}}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n"
            ),
        )
        .unwrap();
        fs::write(
            codex.join("rollout-completed-child.jsonl"),
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-completed\",",
                "\"parent_thread_id\":\"codex-parent\",\"agent_nickname\":\"Finished\",",
                "\"source\":{\"subagent\":{\"thread_spawn\":",
                "{\"parent_thread_id\":\"codex-parent\",\"depth\":1}}}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",",
                "\"error\":null}}\n"
            ),
        )
        .unwrap();
        fs::write(
            claude.join("agent-claude-child.meta.json"),
            "{\"agentType\":\"Explore\",\"description\":\"Inspect parser\",\"spawnDepth\":1}",
        )
        .unwrap();
        fs::write(
            claude.join("agent-claude-child.jsonl"),
            concat!(
                "{\"type\":\"assistant\",\"sessionId\":\"claude-parent\",",
                "\"agentId\":\"claude-child\",\"isSidechain\":true,",
                "\"message\":{\"role\":\"assistant\",\"stop_reason\":null}}\n"
            ),
        )
        .unwrap();

        let observer = ProviderAgentObserver::with_roots(
            temp.path().join("codex"),
            temp.path().join("claude"),
        );
        let children = observer.observe(&[
            worker("codex", "codex-parent"),
            worker("claude", "claude-parent"),
        ]);

        assert_eq!(children.len(), 2);
        let codex_child = children
            .iter()
            .find(|child| child.provider == "codex")
            .unwrap();
        assert_eq!(codex_child.name.as_deref(), Some("Rawls"));
        assert_eq!(codex_child.status, ObservedStatus::Working);
        let claude_child = children
            .iter()
            .find(|child| child.provider == "claude")
            .unwrap();
        assert_eq!(claude_child.description.as_deref(), Some("Inspect parser"));
        assert_eq!(claude_child.status, ObservedStatus::Working);
        assert!(
            children
                .iter()
                .all(|child| child.provider_agent_id != "codex-completed")
        );
    }
}

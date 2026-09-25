use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use thiserror::Error;
use tokio::sync::Mutex;
use yard_domain::{
    ArchitectureEcosystem, ArchitectureEdge, ArchitectureNode, ArchitectureNodeKind,
    ArchitectureRepositoryStatus, ProjectArchitecture, ProjectRepositories, ProjectRepository,
    RepositoryArchitecture,
};
use yard_store::{ProjectStoreError, YardStore};

const CACHE_TTL: Duration = Duration::from_secs(30);
const MAX_REPOSITORIES: usize = 16;
const MAX_DEPTH: usize = 16;
const MAX_ENTRIES: usize = 10_000;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_NODES: usize = 1_000;
const MAX_EDGES: usize = 4_000;
const MAX_ERRORS: usize = 64;
const MAX_NAME_BYTES: usize = 256;
const MAX_RELATIVE_PATH_BYTES: usize = 1_024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct ArchitectureService {
    store: Arc<dyn YardStore>,
    cache: Arc<Mutex<HashMap<String, CachedArchitecture>>>,
}

#[derive(Clone)]
struct CachedArchitecture {
    architecture: ProjectArchitecture,
    repository_signature: Vec<(String, String, u64)>,
    cached_at: Instant,
    refreshing: bool,
}

impl ArchitectureService {
    #[must_use]
    pub fn new(store: Arc<dyn YardStore>) -> Self {
        Self {
            store,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return a bounded architecture projection for the project's linked repositories.
    ///
    /// Cached results are returned immediately after the short freshness window,
    /// marked stale, while one background refresh replaces them.
    ///
    /// # Errors
    ///
    /// Returns [`ArchitectureServiceError`] when project repositories cannot be
    /// loaded or a blocking scan task cannot run.
    pub async fn get(
        &self,
        project_id: &str,
    ) -> Result<ProjectArchitecture, ArchitectureServiceError> {
        let mut repositories = self.store.list_project_repositories(project_id).await?;
        repositories.repositories.sort_by(|a, b| a.id.cmp(&b.id));
        let signature = repository_signature(&repositories);
        let repositories_truncated = repositories.repositories.len() > MAX_REPOSITORIES;
        repositories.repositories.truncate(MAX_REPOSITORIES);
        let cached = {
            let mut cache = self.cache.lock().await;
            cache.get_mut(project_id).and_then(|entry| {
                if entry.repository_signature != signature {
                    return None;
                }
                let stale = entry.cached_at.elapsed() > CACHE_TTL;
                let refresh = stale && !entry.refreshing;
                if refresh {
                    entry.refreshing = true;
                }
                let mut architecture = entry.architecture.clone();
                architecture.stale = stale;
                Some((architecture, refresh))
            })
        };
        if let Some((architecture, refresh)) = cached {
            if refresh {
                let service = self.clone();
                let project_id = project_id.to_owned();
                tokio::spawn(async move {
                    if service
                        .refresh(
                            project_id.clone(),
                            repositories.repositories,
                            signature,
                            repositories_truncated,
                        )
                        .await
                        .is_err()
                    {
                        if let Some(entry) = service.cache.lock().await.get_mut(&project_id) {
                            entry.refreshing = false;
                        }
                    }
                });
            }
            return Ok(architecture);
        }

        self.refresh(
            project_id.to_owned(),
            repositories.repositories,
            signature,
            repositories_truncated,
        )
        .await
    }

    async fn refresh(
        &self,
        project_id: String,
        repositories: Vec<ProjectRepository>,
        signature: Vec<(String, String, u64)>,
        repositories_truncated: bool,
    ) -> Result<ProjectArchitecture, ArchitectureServiceError> {
        let mut valid = Vec::new();
        let mut invalid = Vec::new();
        for repository in repositories {
            if crate::project_service::validate_repository_identity_for_read(&repository)
                .await
                .is_ok()
            {
                valid.push(repository);
            } else {
                invalid.push(unavailable_repository(&repository));
            }
        }
        let scan_project_id = project_id.clone();
        let architecture = tokio::task::spawn_blocking(move || {
            scan_project_architecture_with_errors(
                &scan_project_id,
                &valid,
                invalid,
                repositories_truncated,
            )
        })
        .await
        .map_err(ArchitectureServiceError::ScanTask)?;
        self.cache.lock().await.insert(
            project_id,
            CachedArchitecture {
                architecture: architecture.clone(),
                repository_signature: signature,
                cached_at: Instant::now(),
                refreshing: false,
            },
        );
        Ok(architecture)
    }
}

fn repository_signature(repositories: &ProjectRepositories) -> Vec<(String, String, u64)> {
    repositories
        .repositories
        .iter()
        .map(|repository| {
            (
                repository.id.clone(),
                repository.root_path.clone(),
                repository.updated_at_unix_ms,
            )
        })
        .collect()
}

#[cfg(test)]
fn scan_project_architecture(
    project_id: &str,
    repositories: &[ProjectRepository],
) -> ProjectArchitecture {
    scan_project_architecture_with_errors(project_id, repositories, Vec::new(), false)
}

fn scan_project_architecture_with_errors(
    project_id: &str,
    repositories: &[ProjectRepository],
    mut invalid: Vec<RepositoryArchitecture>,
    repositories_truncated: bool,
) -> ProjectArchitecture {
    let mut scanned: Vec<_> = repositories.iter().map(scan_repository_nodes).collect();
    resolve_edges(&mut scanned);
    invalid.extend(scanned.into_iter().map(|repository| repository.projection));
    let mut projection = ProjectArchitecture {
        project_id: project_id.to_owned(),
        repositories: invalid,
        scanned_at_unix_ms: now_unix_ms(),
        stale: false,
        truncated: repositories_truncated,
    };
    projection
        .repositories
        .sort_by(|a, b| a.repository_id.cmp(&b.repository_id));
    enforce_project_limits(&mut projection);
    projection
}

fn unavailable_repository(repository: &ProjectRepository) -> RepositoryArchitecture {
    RepositoryArchitecture {
        repository_id: repository.id.clone(),
        name: Path::new(&repository.root_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository")
            .to_owned(),
        status: ArchitectureRepositoryStatus::Error,
        nodes: Vec::new(),
        edges: Vec::new(),
        errors: vec!["stored repository identity changed or is unavailable".to_owned()],
        truncated: false,
    }
}

#[cfg(test)]
fn scan_repository(repository: &ProjectRepository) -> RepositoryArchitecture {
    let mut scanned = vec![scan_repository_nodes(repository)];
    resolve_edges(&mut scanned);
    scanned.pop().expect("one scanned repository").projection
}

#[allow(clippy::too_many_lines)]
fn scan_repository_nodes(repository: &ProjectRepository) -> ScannedRepository {
    let name = Path::new(&repository.root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repository")
        .to_owned();
    let mut result = RepositoryArchitecture {
        repository_id: repository.id.clone(),
        name,
        status: ArchitectureRepositoryStatus::Unsupported,
        nodes: Vec::new(),
        edges: Vec::new(),
        errors: Vec::new(),
        truncated: false,
    };
    let root = match fs::canonicalize(&repository.root_path) {
        Ok(root) if root.is_dir() => root,
        _ => {
            result.status = ArchitectureRepositoryStatus::Error;
            push_error(&mut result, "repository root is unavailable");
            return ScannedRepository {
                dependencies: Vec::new(),
                projection: result,
            };
        }
    };

    let mut manifests_seen = 0;
    let mut entries_seen = 0;
    let mut detected = Vec::new();
    let mut stack = vec![(root.clone(), 0_usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            result.truncated = true;
            continue;
        }
        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                push_error(
                    &mut result,
                    format!("{}: {error}", relative_path(&root, &directory)),
                );
                continue;
            }
        };
        for entry in read_dir {
            entries_seen += 1;
            if entries_seen > MAX_ENTRIES {
                result.truncated = true;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    push_error(&mut result, format!("directory entry: {error}"));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    push_error(
                        &mut result,
                        format!("{}: {error}", relative_path(&root, &path)),
                    );
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                if fs::canonicalize(&path).is_ok_and(|target| !target.starts_with(&root)) {
                    push_error(
                        &mut result,
                        format!(
                            "{}: symlink escapes repository root",
                            relative_path(&root, &path)
                        ),
                    );
                }
                continue;
            }
            if metadata.is_dir() {
                if !skip_directory(&entry.file_name()) {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let Some(analyzer) = ANALYZERS
                .iter()
                .find(|analyzer| entry.file_name() == analyzer.file_name)
            else {
                continue;
            };
            manifests_seen += 1;
            let manifest_path = relative_path(&root, &path);
            if manifest_path.len() > MAX_RELATIVE_PATH_BYTES {
                result.truncated = true;
                push_error(&mut result, "manifest path exceeds the scan limit");
                continue;
            }
            if metadata.len() > MAX_MANIFEST_BYTES {
                push_error(
                    &mut result,
                    format!("{manifest_path}: manifest is too large"),
                );
                continue;
            }
            let source = match fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) => {
                    push_error(&mut result, format!("{manifest_path}: {error}"));
                    continue;
                }
            };
            match (analyzer.parse)(&path, &source) {
                Ok(Some(manifest))
                    if detected.len() < MAX_NODES && manifest.name.len() <= MAX_NAME_BYTES =>
                {
                    let id = format!(
                        "{}:{}:{}",
                        repository.id,
                        ecosystem_name(analyzer.ecosystem),
                        manifest_path
                    );
                    detected.push(DetectedNode {
                        dependencies: manifest.dependencies,
                        node: ArchitectureNode {
                            id,
                            kind: manifest.kind,
                            name: manifest.name,
                            manifest_path,
                            ecosystem: analyzer.ecosystem,
                        },
                    });
                }
                Ok(Some(manifest)) if manifest.name.len() > MAX_NAME_BYTES => {
                    result.truncated = true;
                    push_error(&mut result, "package name exceeds the scan limit");
                }
                Ok(Some(_)) => result.truncated = true,
                Ok(None) => {}
                Err(error) => push_error(&mut result, format!("{manifest_path}: {error}")),
            }
        }
        if entries_seen > MAX_ENTRIES {
            break;
        }
    }

    detected.sort_by(|a, b| a.node.id.cmp(&b.node.id));
    result.nodes = detected
        .iter()
        .map(|detected| detected.node.clone())
        .collect();
    result.status = if !result.errors.is_empty() {
        if result.nodes.is_empty() {
            ArchitectureRepositoryStatus::Error
        } else {
            ArchitectureRepositoryStatus::Partial
        }
    } else if manifests_seen == 0 {
        ArchitectureRepositoryStatus::Unsupported
    } else {
        ArchitectureRepositoryStatus::Ready
    };
    ScannedRepository {
        dependencies: detected
            .into_iter()
            .map(|detected| DeclaredDependencies {
                dependencies: detected.dependencies,
                ecosystem: detected.node.ecosystem,
                node_id: detected.node.id,
            })
            .collect(),
        projection: result,
    }
}

fn resolve_edges(repositories: &mut [ScannedRepository]) {
    let mut node_by_name = HashMap::new();
    let mut ambiguous = HashSet::new();
    for repository in repositories.iter() {
        for node in &repository.projection.nodes {
            let key = (node.ecosystem, node.name.clone());
            if node_by_name.insert(key.clone(), node.id.clone()).is_some() {
                ambiguous.insert(key);
            }
        }
    }
    let mut edges_left = MAX_EDGES;
    for repository in repositories {
        for declared in &repository.dependencies {
            for dependency in &declared.dependencies {
                let key = (declared.ecosystem, dependency.clone());
                if ambiguous.contains(&key) {
                    continue;
                }
                if let Some(target) = node_by_name.get(&key) {
                    if edges_left == 0 {
                        repository.projection.truncated = true;
                        break;
                    }
                    repository.projection.edges.push(ArchitectureEdge {
                        from: declared.node_id.clone(),
                        to: target.clone(),
                    });
                    edges_left -= 1;
                }
            }
        }
        repository
            .projection
            .edges
            .sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        repository
            .projection
            .edges
            .dedup_by(|a, b| a.from == b.from && a.to == b.to);
    }
}

fn enforce_project_limits(projection: &mut ProjectArchitecture) {
    let mut nodes_left = MAX_NODES;
    for repository in &mut projection.repositories {
        if repository.nodes.len() > nodes_left {
            repository.nodes.truncate(nodes_left);
            repository.truncated = true;
        }
        nodes_left = nodes_left.saturating_sub(repository.nodes.len());
        projection.truncated |= repository.truncated;
    }
    let node_ids: HashSet<String> = projection
        .repositories
        .iter()
        .flat_map(|repository| repository.nodes.iter())
        .map(|node| node.id.clone())
        .collect();
    let mut edges_left = MAX_EDGES;
    for repository in &mut projection.repositories {
        repository.edges.retain(|edge| {
            node_ids.contains(edge.from.as_str()) && node_ids.contains(edge.to.as_str())
        });
        if repository.edges.len() > edges_left {
            repository.edges.truncate(edges_left);
            repository.truncated = true;
        }
        edges_left = edges_left.saturating_sub(repository.edges.len());
        projection.truncated |= repository.truncated;
    }
    while serde_json::to_vec(projection).is_ok_and(|body| body.len() > MAX_RESPONSE_BYTES) {
        let Some(repository) = projection
            .repositories
            .iter_mut()
            .rev()
            .find(|repository| !repository.edges.is_empty() || !repository.nodes.is_empty())
        else {
            break;
        };
        if repository.edges.is_empty() {
            repository.nodes.truncate(repository.nodes.len() / 2);
        } else {
            repository.edges.truncate(repository.edges.len() / 2);
        }
        repository.truncated = true;
        projection.truncated = true;
    }
}

struct ManifestAnalyzer {
    file_name: &'static str,
    ecosystem: ArchitectureEcosystem,
    parse: fn(&Path, &str) -> Result<Option<ParsedManifest>, String>,
}

const ANALYZERS: [ManifestAnalyzer; 2] = [
    ManifestAnalyzer {
        file_name: "Cargo.toml",
        ecosystem: ArchitectureEcosystem::Cargo,
        parse: parse_cargo_manifest,
    },
    ManifestAnalyzer {
        file_name: "package.json",
        ecosystem: ArchitectureEcosystem::Npm,
        parse: parse_npm_manifest,
    },
];

struct ParsedManifest {
    name: String,
    kind: ArchitectureNodeKind,
    dependencies: Vec<String>,
}

struct DetectedNode {
    node: ArchitectureNode,
    dependencies: Vec<String>,
}

struct DeclaredDependencies {
    dependencies: Vec<String>,
    ecosystem: ArchitectureEcosystem,
    node_id: String,
}

struct ScannedRepository {
    dependencies: Vec<DeclaredDependencies>,
    projection: RepositoryArchitecture,
}

fn parse_cargo_manifest(_path: &Path, source: &str) -> Result<Option<ParsedManifest>, String> {
    let manifest: toml::Value = toml::from_str(source).map_err(|error| error.to_string())?;
    let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
        return Ok(None);
    };
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "package.name is required".to_owned())?
        .to_owned();
    let kind = if manifest.get("lib").is_some() {
        ArchitectureNodeKind::Library
    } else if manifest.get("bin").is_some() {
        ArchitectureNodeKind::Application
    } else {
        ArchitectureNodeKind::Package
    };
    let mut dependencies = Vec::new();
    collect_cargo_dependencies(&manifest, &mut dependencies);
    dependencies.sort();
    dependencies.dedup();
    Ok(Some(ParsedManifest {
        name,
        kind,
        dependencies,
    }))
}

fn collect_cargo_dependencies(value: &toml::Value, dependencies: &mut Vec<String>) {
    let Some(table) = value.as_table() else {
        return;
    };
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(entries) = table.get(key).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, specification) in entries {
            dependencies.push(
                specification
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(name)
                    .to_owned(),
            );
        }
    }
    if let Some(targets) = table.get("target").and_then(toml::Value::as_table) {
        for value in targets.values() {
            collect_cargo_dependencies(value, dependencies);
        }
    }
}

fn parse_npm_manifest(_path: &Path, source: &str) -> Result<Option<ParsedManifest>, String> {
    let manifest: serde_json::Value =
        serde_json::from_str(source).map_err(|error| error.to_string())?;
    let name = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "name is required".to_owned())?
        .to_owned();
    let kind = if manifest.get("bin").is_some() {
        ArchitectureNodeKind::Application
    } else {
        ArchitectureNodeKind::Package
    };
    let mut dependencies = Vec::new();
    for field in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if let Some(entries) = manifest.get(field).and_then(serde_json::Value::as_object) {
            dependencies.extend(entries.keys().cloned());
        }
    }
    dependencies.sort();
    dependencies.dedup();
    Ok(Some(ParsedManifest {
        name,
        kind,
        dependencies,
    }))
}

fn skip_directory(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | "build" | "dist" | "node_modules" | "out" | "target" | "vendor")
    )
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn ecosystem_name(ecosystem: ArchitectureEcosystem) -> &'static str {
    match ecosystem {
        ArchitectureEcosystem::Cargo => "cargo",
        ArchitectureEcosystem::Npm => "npm",
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn push_error(result: &mut RepositoryArchitecture, message: impl AsRef<str>) {
    if result.errors.len() < MAX_ERRORS {
        result
            .errors
            .push(bounded_text(message.as_ref(), MAX_NAME_BYTES * 2));
    } else {
        result.truncated = true;
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[derive(Debug, Error)]
pub enum ArchitectureServiceError {
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error("architecture scan task failed: {0}")]
    ScanTask(tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Instant};

    use tempfile::TempDir;

    use super::*;

    fn repository(id: &str, root: &Path) -> ProjectRepository {
        ProjectRepository {
            id: id.to_owned(),
            project_id: "project-1".to_owned(),
            root_path: root.to_string_lossy().into_owned(),
            git_common_dir: root.join(".git").to_string_lossy().into_owned(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn detects_cargo_workspace_and_npm_workspaces_with_internal_edges() {
        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        write(
            &temp.path().join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[[bin]]\nname = \"app\"\n[dependencies]\ncore = { path = \"../core\" }\n",
        );
        write(
            &temp.path().join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\n[lib]\n",
        );
        write(
            &temp.path().join("package.json"),
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"dependencies":{"ui":"workspace:*"}}"#,
        );
        write(
            &temp.path().join("packages/ui/package.json"),
            r#"{"name":"ui","version":"1.0.0"}"#,
        );

        let result = scan_repository(&repository("repo-1", temp.path()));

        assert_eq!(
            result.status,
            ArchitectureRepositoryStatus::Ready,
            "{:?}",
            result.errors
        );
        assert_eq!(result.nodes.len(), 4);
        assert!(
            result.nodes.iter().any(|node| {
                node.name == "app" && node.kind == ArchitectureNodeKind::Application
            })
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|node| { node.name == "core" && node.kind == ArchitectureNodeKind::Library })
        );
        assert_eq!(result.edges.len(), 2);
    }

    #[test]
    fn reports_multiple_none_unsupported_and_malformed_repositories() {
        let cargo = TempDir::new().unwrap();
        write(
            &cargo.path().join("Cargo.toml"),
            "[package]\nname = \"one\"\nversion = \"0.1.0\"\n",
        );
        let unsupported = TempDir::new().unwrap();
        write(&unsupported.path().join("README.md"), "plain files");
        let malformed = TempDir::new().unwrap();
        write(&malformed.path().join("package.json"), "{");

        let empty = scan_project_architecture("project-1", &[]);
        assert!(empty.repositories.is_empty());

        let result = scan_project_architecture(
            "project-1",
            &[
                repository("cargo", cargo.path()),
                repository("unsupported", unsupported.path()),
                repository("malformed", malformed.path()),
            ],
        );
        assert_eq!(result.repositories.len(), 3);
        assert_eq!(
            result.repositories[1].status,
            ArchitectureRepositoryStatus::Error
        );
        assert_eq!(
            result.repositories[2].status,
            ArchitectureRepositoryStatus::Unsupported
        );
    }

    #[test]
    fn stable_ids_survive_repeated_loads_and_added_nodes() {
        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join("a/package.json"),
            r#"{"name":"a","version":"1.0.0"}"#,
        );
        let repository = repository("repo-1", temp.path());
        let first = scan_repository(&repository);
        let repeated = scan_repository(&repository);
        assert_eq!(first.nodes, repeated.nodes);

        write(
            &temp.path().join("b/package.json"),
            r#"{"name":"b","version":"1.0.0"}"#,
        );
        let expanded = scan_repository(&repository);
        assert_eq!(first.nodes[0], expanded.nodes[0]);
    }

    #[test]
    fn dependency_cycles_remain_declared_edges() {
        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join("a/package.json"),
            r#"{"name":"a","dependencies":{"b":"1"}}"#,
        );
        write(
            &temp.path().join("b/package.json"),
            r#"{"name":"b","dependencies":{"a":"1"}}"#,
        );
        let result = scan_repository(&repository("repo-1", temp.path()));
        assert_eq!(result.edges.len(), 2);
    }

    #[test]
    fn resolves_declared_dependencies_across_repository_associations() {
        let application = TempDir::new().unwrap();
        let library = TempDir::new().unwrap();
        write(
            &application.path().join("package.json"),
            r#"{"name":"application","dependencies":{"library":"1"}}"#,
        );
        write(
            &library.path().join("package.json"),
            r#"{"name":"library"}"#,
        );

        let result = scan_project_architecture(
            "project-1",
            &[
                repository("application-repository", application.path()),
                repository("library-repository", library.path()),
            ],
        );

        assert_eq!(result.repositories[0].edges.len(), 1);
        assert_eq!(
            result.repositories[0].edges[0].to,
            result.repositories[1].nodes[0].id
        );
    }

    #[test]
    fn truncates_huge_repositories_within_a_generous_time_bound() {
        let temp = TempDir::new().unwrap();
        for index in 0..1_100 {
            write(
                &temp.path().join(format!("packages/{index}/package.json")),
                &format!(r#"{{"name":"package-{index}"}}"#),
            );
        }
        let started = Instant::now();
        let result = scan_repository(&repository("repo-1", temp.path()));
        assert!(result.truncated);
        assert_eq!(result.nodes.len(), MAX_NODES);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape_the_repository() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(
            &outside.path().join("package.json"),
            r#"{"name":"outside"}"#,
        );
        symlink(outside.path(), temp.path().join("escape")).unwrap();

        let result = scan_repository(&repository("repo-1", temp.path()));
        assert_eq!(result.status, ArchitectureRepositoryStatus::Error);
        assert!(result.errors[0].contains("escapes repository root"));
        assert!(result.nodes.is_empty());
    }
}

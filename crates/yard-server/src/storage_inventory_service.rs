//! Read-only storage scan (design D9, PR3 preview only).
//!
//! The server decides what to scan: only the canonical `YARD_STORAGE_ROOTS`.
//! One scan runs at a time; the filesystem walk runs in `spawn_blocking`
//! with entry, depth, and time budgets, and Git probes run one at a time
//! afterwards. Only the last result is kept, in memory, for 15 minutes.
//! There is no deletion code here: a scan only classifies candidates as
//! `safe`, `review`, or `blocked`.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, hash_map::Entry},
    ffi::OsString,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::watch;

use yard_domain::{
    RuntimeInventory, StorageCandidate, StorageClass, StorageInUseCheck, StorageOwner,
    StorageOwnerKind, StorageSafety, StorageSafetyTotal, StorageScan, StorageScanStatus,
    StorageTotals, StorageTruncationReason, StorageWorktree,
};
use yard_store::YardStore;

use crate::{
    config::ServerConfig,
    git_probe::{self, GitError, GitRunner, WorktreeEntry},
    inventory_service::InventorySource,
    reconciliation_service::ReconciliationService,
    storage_classify::{
        CandidateFacts, GitCheck, InUse, PackageProbe, PackageResult, RecordedPath, SourceSummary,
        WORKSPACE_ENV_ACKNOWLEDGEMENT, WorktreeFacts, classify, contains, is_in_use, resolve_owner,
        source_summary,
    },
    storage_walk::{self, NestedGit, RepoScope, WalkCandidate, WalkOutput, WalkSettings},
};

/// How long the last scan result is kept in memory.
pub const STORAGE_SCAN_TTL: Duration = Duration::from_secs(15 * 60);

/// Bounds that keep one scan from freezing Yard or the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageScanBudgets {
    /// Directory entries read across the whole walk.
    pub max_entries: u64,
    /// Wall time for the filesystem walk.
    pub walk_time: Duration,
    /// Wall time for the whole scan, including Git probes.
    pub total_time: Duration,
    /// Timeout of one Git command.
    pub git_timeout: Duration,
    /// Wall time for the live Herdr observations used by the in-use check.
    pub observe_time: Duration,
    /// Directory depth below a root at which candidates are still found.
    pub max_discovery_depth: usize,
    /// Directory depth inside one candidate that is still sized.
    pub max_size_depth: usize,
    pub max_candidates: usize,
    pub max_workspace_packages: usize,
    pub max_processes: usize,
}

impl Default for StorageScanBudgets {
    fn default() -> Self {
        Self {
            // Entries cost little (about 3M in 17 s on one core on
            // the dev host); the wall-time budget is the one meant to bind, so
            // this only stops a pathological tree. Nothing is kept per entry.
            max_entries: 25_000_000,
            walk_time: Duration::from_secs(120),
            total_time: Duration::from_secs(240),
            git_timeout: Duration::from_secs(15),
            observe_time: Duration::from_secs(60),
            max_discovery_depth: 12,
            max_size_depth: 64,
            max_candidates: 10_000,
            max_workspace_packages: 256,
            max_processes: 65_536,
        }
    }
}

/// Everything a scan needs besides the store and Herdr.
#[derive(Debug, Clone)]
pub struct StorageScanSettings {
    /// Canonical roots; `None` means not configured (nothing is scanned).
    pub roots: Option<Vec<PathBuf>>,
    pub log_retention_days: u32,
    /// File names that mark a workspace root (`YARD_STORAGE_WORKSPACE_MARKERS`).
    /// Empty disables the workspace classes.
    pub workspace_markers: Vec<String>,
    /// Yard's database, artifact, coordination, knowledge, and lifecycle
    /// runtime paths. No candidate may be inside or contain one.
    pub excluded_paths: Vec<PathBuf>,
    /// The running executable; a candidate containing it is excluded. A scan
    /// fails closed when it is unknown.
    pub current_exe: Option<PathBuf>,
    /// Linux process table used by the in-use check (`/proc`).
    pub proc_root: PathBuf,
    pub git_binary: OsString,
    /// Extra environment for Git (tests isolate the global configuration).
    pub git_env: Vec<(OsString, OsString)>,
    /// Directories Git never enters while discovering the repository that
    /// encloses a root (`GIT_CEILING_DIRECTORIES`; tests pin it to their
    /// temporary directory so nothing above it can influence a scan).
    pub git_ceilings: Vec<PathBuf>,
    pub budgets: StorageScanBudgets,
    /// How long a finished scan is kept (`STORAGE_SCAN_TTL`; tests shorten it).
    pub result_ttl: Duration,
}

impl StorageScanSettings {
    /// Settings that never scan.
    #[must_use]
    pub fn not_configured() -> Self {
        Self {
            roots: None,
            log_retention_days: crate::config::DEFAULT_STORAGE_LOG_RETENTION_DAYS,
            workspace_markers: Vec::new(),
            excluded_paths: Vec::new(),
            current_exe: None,
            proc_root: PathBuf::from("/proc"),
            git_binary: OsString::from("git"),
            git_env: Vec::new(),
            git_ceilings: Vec::new(),
            budgets: StorageScanBudgets::default(),
            result_ttl: STORAGE_SCAN_TTL,
        }
    }

    /// Settings for the running server.
    #[must_use]
    pub fn from_config(config: &ServerConfig, runtime_dir: Option<&Path>) -> Self {
        let mut excluded_paths = vec![
            config.database_path.clone(),
            sibling_with_suffix(&config.database_path, "-wal"),
            sibling_with_suffix(&config.database_path, "-shm"),
            config.artifact_path.clone(),
            config.coordination_path.clone(),
            config.knowledge_path.clone(),
        ];
        excluded_paths.extend(runtime_dir.map(Path::to_path_buf));
        Self {
            roots: config.storage.roots.clone(),
            log_retention_days: config.storage.log_retention_days,
            workspace_markers: config.storage.workspace_markers.clone(),
            excluded_paths,
            current_exe: std::env::current_exe().ok(),
            git_ceilings: git_ceilings_from_env(),
            ..Self::not_configured()
        }
    }
}

/// The absolute entries of `GIT_CEILING_DIRECTORIES`, which Git would
/// honour for a plain `git` call (relative entries are ignored, as Git does),
/// with symlinks resolved as Git resolves them.
fn git_ceilings_from_env() -> Vec<PathBuf> {
    std::env::var_os("GIT_CEILING_DIRECTORIES")
        .map(|value| {
            let absolute: Vec<PathBuf> = std::env::split_paths(&value)
                .filter(|path| path.is_absolute())
                .collect();
            git_probe::canonical_ceilings(&absolute)
        })
        .unwrap_or_default()
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Result of `start_or_join`.
#[derive(Debug, Clone)]
pub struct StorageScanStart {
    pub scan: StorageScan,
    /// False when the request joined a scan that was already running.
    pub started: bool,
}

#[derive(Clone)]
pub struct StorageScanService {
    inner: Arc<Inner>,
}

struct Inner {
    settings: StorageScanSettings,
    store: Arc<dyn YardStore>,
    source: Arc<dyn InventorySource>,
    reconciliation: ReconciliationService,
    /// Server shutdown: a running walk is cancelled so the blocking thread
    /// does not hold the runtime (and the instance lock) open.
    shutdown: Option<watch::Receiver<bool>>,
    current: Mutex<Option<ScanRecord>>,
}

struct ScanRecord {
    scan: StorageScan,
    expires_at: Option<std::time::Instant>,
}

impl StorageScanService {
    #[must_use]
    pub fn new(
        settings: StorageScanSettings,
        store: Arc<dyn YardStore>,
        source: Arc<dyn InventorySource>,
        reconciliation: ReconciliationService,
    ) -> Self {
        Self::with_shutdown(settings, store, source, reconciliation, None)
    }

    /// Like `new`; a scan's walk stops when `shutdown` becomes true.
    #[must_use]
    pub fn with_shutdown(
        settings: StorageScanSettings,
        store: Arc<dyn YardStore>,
        source: Arc<dyn InventorySource>,
        reconciliation: ReconciliationService,
        shutdown: Option<watch::Receiver<bool>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                settings,
                store,
                source,
                reconciliation,
                shutdown,
                current: Mutex::new(None),
            }),
        }
    }

    /// Start a scan, or join the one that is already running. A completed
    /// scan is replaced. Nothing is scanned when storage is not configured.
    #[must_use]
    pub fn start_or_join(&self) -> StorageScanStart {
        let Some(roots) = self.inner.settings.roots.clone() else {
            return StorageScanStart {
                scan: StorageScan::not_configured(),
                started: false,
            };
        };
        let mut current = self.lock();
        if let Some(record) = current.as_ref()
            && record.scan.status == StorageScanStatus::Running
        {
            return StorageScanStart {
                scan: record.scan.clone(),
                started: false,
            };
        }
        let id = uuid::Uuid::now_v7().to_string();
        let scan = StorageScan {
            id: Some(id.clone()),
            status: StorageScanStatus::Running,
            roots: roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            started_at_unix_ms: Some(unix_ms(SystemTime::now())),
            ..StorageScan::not_configured()
        };
        *current = Some(ScanRecord {
            scan: scan.clone(),
            expires_at: None,
        });
        drop(current);
        let service = self.clone();
        tokio::spawn(async move {
            let inner = Arc::clone(&service.inner);
            let outcome = tokio::spawn(async move { execute(&inner, roots).await }).await;
            service.finish(
                &id,
                outcome.unwrap_or_else(|error| {
                    Err(format!("the storage scan stopped unexpectedly: {error}"))
                }),
            );
        });
        StorageScanStart {
            scan,
            started: true,
        }
    }

    /// The scan with `id`, if it is the current one and has not expired.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<StorageScan> {
        let mut current = self.lock();
        if current
            .as_ref()
            .and_then(|record| record.expires_at)
            .is_some_and(|expires_at| std::time::Instant::now() >= expires_at)
        {
            *current = None;
        }
        current
            .as_ref()
            .filter(|record| record.scan.id.as_deref() == Some(id))
            .map(|record| record.scan.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<ScanRecord>> {
        self.inner
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn finish(&self, id: &str, outcome: Result<StorageScan, String>) {
        let mut current = self.lock();
        let Some(record) = current
            .as_mut()
            .filter(|record| record.scan.id.as_deref() == Some(id))
        else {
            return;
        };
        let now = SystemTime::now();
        let ttl = self.inner.settings.result_ttl;
        match outcome {
            Ok(result) => {
                tracing::info!(
                    target: "yard_server",
                    entries = result.entries_scanned,
                    candidates = result.candidates.len(),
                    truncated = result.estimate_truncated,
                    in_use_check = result
                        .in_use_check
                        .as_ref()
                        .is_some_and(|check| check.available),
                    "storage scan completed"
                );
                record.scan = StorageScan {
                    id: record.scan.id.clone(),
                    roots: record.scan.roots.clone(),
                    started_at_unix_ms: record.scan.started_at_unix_ms,
                    ..result
                };
            }
            Err(error) => {
                tracing::warn!(target: "yard_server", %error, "storage scan failed");
                record.scan.status = StorageScanStatus::Failed;
                record.scan.error = Some(error);
            }
        }
        record.scan.completed_at_unix_ms = Some(unix_ms(now));
        record.scan.expires_at_unix_ms = Some(unix_ms(now + ttl));
        record.expires_at = Some(std::time::Instant::now() + ttl);
        drop(current);
        let service = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            let _ = service.get(&id);
        });
    }
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    })
}

/// Canonicalize a path that may not exist yet: the nearest existing
/// ancestor is resolved and the rest is appended. A relative path is first
/// made absolute against the working directory, so it can still match.
fn canonical_or_lexical(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let path = absolute.as_path();
    let mut suffix = Vec::new();
    let mut current = path;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            suffix.push(name.to_owned());
        }
        if let Ok(canonical) = fs::canonicalize(parent) {
            let mut resolved = canonical;
            for name in suffix.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        current = parent;
    }
    path.to_path_buf()
}

/// Paths observed by the in-use probes.
struct Observations {
    in_use_paths: Vec<PathBuf>,
    herdr_error: Option<String>,
    live_recorded: Vec<(String, String, PathBuf)>,
    live_known: Vec<PathBuf>,
}

#[allow(clippy::too_many_lines)]
async fn execute(inner: &Inner, roots: Vec<PathBuf>) -> Result<StorageScan, String> {
    let settings = &inner.settings;
    let budgets = &settings.budgets;
    let current_exe = settings
        .current_exe
        .clone()
        .ok_or("Yard could not resolve its own executable path, so it cannot exclude it")?;
    let started = tokio::time::Instant::now();
    let git_deadline = started + budgets.total_time;
    let walk_deadline = std::time::Instant::now() + budgets.walk_time;

    let mut excluded_raw = settings.excluded_paths.clone();
    excluded_raw.push(current_exe);
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_watch = inner.shutdown.clone().map(|mut shutdown| {
        // Seeded synchronously so a shutdown that already happened is seen.
        cancel.store(*shutdown.borrow(), Ordering::Relaxed);
        let cancel = Arc::clone(&cancel);
        AbortOnDrop(tokio::spawn(async move {
            if shutdown.wait_for(|requested| *requested).await.is_ok() {
                cancel.store(true, Ordering::Relaxed);
            }
        }))
    });
    let walk_cancel = Arc::clone(&cancel);
    let owner_roots = roots.clone();
    let walk_input = (
        roots,
        excluded_raw,
        budgets.clone(),
        settings.workspace_markers.clone(),
    );
    let walk = tokio::task::spawn_blocking(move || {
        let (roots, excluded_raw, budgets, workspace_markers) = walk_input;
        let excluded = excluded_raw
            .iter()
            .map(|path| canonical_or_lexical(path))
            .collect();
        storage_walk::walk(&WalkSettings {
            roots,
            excluded,
            max_entries: budgets.max_entries,
            deadline: walk_deadline,
            max_discovery_depth: budgets.max_discovery_depth,
            max_size_depth: budgets.max_size_depth,
            max_candidates: budgets.max_candidates,
            max_workspace_packages: budgets.max_workspace_packages,
            workspace_markers,
            cancel: walk_cancel,
        })
    })
    .await
    .map_err(|error| format!("the storage walk stopped unexpectedly: {error}"))?;
    if walk.cancelled || cancel.load(Ordering::Relaxed) {
        return Err("the storage scan was cancelled because Yard is stopping".to_owned());
    }

    let mut notes = Vec::new();
    let records = match inner.store.storage_owner_records().await {
        Ok(records) => records,
        Err(error) => {
            notes.push(format!("Recorded owner paths are unavailable: {error}"));
            yard_store::StorageOwnerRecords::default()
        }
    };
    let observations =
        tokio::time::timeout(budgets.observe_time, observe(inner, &records.bindings))
            .await
            .unwrap_or_else(|_| Observations {
                in_use_paths: Vec::new(),
                herdr_error: Some("Herdr observation timed out".to_owned()),
                live_recorded: Vec::new(),
                live_known: Vec::new(),
            });
    let proc_root = settings.proc_root.clone();
    let max_processes = budgets.max_processes;
    let (process_paths, recorded_paths) = {
        let cancel = Arc::clone(&cancel);
        let recorded_raw = records.paths.clone();
        let live_recorded = observations.live_recorded.clone();
        let live_known = observations.live_known.clone();
        let in_use_raw = observations.in_use_paths.clone();
        tokio::task::spawn_blocking(move || {
            // Herdr reports paths as the shell sees them (possibly through
            // symlinked ancestors); the kernel already resolves `/proc` links.
            let mut in_use: BTreeSet<PathBuf> = in_use_raw
                .iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|path| canonical_or_lexical(path))
                .collect();
            let euid = rustix::process::geteuid().as_raw();
            let processes =
                observe_processes(&proc_root, euid, max_processes, &cancel).map(|observed| {
                    in_use.extend(observed.paths);
                    (observed.other_users, observed.not_dumpable)
                });
            let mut recorded = Vec::new();
            for record in recorded_raw {
                let path = canonical_or_lexical(Path::new(&record.path));
                recorded.push(RecordedPath {
                    path,
                    owner: StorageOwner {
                        kind: record.kind,
                        id: record.owner_id,
                        name: record.owner_name,
                    },
                });
            }
            for (id, name, path) in live_recorded {
                recorded.push(RecordedPath {
                    path: canonical_or_lexical(&path),
                    owner: StorageOwner {
                        kind: StorageOwnerKind::Project,
                        id,
                        name,
                    },
                });
            }
            let mut known: BTreeSet<PathBuf> =
                recorded.iter().map(|record| record.path.clone()).collect();
            known.extend(live_known.iter().map(|path| canonical_or_lexical(path)));
            ((processes, in_use), (recorded, known))
        })
        .await
        .map_err(|error| format!("the in-use probe stopped unexpectedly: {error}"))?
    };
    let (process_result, in_use_paths) = process_paths;
    let (recorded, known) = recorded_paths;
    let mut in_use_reasons = Vec::new();
    if let Some(error) = &observations.herdr_error {
        in_use_reasons.push(format!("Herdr sessions unavailable: {error}"));
    }
    let (other_users, not_dumpable) = match process_result {
        Ok(counts) => counts,
        Err(error) => {
            in_use_reasons.push(error);
            (0, Vec::new())
        }
    };
    if other_users > 0 {
        notes.push(format!(
            "{other_users} processes of other users could not be inspected and were ignored"
        ));
    }
    if let Some(note) = not_dumpable_note(&not_dumpable) {
        notes.push(note);
    }
    let in_use_available = in_use_reasons.is_empty();

    let runner = GitRunner::new(
        settings.git_binary.clone(),
        budgets.git_timeout,
        git_deadline,
    )
    .with_env(settings.git_env.clone());
    let mut git_deadline_hit = false;
    let mut walk = walk;
    let mut worktrees = confirm_worktrees(&runner, &mut walk, &mut git_deadline_hit).await;
    for (index, (facts, _)) in &mut worktrees {
        facts.known_to_yard = known.contains(&walk.candidates[*index].path);
    }
    let log_cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            u64::from(settings.log_retention_days) * 24 * 60 * 60,
        ))
        .unwrap_or(UNIX_EPOCH);

    let mut sources = Vec::new();
    for workspace in &walk.workspaces {
        let mut probes = Vec::new();
        for package in &workspace.packages {
            let result = if package.has_git {
                PackageResult::Git(
                    match git_probe::probe_package(&runner, &package.path).await {
                        Ok(facts) => GitCheck::Checked(facts),
                        Err(error) => {
                            git_deadline_hit |= error == GitError::DeadlineExceeded;
                            error.into()
                        }
                    },
                )
            } else {
                PackageResult::NotAGitRepository
            };
            probes.push(PackageProbe {
                name: package.name.clone(),
                path: package.path.clone(),
                result,
            });
        }
        sources.push(source_summary(&probes, workspace.listing_complete));
    }

    let mut repositories = WorkTrees::new(&owner_roots, &walk.nested_git, &settings.git_ceilings);
    let mut generated = Vec::with_capacity(walk.candidates.len());
    for candidate in &walk.candidates {
        let check = match (candidate.repo_scope, candidate.class) {
            (Some(scope), class) if class.is_generated() => {
                match repositories.resolve(&runner, scope).await {
                    Ok(None) => GitCheck::NotApplicable,
                    Ok(Some(work_tree)) => {
                        match git_probe::probe_generated(&runner, &work_tree, &candidate.path).await
                        {
                            Ok(facts) => GitCheck::Checked(facts),
                            Err(error) => {
                                git_deadline_hit |= error == GitError::DeadlineExceeded;
                                error.into()
                            }
                        }
                    }
                    Err(error) => {
                        git_deadline_hit |= error == GitError::DeadlineExceeded;
                        match error {
                            GitError::DubiousOwnership => GitCheck::DubiousOwnership,
                            error => GitCheck::Failed(format!(
                                "the enclosing Git repository could not be determined: {error}"
                            )),
                        }
                    }
                }
            }
            _ => GitCheck::NotApplicable,
        };
        generated.push(check);
    }

    let mut truncation = walk.truncation.clone();
    if git_deadline_hit {
        truncation.insert(StorageTruncationReason::TimeBudget);
    }
    if walk.unreadable > 0 {
        notes.push(format!(
            "{} directories could not be read; affected candidates are truncated",
            walk.unreadable
        ));
    }
    if walk.depth_limited > 0 {
        notes.push(format!(
            "{} directories were deeper than the discovery limit and were not searched",
            walk.depth_limited
        ));
    }
    if walk.undiscovered_count > 0 {
        let named: Vec<String> = walk
            .undiscovered
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let more = walk.undiscovered_count - named.len() as u64;
        notes.push(format!(
            "{} directories were never searched because a scan budget ran out, so candidates below them are missing: {}{}",
            walk.undiscovered_count,
            named.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
    }
    for (root, reason) in &walk.skipped_roots {
        notes.push(format!("Root {} was skipped: {reason}", root.display()));
    }
    for root in &walk.worktree_roots {
        notes.push(format!(
            "Root {} is a linked Git worktree and is not listed as a candidate; configure its parent directory to include it",
            root.display()
        ));
    }

    let ids: Vec<String> = walk
        .candidates
        .iter()
        .map(|_| uuid::Uuid::now_v7().to_string())
        .collect();
    let mut nested = vec![0_u64; walk.candidates.len()];
    for candidate in &walk.candidates {
        let mut parent = candidate.parent;
        while let Some(index) = parent {
            nested[index] = nested[index].saturating_add(candidate.bytes);
            parent = walk.candidates[index].parent;
        }
    }
    let in_use_state = |scope: &Path| {
        if !in_use_available {
            InUse::Unavailable
        } else if is_in_use(scope, &in_use_paths) {
            InUse::Busy
        } else {
            InUse::Free
        }
    };
    let known_to_yard = |path: &Path| known.contains(path);

    let mut candidates = Vec::with_capacity(walk.candidates.len());
    for (index, candidate) in walk.candidates.iter().enumerate() {
        let workspace = candidate
            .marked_workspace
            .map(|workspace| &walk.workspaces[workspace]);
        let source = candidate
            .marked_workspace
            .map(|workspace| &sources[workspace]);
        let scope = workspace.map_or(candidate.path.as_path(), |workspace| &workspace.path);
        let in_use = in_use_state(scope);
        let worktree = worktrees.get(&index);
        let worktree_facts = worktree.map(|(facts, _)| facts);
        let recent_logs = match candidate.class {
            StorageClass::WorkspaceBuild => candidate
                .newest_log_mtime
                .is_some_and(|modified| modified > log_cutoff),
            StorageClass::WorkspaceLogs => candidate
                .newest_mtime
                .is_some_and(|modified| modified > log_cutoff),
            _ => false,
        };
        let uses_source = matches!(
            candidate.class,
            StorageClass::WorkspaceBuild | StorageClass::WorkspaceEnv
        );
        let verdict = classify(&CandidateFacts {
            class: candidate.class,
            complete: candidate.complete,
            contains_git: candidate.contains_git,
            contains_nested_repo: candidate.contains_nested_repo,
            crosses_device: candidate.crosses_device,
            path_verified: candidate.path_verified,
            in_use: &in_use,
            generated_git: &generated[index],
            source: source.filter(|_| uses_source),
            recent_logs,
            worktree: worktree_facts,
            unknown_detail: candidate.unknown_detail,
        });
        candidates.push(StorageCandidate {
            id: ids[index].clone(),
            path: candidate.path.to_string_lossy().into_owned(),
            class: candidate.class,
            bytes: candidate.bytes,
            shared_bytes: candidate.shared_bytes,
            nested_bytes: nested[index],
            file_count: candidate.file_count,
            newest_mtime_unix_ms: candidate.newest_mtime.map(unix_ms),
            owner: resolve_owner(
                &candidate.path,
                workspace.map(|workspace| workspace.path.as_path()),
                &recorded,
                &owner_roots,
            ),
            known_to_yard: (candidate.class == StorageClass::GitWorktree)
                .then(|| known_to_yard(&candidate.path)),
            safety: verdict.safety,
            reasons: verdict.reasons,
            acknowledgement: (candidate.class == StorageClass::WorkspaceEnv)
                .then(|| WORKSPACE_ENV_ACKNOWLEDGEMENT.to_owned()),
            source_state: source.map(|source: &SourceSummary| source.state.clone()),
            estimate_truncated: !candidate.complete,
            workspace_path: workspace
                .map(|workspace| workspace.path.to_string_lossy().into_owned()),
            worktree: worktree.map(|(_, details)| details.clone()),
            parent_id: candidate.parent.map(|parent| ids[parent].clone()),
        });
    }
    candidates.sort_by(|left, right| {
        (right.bytes + right.nested_bytes)
            .cmp(&(left.bytes + left.nested_bytes))
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut totals = StorageTotals::default();
    for candidate in &candidates {
        let total = match candidate.safety {
            StorageSafety::Safe => &mut totals.safe,
            StorageSafety::Review => &mut totals.review,
            StorageSafety::Blocked => &mut totals.blocked,
        };
        add_total(total, candidate);
    }
    let estimate_truncated = !truncation.is_empty()
        || candidates
            .iter()
            .any(|candidate| candidate.estimate_truncated);
    Ok(StorageScan {
        id: None,
        status: StorageScanStatus::Completed,
        roots: Vec::new(),
        started_at_unix_ms: None,
        completed_at_unix_ms: None,
        expires_at_unix_ms: None,
        estimate_truncated,
        truncation: truncation.into_iter().collect(),
        entries_scanned: walk.entries,
        in_use_check: Some(StorageInUseCheck {
            available: in_use_available,
            reason: (!in_use_available).then(|| in_use_reasons.join("; ")),
        }),
        notes,
        totals,
        candidates,
        error: None,
    })
}

fn add_total(total: &mut StorageSafetyTotal, candidate: &StorageCandidate) {
    total.count += 1;
    total.bytes = total.bytes.saturating_add(candidate.bytes);
    total.shared_bytes = total.shared_bytes.saturating_add(candidate.shared_bytes);
}

/// Live Herdr observations: every pane, worker, and workspace path is an
/// in-use source; bound project workspaces also supply owner paths.
async fn observe(inner: &Inner, bindings: &[yard_store::StorageWorkspaceBinding]) -> Observations {
    let mut observations = Observations {
        in_use_paths: Vec::new(),
        herdr_error: None,
        live_recorded: Vec::new(),
        live_known: Vec::new(),
    };
    let sessions = match inner.source.sessions().await {
        Ok(sessions) => sessions,
        Err(error) => {
            observations.herdr_error = Some(error.to_string());
            return observations;
        }
    };
    for session in sessions.sessions.iter().filter(|session| session.running) {
        match inner.reconciliation.inventory(&session.name).await {
            Ok((inventory, _)) => collect_inventory(&inventory, bindings, &mut observations),
            Err(error) => {
                observations.herdr_error = Some(format!("session {}: {error}", session.name));
                return observations;
            }
        }
    }
    observations
}

fn collect_inventory(
    inventory: &RuntimeInventory,
    bindings: &[yard_store::StorageWorkspaceBinding],
    observations: &mut Observations,
) {
    let mut push = |path: &Option<String>| {
        if let Some(path) = path.as_deref().filter(|path| path.starts_with('/')) {
            observations.in_use_paths.push(PathBuf::from(path));
        }
    };
    for pane in &inventory.panes {
        push(&pane.cwd);
        push(&pane.foreground_cwd);
    }
    for worker in &inventory.workers {
        push(&worker.cwd);
        push(&worker.foreground_cwd);
    }
    for workspace in &inventory.workspaces {
        if let Some(worktree) = &workspace.worktree
            && worktree.checkout_path.starts_with('/')
        {
            observations
                .in_use_paths
                .push(PathBuf::from(&worktree.checkout_path));
        }
    }
    for binding in bindings.iter().filter(|binding| {
        binding.adapter == inventory.adapter && binding.session == inventory.session
    }) {
        for workspace in inventory
            .workspaces
            .iter()
            .filter(|workspace| workspace.runtime_id == binding.workspace_id)
        {
            if let Some(worktree) = workspace
                .worktree
                .as_ref()
                .filter(|worktree| worktree.checkout_path.starts_with('/'))
            {
                observations.live_recorded.push((
                    binding.project_id.clone(),
                    binding.project_name.clone(),
                    PathBuf::from(&worktree.checkout_path),
                ));
            }
        }
        for pane in inventory
            .panes
            .iter()
            .filter(|pane| pane.workspace_id == binding.workspace_id)
        {
            for path in [&pane.cwd, &pane.foreground_cwd].into_iter().flatten() {
                if path.starts_with('/') {
                    observations.live_known.push(PathBuf::from(path));
                }
            }
        }
    }
}

/// What the process table showed: every readable `cwd`/`exe`, the number of
/// other users' processes that could not be read, and this user's
/// non-dumpable processes (`pid (name)`).
#[derive(Debug)]
struct ProcessObservation {
    paths: Vec<PathBuf>,
    other_users: u64,
    not_dumpable: Vec<String>,
}

/// Read `cwd` and `exe` of every process in the process table (as user
/// `euid`). Other users' unreadable processes are counted and ignored.
/// A process of this user whose unreadable link is owned by another user
/// (root) is not dumpable (e.g. the `sshd: user@pts` session process after
/// its privilege drop): the kernel hides its links from its own user too,
/// so it is listed and ignored. Any other unreadable process of this user
/// makes the check unavailable (fail closed).
fn observe_processes(
    proc_root: &Path,
    euid: u32,
    max_processes: usize,
    cancel: &AtomicBool,
) -> Result<ProcessObservation, String> {
    let entries = fs::read_dir(proc_root).map_err(|error| {
        format!(
            "process table unavailable ({}: {error})",
            proc_root.display()
        )
    })?;
    let mut observed = ProcessObservation {
        paths: Vec::new(),
        other_users: 0,
        not_dumpable: Vec::new(),
    };
    let mut processes = 0_usize;
    for entry in entries {
        let entry = entry.map_err(|error| format!("process table unreadable: {error}"))?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_none_or(|name| !name.bytes().all(|byte| byte.is_ascii_digit()))
        {
            continue;
        }
        processes += 1;
        if processes > max_processes {
            return Err(format!("more than {max_processes} processes"));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("the process probe was cancelled because Yard is stopping".to_owned());
        }
        let process = entry.path();
        let mut unreadable = Vec::new();
        for link in ["cwd", "exe"] {
            match fs::read_link(process.join(link)) {
                Ok(path) => {
                    use std::os::unix::ffi::OsStrExt;

                    // An unlinked cwd or executable reads as "<path> (deleted)".
                    let bytes = path.as_os_str().as_bytes();
                    observed
                        .paths
                        .push(bytes.strip_suffix(b" (deleted)").map_or_else(
                            || path.clone(),
                            |stripped| PathBuf::from(std::ffi::OsStr::from_bytes(stripped)),
                        ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => unreadable.push(link),
            }
        }
        if unreadable.is_empty() {
            continue;
        }
        let this_user = || {
            format!(
                "process {} of this user could not be inspected",
                name.to_string_lossy()
            )
        };
        // `/proc/<pid>` is owned by root when the process is not dumpable,
        // so the owner alone would miss this user's process.
        let status = fs::read_to_string(process.join("status")).ok();
        if status_uids_include(status.as_deref(), euid) {
            // Every unreadable link must be owned by someone else (root);
            // a link of this user that cannot be read is unexplained.
            let hidden_by_kernel = unreadable.iter().all(|link| {
                fs::symlink_metadata(process.join(link))
                    .is_ok_and(|metadata| metadata.uid() != euid)
            });
            if !hidden_by_kernel {
                return Err(this_user());
            }
            let command = status
                .as_deref()
                .and_then(|status| status.lines().find_map(|line| line.strip_prefix("Name:")))
                .map_or("", str::trim);
            observed
                .not_dumpable
                .push(format!("{} ({command})", name.to_string_lossy()));
            continue;
        }
        match fs::symlink_metadata(&process) {
            Ok(metadata) if metadata.uid() == euid => return Err(this_user()),
            Ok(_) => observed.other_users += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("process table unreadable: {error}")),
        }
    }
    Ok(observed)
}

/// The note for this user's non-dumpable processes (first 10 named).
fn not_dumpable_note(processes: &[String]) -> Option<String> {
    const SHOWN: usize = 10;
    if processes.is_empty() {
        return None;
    }
    let named = processes
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let more = processes
        .len()
        .checked_sub(SHOWN)
        .filter(|more| *more > 0)
        .map_or_else(String::new, |more| format!(" and {more} more"));
    Some(format!(
        "{} non-dumpable processes of this user could not be inspected and were ignored \
         (the kernel hides their working directory, e.g. an sshd session): {named}{more}",
        processes.len()
    ))
}

/// Whether any uid (real, effective, saved, filesystem) on the `Uid:` line
/// of a `/proc/<pid>/status` text is `euid`.
fn status_uids_include(status: Option<&str>, euid: u32) -> bool {
    status
        .and_then(|status| status.lines().find_map(|line| line.strip_prefix("Uid:")))
        .is_some_and(|uids| {
            uids.split_whitespace()
                .any(|uid| uid.parse::<u32>().is_ok_and(|uid| uid == euid))
        })
}

/// Confirm provisional linked worktrees against `git worktree list` and
/// probe the confirmed ones. Unlisted ones become `unknown`.
#[allow(clippy::too_many_lines)]
async fn confirm_worktrees(
    runner: &GitRunner,
    walk: &mut WalkOutput,
    deadline_hit: &mut bool,
) -> HashMap<usize, (WorktreeFacts, StorageWorktree)> {
    let mut listed = ListedWorktrees::new();
    let mut failures: BTreeMap<PathBuf, GitError> = BTreeMap::new();
    // Only repositories with a discovered worktree are listed: a listing
    // from any worktree names every worktree of its repository, so plain
    // clones cost no Git time.
    let provisional: Vec<usize> = walk
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.class == StorageClass::GitWorktree)
        .map(|(index, _)| index)
        .collect();
    for &index in &provisional {
        let path = walk.candidates[index].path.clone();
        if listed.contains_key(&path) {
            continue;
        }
        match git_probe::list_worktrees(runner, &path).await {
            Ok(entries) => record_worktrees(entries, &mut listed).await,
            Err(error) => {
                *deadline_hit |= error == GitError::DeadlineExceeded;
                failures.insert(path, error);
            }
        }
    }
    let mut results = HashMap::new();
    for index in provisional {
        let path = walk.candidates[index].path.clone();
        let Some((entry, repository, main)) = listed.get(&path).cloned() else {
            if let Some(error) = failures.remove(&path) {
                let details = StorageWorktree {
                    repository_path: String::new(),
                    branch: None,
                    head: None,
                    upstream: None,
                    locked: false,
                    prunable: false,
                };
                results.insert(
                    index,
                    (
                        WorktreeFacts {
                            known_to_yard: false,
                            main: false,
                            locked: false,
                            prunable: false,
                            state: error.into(),
                        },
                        details,
                    ),
                );
            } else {
                let candidate = &mut walk.candidates[index];
                candidate.class = StorageClass::Unknown;
                candidate.unknown_detail = Some("a linked worktree its repository does not list");
            }
            continue;
        };
        let state = match git_probe::probe_worktree(runner, &path).await {
            Ok((mut facts, ignored)) => {
                facts.ignored_user_files = count_ignored_user_files(walk, index, &ignored);
                GitCheck::Checked(facts)
            }
            Err(error) => {
                *deadline_hit |= error == GitError::DeadlineExceeded;
                error.into()
            }
        };
        let upstream = match &state {
            GitCheck::Checked(facts) if facts.has_upstream => facts.upstream.clone(),
            _ => None,
        };
        let details = StorageWorktree {
            repository_path: repository.to_string_lossy().into_owned(),
            branch: entry.branch.as_deref().map(|branch| {
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned()
            }),
            head: entry.head.clone(),
            upstream,
            locked: entry.locked,
            prunable: entry.prunable,
        };
        results.insert(
            index,
            (
                WorktreeFacts {
                    // Filled in by the caller, which owns the known paths.
                    known_to_yard: false,
                    main,
                    locked: entry.locked,
                    prunable: entry.prunable,
                    state,
                },
                details,
            ),
        );
    }
    results
}

type ListedWorktrees = BTreeMap<PathBuf, (WorktreeEntry, PathBuf, bool)>;

/// Index listed worktrees by canonical path; the first entry is the main one.
/// Canonicalization runs on a blocking thread: a listed path may sit on a
/// slow or stale mount. If it cannot run, nothing is recorded, so the
/// worktrees become `unknown` (never cleanable).
async fn record_worktrees(entries: Vec<WorktreeEntry>, listed: &mut ListedWorktrees) {
    let Ok(resolved) = tokio::task::spawn_blocking(move || {
        entries
            .into_iter()
            .map(|entry| (canonical_or_lexical(&entry.path), entry))
            .collect::<Vec<_>>()
    })
    .await
    else {
        return;
    };
    let main = resolved
        .first()
        .map(|(path, _)| path.clone())
        .unwrap_or_default();
    for (index, (path, entry)) in resolved.into_iter().enumerate() {
        listed.insert(path, (entry, main.clone(), index == 0));
    }
}

/// Resolves a candidate's `RepoScope` to the work tree Git reports, asking
/// Git at most once per root and per nested `.git`.
struct WorkTrees<'a> {
    roots: &'a [PathBuf],
    nested: &'a [NestedGit],
    ceilings: &'a [PathBuf],
    resolved_roots: HashMap<usize, Result<Option<PathBuf>, GitError>>,
    resolved_nested: HashMap<usize, Result<Option<PathBuf>, GitError>>,
}

impl<'a> WorkTrees<'a> {
    fn new(roots: &'a [PathBuf], nested: &'a [NestedGit], ceilings: &'a [PathBuf]) -> Self {
        Self {
            roots,
            nested,
            ceilings,
            resolved_roots: HashMap::new(),
            resolved_nested: HashMap::new(),
        }
    }

    /// `Ok(None)`: no repository encloses the scope.
    async fn resolve(
        &mut self,
        runner: &GitRunner,
        mut scope: RepoScope,
    ) -> Result<Option<PathBuf>, GitError> {
        loop {
            match scope {
                RepoScope::Root(index) => {
                    let Some(root) = self.roots.get(index) else {
                        return Err(GitError::Failed("unknown scan root".to_owned()));
                    };
                    if let Entry::Vacant(entry) = self.resolved_roots.entry(index) {
                        entry.insert(
                            git_probe::discover_work_tree(runner, root, self.ceilings).await,
                        );
                    }
                    return self.resolved_roots[&index].clone();
                }
                RepoScope::Nested(index) => {
                    let Some(nested) = self.nested.get(index) else {
                        return Err(GitError::Failed("unknown nested repository".to_owned()));
                    };
                    if let Entry::Vacant(entry) = self.resolved_nested.entry(index) {
                        entry.insert(git_probe::work_tree_at(runner, &nested.path).await);
                    }
                    match &self.resolved_nested[&index] {
                        // A `.git` Git does not accept is not a repository;
                        // whatever encloses this directory still applies.
                        Ok(None) => scope = nested.enclosing,
                        result => return result.clone(),
                    }
                }
            }
        }
    }
}

/// Aborts the task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `git worktree remove` deletes ignored files, so every ignored path must
/// lie inside a nested generated candidate that is itself removable output.
fn count_ignored_user_files(walk: &WalkOutput, worktree: usize, ignored: &[PathBuf]) -> u64 {
    let generated: Vec<&WalkCandidate> = walk
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.parent == Some(worktree)
                && matches!(
                    candidate.class,
                    StorageClass::RustTarget
                        | StorageClass::NodeModules
                        | StorageClass::ViteDist
                        | StorageClass::GradleOutput
                )
                && candidate.complete
                && !candidate.contains_git
                && !candidate.crosses_device
        })
        .collect();
    let unexplained = ignored
        .iter()
        .filter(|path| {
            !generated
                .iter()
                .any(|candidate| contains(&candidate.path, path))
        })
        .count();
    u64::try_from(unexplained).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;

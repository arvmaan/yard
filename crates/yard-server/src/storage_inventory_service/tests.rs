use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use tempfile::TempDir;
use yard_domain::{
    CanvasPlacement, CoordinationNodeKind, CreateCoordinationNode, FocusObservation,
    ObservedStatus, PaneObservation, RuntimeInventory, RuntimeSession, RuntimeSessions,
    StorageCandidate, StorageClass, StorageSafety, StorageScan, StorageScanStatus,
    StorageSourceStatus, StorageTruncationReason,
};
use yard_herdr::HerdrError;
use yard_store::{SqliteProjectStore, YardStore};

use super::{StorageScanBudgets, StorageScanService, StorageScanSettings};
use crate::{
    inventory_service::{InventoryServiceError, InventorySource},
    reconciliation_service::ReconciliationService,
};

/// Herdr stand-in: one running session whose panes report `cwds`.
struct TestInventory {
    cwds: Vec<PathBuf>,
    available: bool,
    hangs: bool,
}

#[async_trait]
impl InventorySource for TestInventory {
    async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
        if self.hangs {
            std::future::pending::<()>().await;
        }
        if !self.available {
            return Err(InventoryServiceError::Herdr(HerdrError::DiscoveryTimeout));
        }
        Ok(RuntimeSessions {
            adapter: "herdr".to_owned(),
            sessions: vec![RuntimeSession {
                name: "default".to_owned(),
                is_default: true,
                running: true,
            }],
        })
    }

    async fn inventory(&self, session: &str) -> Result<RuntimeInventory, InventoryServiceError> {
        let panes = self
            .cwds
            .iter()
            .enumerate()
            .map(|(index, cwd)| PaneObservation {
                runtime_id: format!("pane-{index}"),
                terminal_id: format!("terminal-{index}"),
                pane_instance_id: None,
                workspace_id: "workspace-1".to_owned(),
                tab_id: "tab-1".to_owned(),
                focused: false,
                cwd: Some(cwd.to_string_lossy().into_owned()),
                foreground_cwd: None,
                label: None,
                provider: None,
                display_provider: None,
                status: ObservedStatus::Idle,
                tokens: BTreeMap::new(),
                provider_session: None,
                revision: 1,
            })
            .collect();
        Ok(RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: session.to_owned(),
            runtime_version: "0.8.0".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 1,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes,
            workers: Vec::new(),
            child_agents: Vec::new(),
        })
    }
}

struct Fixture {
    root: PathBuf,
    proc_root: PathBuf,
    store: Arc<SqliteProjectStore>,
    _temp: TempDir,
    _data: TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let root = base.join("root");
        let proc_root = base.join("proc");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&proc_root).unwrap();
        let data = TempDir::new().unwrap();
        let store = Arc::new(
            SqliteProjectStore::open(data.path().join("yard.sqlite3"))
                .await
                .unwrap(),
        );
        Self {
            root,
            proc_root,
            store,
            _temp: temp,
            _data: data,
        }
    }

    fn base(&self) -> PathBuf {
        self.root.parent().unwrap().to_path_buf()
    }

    fn settings(&self) -> StorageScanSettings {
        StorageScanSettings {
            roots: Some(vec![self.root.clone()]),
            log_retention_days: 14,
            workspace_markers: vec!["workspace.toml".to_owned()],
            excluded_paths: Vec::new(),
            current_exe: Some(PathBuf::from("/nonexistent/yard-storage-test-exe")),
            proc_root: self.proc_root.clone(),
            git_binary: "git".into(),
            git_env: vec![("GIT_CONFIG_GLOBAL".into(), "/dev/null".into())],
            // Nothing above the temporary directory (a stray `.git` in the
            // system temp dir, a real repository) may influence a scan.
            git_ceilings: vec![self.base().parent().unwrap().to_path_buf()],
            budgets: StorageScanBudgets::default(),
            result_ttl: super::STORAGE_SCAN_TTL,
        }
    }

    /// A process of this user whose working directory is `cwd`.
    fn process_at(&self, pid: u32, cwd: &Path) {
        let process = self.proc_root.join(pid.to_string());
        fs::create_dir_all(&process).unwrap();
        symlink(cwd, process.join("cwd")).unwrap();
        symlink("/usr/bin/true", process.join("exe")).unwrap();
    }

    async fn register_checkout(&self, path: &Path) {
        let node_id = uuid::Uuid::now_v7().to_string();
        self.store
            .create_coordination_node(
                &node_id,
                Some(path.to_string_lossy().into_owned()),
                None,
                CreateCoordinationNode {
                    command_id: format!("storage-node-{node_id}"),
                    actor: "local-user".to_owned(),
                    name: "Storage checkout".to_owned(),
                    kind: CoordinationNodeKind::Workstream,
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 200.0,
                        height: 120.0,
                    },
                    attached_project_ids: Vec::new(),
                },
            )
            .await
            .unwrap();
    }

    fn service(&self, settings: StorageScanSettings, cwds: Vec<PathBuf>) -> StorageScanService {
        self.service_with_herdr(
            settings,
            TestInventory {
                cwds,
                available: true,
                hangs: false,
            },
        )
    }

    fn service_with_herdr(
        &self,
        settings: StorageScanSettings,
        herdr: TestInventory,
    ) -> StorageScanService {
        let store: Arc<dyn YardStore> = self.store.clone();
        let source: Arc<dyn InventorySource> = Arc::new(herdr);
        let reconciliation = ReconciliationService::new(Arc::clone(&source), Arc::clone(&store));
        StorageScanService::new(settings, store, source, reconciliation)
    }
}

async fn scan(service: &StorageScanService) -> StorageScan {
    let start = service.start_or_join();
    assert!(start.started);
    let id = start.scan.id.clone().unwrap();
    for _ in 0..1_200 {
        let scan = service.get(&id).expect("the current scan");
        if scan.status != StorageScanStatus::Running {
            assert_eq!(
                scan.status,
                StorageScanStatus::Completed,
                "{:?}",
                scan.error
            );
            return scan;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the storage scan did not finish");
}

fn candidate<'a>(scan: &'a StorageScan, path: &Path) -> &'a StorageCandidate {
    let path = path.to_string_lossy();
    scan.candidates
        .iter()
        .find(|candidate| candidate.path == path)
        .unwrap_or_else(|| panic!("no candidate {path}: {:#?}", scan.candidates))
}

fn has_candidate(scan: &StorageScan, path: &Path) -> bool {
    let path = path.to_string_lossy();
    scan.candidates
        .iter()
        .any(|candidate| candidate.path == path)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Yard Test",
            "-c",
            "user.email=yard@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn age(path: &Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60);
    fs::File::open(path).unwrap().set_modified(when).unwrap();
}

/// A bare `origin` outside the roots and a clone with one pushed commit.
fn repository(base: &Path, origin: &str, clone: &Path, files: &[(&str, &str)]) {
    let origin = base.join(origin);
    git(base, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    fs::create_dir_all(clone).unwrap();
    git(clone, &["init", "-q"]);
    for (path, contents) in files {
        write(&clone.join(path), contents);
    }
    git(clone, &["add", "-A"]);
    git(clone, &["commit", "-q", "-m", "initial"]);
    git(
        clone,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(clone, &["push", "-q", "-u", "origin", "main"]);
}

#[tokio::test]
async fn unset_roots_scan_nothing_and_unknown_ids_are_absent() {
    let fixture = Fixture::new().await;
    let service = fixture.service(StorageScanSettings::not_configured(), Vec::new());
    let start = service.start_or_join();
    assert!(!start.started);
    assert_eq!(start.scan.status, StorageScanStatus::NotConfigured);
    assert_eq!(start.scan.id, None);
    assert!(start.scan.candidates.is_empty());
    assert!(service.get("anything").is_none());
}

#[tokio::test]
async fn a_second_request_joins_the_running_scan() {
    let fixture = Fixture::new().await;
    let service = fixture.service(fixture.settings(), Vec::new());
    let first = service.start_or_join();
    let second = service.start_or_join();
    assert!(first.started);
    assert!(!second.started);
    assert_eq!(first.scan.id, second.scan.id);
    assert!(service.get("other-id").is_none());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn git_worktrees_are_safe_only_when_known_clean_pushed_and_merged() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let repo = fixture.root.join("repo");
    repository(
        &base,
        "origin.git",
        &repo,
        &[
            (".gitignore", "target/\n*.local\n.worktrees/\n"),
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "pub fn app() {}\n"),
        ],
    );
    git(&repo, &["remote", "set-head", "origin", "main"]);
    let worktree = |name: &str| repo.join(".worktrees").join(name);
    let add_pushed = |name: &str| {
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                name,
                worktree(name).to_str().unwrap(),
                "main",
            ],
        );
        git(&worktree(name), &["push", "-q", "-u", "origin", name]);
    };
    for name in [
        "merged",
        "dirty",
        "untracked",
        "ignored",
        "locked",
        "unknown",
        "busy",
    ] {
        add_pushed(name);
    }
    write(&worktree("merged").join("target/debug/app"), "binary");
    write(&worktree("ignored").join("target/debug/app"), "binary");
    write(
        &worktree("dirty").join("src/lib.rs"),
        "pub fn changed() {}\n",
    );
    write(&worktree("untracked").join("notes.md"), "draft\n");
    write(&worktree("ignored").join("draft.local"), "private\n");
    git(
        &repo,
        &["worktree", "lock", worktree("locked").to_str().unwrap()],
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree("detached").to_str().unwrap(),
            "main",
        ],
    );
    add_pushed("feature");
    write(
        &worktree("feature").join("feature.rs"),
        "pub fn feature() {}\n",
    );
    git(&worktree("feature"), &["add", "feature.rs"]);
    git(&worktree("feature"), &["commit", "-q", "-m", "feature"]);
    git(&worktree("feature"), &["push", "-q"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "local",
            worktree("local").to_str().unwrap(),
            "main",
        ],
    );
    write(&worktree("local").join("local.rs"), "pub fn local() {}\n");
    git(&worktree("local"), &["add", "local.rs"]);
    git(&worktree("local"), &["commit", "-q", "-m", "local"]);
    write(&repo.join(".worktrees/stray/notes.txt"), "not a worktree\n");
    for name in [
        "merged",
        "dirty",
        "untracked",
        "ignored",
        "locked",
        "busy",
        "detached",
        "feature",
        "local",
    ] {
        fixture.register_checkout(&worktree(name)).await;
    }
    fixture.process_at(4242, &worktree("busy").join("src"));
    fixture.process_at(4243, &fixture.root);

    let service = fixture.service(fixture.settings(), vec![base.clone(), repo.clone()]);
    let scan = scan(&service).await;

    let merged = candidate(&scan, &worktree("merged"));
    assert_eq!(merged.class, StorageClass::GitWorktree);
    assert_eq!(merged.safety, StorageSafety::Safe, "{:?}", merged.reasons);
    assert_eq!(merged.known_to_yard, Some(true));
    let details = merged.worktree.as_ref().unwrap();
    assert_eq!(details.branch.as_deref(), Some("merged"));
    assert_eq!(details.repository_path, repo.to_string_lossy());
    let nested = candidate(&scan, &worktree("merged").join("target"));
    assert_eq!(nested.class, StorageClass::RustTarget);
    assert_eq!(nested.safety, StorageSafety::Safe, "{:?}", nested.reasons);
    assert_eq!(nested.parent_id.as_deref(), Some(merged.id.as_str()));
    assert!(merged.nested_bytes >= nested.bytes && nested.bytes > 0);

    for (name, reasons) in [
        ("dirty", vec!["Blocked: uncommitted changes"]),
        ("untracked", vec!["Blocked: untracked files (1)"]),
        ("ignored", vec!["Blocked: ignored user files (1)"]),
        ("locked", vec!["Blocked: locked worktree"]),
        ("unknown", vec!["Blocked: not a Yard project checkout"]),
        ("busy", vec!["Blocked: in use"]),
        ("detached", vec!["Blocked: detached HEAD"]),
        ("feature", vec!["Blocked: not merged into origin/main"]),
        (
            "local",
            vec![
                "Blocked: no upstream branch",
                "Blocked: not merged into origin/main",
            ],
        ),
    ] {
        let worktree = candidate(&scan, &worktree(name));
        assert_eq!(worktree.class, StorageClass::GitWorktree, "{name}");
        assert_eq!(worktree.safety, StorageSafety::Blocked, "{name}");
        assert_eq!(worktree.reasons, reasons, "{name}");
    }
    assert_eq!(
        candidate(&scan, &worktree("unknown")).known_to_yard,
        Some(false)
    );
    assert!(
        candidate(&scan, &worktree("locked"))
            .worktree
            .as_ref()
            .unwrap()
            .locked
    );
    let stray = candidate(&scan, &repo.join(".worktrees/stray"));
    assert_eq!(stray.class, StorageClass::Unknown);
    assert_eq!(stray.safety, StorageSafety::Blocked);
    assert!(stray.reasons[0].starts_with("Report only:"));
    assert!(
        !has_candidate(&scan, &repo),
        "the main worktree is never a candidate"
    );
    assert!(scan.in_use_check.as_ref().unwrap().available);
    let sizes: Vec<u64> = scan
        .candidates
        .iter()
        .map(|candidate| candidate.bytes + candidate.nested_bytes)
        .collect();
    assert!(
        sizes.windows(2).all(|pair| pair[0] >= pair[1]),
        "largest first"
    );
    let blocked: u64 = scan
        .candidates
        .iter()
        .filter(|candidate| candidate.safety == StorageSafety::Blocked)
        .map(|candidate| candidate.bytes)
        .sum();
    assert_eq!(scan.totals.blocked.bytes, blocked);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn marked_workspaces_review_unclean_sources_and_recent_logs() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let marked = |name: &str| fixture.root.join(".worktrees").join(name);
    for (name, origin) in [("ws-ahead", "pkg-ahead.git"), ("ws-clean", "pkg-clean.git")] {
        let ws = marked(name);
        write(&ws.join("workspace.toml"), "{}\n");
        let package = ws.join("src/Pkg");
        repository(
            &base,
            origin,
            &package,
            &[(".gitignore", "build\n"), ("Config", "package Pkg\n")],
        );
        write(&ws.join("build/Pkg/out.jar"), "jar");
        symlink(ws.join("build/Pkg"), package.join("build")).unwrap();
        write(&ws.join("env/runtime/lib.so"), "lib");
    }
    let ahead = marked("ws-ahead").join("src/Pkg");
    write(&ahead.join("Local.java"), "class Local {}\n");
    git(&ahead, &["add", "Local.java"]);
    git(&ahead, &["commit", "-q", "-m", "local change"]);
    write(&marked("ws-ahead").join("build/logs/recent.log"), "fresh\n");
    let clean = marked("ws-clean");
    write(&clean.join("build/logs/old.log"), "old\n");
    age(&clean.join("build/logs/old.log"), 30);
    write(&clean.join(".build-logs/old.log"), "old\n");
    age(&clean.join(".build-logs/old.log"), 30);
    age(&clean.join(".build-logs"), 30);
    let busy = marked("ws-busy");
    write(&busy.join("workspace.toml"), "{}\n");
    fs::create_dir_all(busy.join("src/Plain")).unwrap();
    write(&busy.join("build/out.jar"), "jar");
    fixture.process_at(5151, &busy.join("src/Plain"));

    let service = fixture.service(fixture.settings(), Vec::new());
    let scan = scan(&service).await;

    let build = candidate(&scan, &marked("ws-ahead").join("build"));
    assert_eq!(build.class, StorageClass::WorkspaceBuild);
    assert_eq!(build.safety, StorageSafety::Review);
    assert_eq!(
        build.reasons,
        [
            "Review: 1 package has unpushed commits",
            "Review: contains recent build logs"
        ]
    );
    let source = build.source_state.as_ref().unwrap();
    assert_eq!(source.status, StorageSourceStatus::NotClean);
    assert_eq!(source.packages[0].ahead, Some(1));
    let env = candidate(&scan, &marked("ws-ahead").join("env"));
    assert_eq!(env.class, StorageClass::WorkspaceEnv);
    assert_eq!(env.safety, StorageSafety::Review);
    assert_eq!(env.reasons, ["Review: 1 package has unpushed commits"]);
    assert_eq!(
        env.acknowledgement.as_deref(),
        Some("The workspace environment will be regenerated on the next build.")
    );
    assert_eq!(
        env.workspace_path.as_deref(),
        Some(marked("ws-ahead").to_string_lossy().as_ref())
    );

    for path in [
        clean.join("build"),
        clean.join("env"),
        clean.join(".build-logs"),
    ] {
        let clean = candidate(&scan, &path);
        assert_eq!(
            clean.safety,
            StorageSafety::Safe,
            "{path:?}: {:?}",
            clean.reasons
        );
    }
    assert_eq!(
        candidate(&scan, &clean.join(".build-logs")).class,
        StorageClass::WorkspaceLogs
    );
    assert!(
        candidate(&scan, &clean.join("env"))
            .acknowledgement
            .is_some()
    );

    let busy_build = candidate(&scan, &busy.join("build"));
    assert_eq!(busy_build.safety, StorageSafety::Blocked);
    assert_eq!(
        busy_build.reasons,
        [
            "Blocked: in use",
            "Review: 1 package is not a Git repository"
        ]
    );
    assert!(
        scan.candidates
            .iter()
            .all(|candidate| !candidate.path.contains("/src/")),
        "nothing inside a workspace src/ is a candidate"
    );
    for name in ["ws-ahead", "ws-clean", "ws-busy"] {
        assert!(
            !has_candidate(&scan, &marked(name)),
            "never the workspace itself"
        );
    }

    let unmarked = fixture.service(
        StorageScanSettings {
            workspace_markers: Vec::new(),
            ..fixture.settings()
        },
        Vec::new(),
    );
    let unmarked_scan = self::scan(&unmarked).await;
    assert!(
        unmarked_scan
            .candidates
            .iter()
            .all(|candidate| !candidate.class.is_workspace()),
        "no configured marker: no workspace classes"
    );
}

#[tokio::test]
async fn generated_output_in_git_needs_untracked_and_ignored_and_excludes_the_live_binary() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let tracked = fixture.root.join("tracked");
    repository(
        &base,
        "tracked.git",
        &tracked,
        &[(".gitignore", "target/\n"), ("Cargo.toml", "[package]\n")],
    );
    write(&tracked.join("target/keep.txt"), "tracked on purpose\n");
    git(&tracked, &["add", "-f", "target/keep.txt"]);
    git(&tracked, &["commit", "-q", "-m", "track generated file"]);
    let web = fixture.root.join("web");
    repository(
        &base,
        "web.git",
        &web,
        &[
            ("vite.config.ts", "export default {}\n"),
            ("package.json", "{}\n"),
        ],
    );
    write(&web.join("dist/index.html"), "<html></html>\n");
    write(
        &web.join("node_modules/dep/index.js"),
        "module.exports = 1;\n",
    );
    let ignored = fixture.root.join("ignored");
    repository(
        &base,
        "ignored.git",
        &ignored,
        &[(".gitignore", "target/\n"), ("Cargo.toml", "[package]\n")],
    );
    write(&ignored.join("target/debug/app"), "binary");
    let live = fixture.root.join("live");
    write(&live.join("Cargo.toml"), "[package]\n");
    write(&live.join("target/debug/yard"), "the running yard");

    let mut settings = fixture.settings();
    settings.current_exe = Some(live.join("target/debug/yard"));
    let scan = scan(&fixture.service(settings, Vec::new())).await;

    let tracked = candidate(&scan, &tracked.join("target"));
    assert_eq!(tracked.safety, StorageSafety::Review);
    assert_eq!(tracked.reasons, ["Review: contains tracked files"]);
    let dist = candidate(&scan, &web.join("dist"));
    assert_eq!(dist.class, StorageClass::ViteDist);
    assert_eq!(dist.reasons, ["Review: not ignored by Git"]);
    let modules = candidate(&scan, &web.join("node_modules"));
    assert_eq!(
        modules.reasons,
        [
            "Review: not ignored by Git",
            "Review: dependencies must be reinstalled"
        ]
    );
    assert_eq!(
        candidate(&scan, &ignored.join("target")).safety,
        StorageSafety::Safe
    );
    assert!(
        !has_candidate(&scan, &live.join("target")),
        "a candidate containing current_exe is excluded"
    );

    let control = self::scan(&fixture.service(fixture.settings(), Vec::new())).await;
    assert!(has_candidate(&control, &live.join("target")));
}

/// The `.git` entries Git does not accept: an empty directory, a garbage
/// file, and a gitfile that points nowhere.
fn plant_stale_git(dir: &Path, kind: usize) {
    let git = dir.join(".git");
    match kind {
        0 => fs::create_dir_all(git).unwrap(),
        1 => write(&git, "garbage\n"),
        _ => write(&git, "gitdir: /nonexistent/yard-stale-gitdir\n"),
    }
}

#[tokio::test]
async fn a_stale_dot_git_above_or_inside_the_root_never_blocks_generated_output() {
    for kind in 0..3 {
        let fixture = Fixture::new().await;
        // Above the root (but below the ceiling, so Git really looks at it).
        plant_stale_git(&fixture.base(), kind);
        let app = fixture.root.join("app");
        write(&app.join("Cargo.toml"), "[package]\n");
        write(&app.join("target/debug/app"), "binary");
        // Next to generated output inside the walk.
        let web = fixture.root.join("web");
        write(&web.join("package.json"), "{}\n");
        write(
            &web.join("node_modules/dep/index.js"),
            "module.exports = 1;\n",
        );
        plant_stale_git(&web, kind);

        let scan = scan(&fixture.service(fixture.settings(), Vec::new())).await;

        let target = candidate(&scan, &app.join("target"));
        assert_eq!(
            target.safety,
            StorageSafety::Safe,
            "{kind}: {:?}",
            target.reasons
        );
        let modules = candidate(&scan, &web.join("node_modules"));
        assert_eq!(
            modules.reasons,
            ["Review: dependencies must be reinstalled"],
            "{kind}: outside any repository, not blocked"
        );
    }
}

#[tokio::test]
async fn git_finds_the_real_enclosing_repository_past_a_stale_dot_git() {
    for kind in 0..3 {
        let fixture = Fixture::new().await;
        let base = fixture.base();
        let repo = fixture.root.join("repo");
        let scanned = repo.join("stale/scanned");
        repository(
            &base,
            "repo.git",
            &repo,
            &[
                (".gitignore", "/stale/scanned/ignored/target/\n"),
                ("stale/scanned/tracked/Cargo.toml", "[package]\n"),
                ("stale/scanned/tracked/target/keep.txt", "tracked\n"),
                ("stale/scanned/ignored/Cargo.toml", "[package]\n"),
                ("stale/scanned/plain/Cargo.toml", "[package]\n"),
            ],
        );
        write(&scanned.join("ignored/target/debug/app"), "binary");
        write(&scanned.join("plain/target/debug/app"), "binary");
        // Between the root and the repository; the old ancestor climb took it
        // for the work tree.
        plant_stale_git(&repo.join("stale"), kind);
        let mut settings = fixture.settings();
        settings.roots = Some(vec![scanned.clone()]);

        let scan = scan(&fixture.service(settings, Vec::new())).await;

        let tracked = candidate(&scan, &scanned.join("tracked/target"));
        assert_eq!(
            tracked.reasons,
            ["Review: contains tracked files"],
            "{kind}"
        );
        let plain = candidate(&scan, &scanned.join("plain/target"));
        assert_eq!(plain.reasons, ["Review: not ignored by Git"], "{kind}");
        let ignored = candidate(&scan, &scanned.join("ignored/target"));
        assert_eq!(
            ignored.safety,
            StorageSafety::Safe,
            "{kind}: {:?}",
            ignored.reasons
        );
    }
}

#[tokio::test]
async fn repository_discovery_never_enters_a_ceiling_directory() {
    let fixture = Fixture::new().await;
    let outer = fixture.root.join("outer");
    repository(
        &fixture.base(),
        "outer.git",
        &outer,
        &[("README.md", "outer\n")],
    );
    let inner = outer.join("inner");
    write(&inner.join("app/Cargo.toml"), "[package]\n");
    write(&inner.join("app/target/debug/app"), "binary");
    let mut settings = fixture.settings();
    settings.roots = Some(vec![inner.clone()]);

    let found = scan(&fixture.service(settings.clone(), Vec::new())).await;
    assert_eq!(
        candidate(&found, &inner.join("app/target")).reasons,
        ["Review: not ignored by Git"],
        "below the ceiling Git finds the outer repository"
    );

    settings.git_ceilings.push(outer.clone());
    let fenced = scan(&fixture.service(settings, Vec::new())).await;
    let target = candidate(&fenced, &inner.join("app/target"));
    assert_eq!(target.safety, StorageSafety::Safe, "{:?}", target.reasons);
}

#[tokio::test]
async fn a_ceiling_spelled_through_a_symlink_still_fences_discovery() {
    // Garbage, and a gitfile that points nowhere: Git stops at either, so the
    // scan resumes above it and must compare against the canonical ceiling.
    for kind in 1..3 {
        let fixture = Fixture::new().await;
        let outer = fixture.root.join("outer");
        repository(
            &fixture.base(),
            "outer.git",
            &outer,
            &[("README.md", "outer\n")],
        );
        let alias = fixture.base().join("alias");
        symlink(&outer, &alias).unwrap();
        let scanned = outer.join("ceiling/scan");
        write(&scanned.join("app/Cargo.toml"), "[package]\n");
        write(&scanned.join("app/target/debug/app"), "binary");
        // Right below the ceiling, which Git (unlike a lexical comparison)
        // resolves through the symlink.
        plant_stale_git(&scanned, kind);
        let mut settings = fixture.settings();
        settings.roots = Some(vec![scanned.clone()]);
        settings.git_ceilings.push(alias.join("ceiling"));

        let scan = scan(&fixture.service(settings, Vec::new())).await;

        let target = candidate(&scan, &scanned.join("app/target"));
        assert_eq!(
            target.safety,
            StorageSafety::Safe,
            "{kind}: the outer repository is above the ceiling: {:?}",
            target.reasons
        );
    }
}

#[tokio::test]
async fn a_failed_repository_discovery_blocks_with_its_reason() {
    let fixture = Fixture::new().await;
    let app = fixture.root.join("app");
    write(&app.join("Cargo.toml"), "[package]\n");
    write(&app.join("target/debug/app"), "binary");
    let broken = fixture.base().join("broken-git");
    script(
        &broken,
        "#!/bin/sh\necho \"fatal: bad config line 1\" >&2\nexit 128\n",
    );
    let mut settings = fixture.settings();
    settings.git_binary = broken.into_os_string();

    let scan = scan(&fixture.service(settings, Vec::new())).await;

    let target = candidate(&scan, &app.join("target"));
    assert_eq!(target.safety, StorageSafety::Blocked);
    assert_eq!(
        target.reasons,
        [
            "Blocked: Git check failed (the enclosing Git repository could not be determined: \
             fatal: bad config line 1)"
        ]
    );
}

#[tokio::test]
async fn truncated_scans_cap_candidates_at_review() {
    let fixture = Fixture::new().await;
    let app = fixture.root.join("app");
    write(&app.join("Cargo.toml"), "[package]\n");
    for index in 0..40 {
        write(&app.join(format!("target/debug/f{index}")), "x");
    }
    let mut settings = fixture.settings();
    settings.budgets.max_entries = 12;
    let scan = scan(&fixture.service(settings, Vec::new())).await;
    let target = candidate(&scan, &app.join("target"));
    assert!(target.estimate_truncated);
    assert_eq!(target.safety, StorageSafety::Review);
    assert_eq!(target.reasons, ["Review: size estimate truncated"]);
    assert!(scan.estimate_truncated);
    assert_eq!(scan.truncation, [StorageTruncationReason::EntryBudget]);

    let complete = self::scan(&fixture.service(fixture.settings(), Vec::new())).await;
    assert_eq!(
        candidate(&complete, &app.join("target")).safety,
        StorageSafety::Safe
    );
}

#[tokio::test]
async fn unavailable_in_use_probes_block_every_candidate() {
    let fixture = Fixture::new().await;
    let app = fixture.root.join("app");
    write(&app.join("Cargo.toml"), "[package]\n");
    write(&app.join("target/debug/app"), "binary");

    let mut no_proc = fixture.settings();
    no_proc.proc_root = fixture.base().join("missing-proc");
    let scan = scan(&fixture.service(no_proc, Vec::new())).await;
    let target = candidate(&scan, &app.join("target"));
    assert_eq!(target.safety, StorageSafety::Blocked);
    assert_eq!(target.reasons, ["Blocked: in-use check unavailable"]);
    let check = scan.in_use_check.as_ref().unwrap();
    assert!(!check.available);
    assert!(
        check
            .reason
            .as_deref()
            .unwrap()
            .contains("process table unavailable")
    );

    let without_herdr = self::scan(&fixture.service_with_herdr(
        fixture.settings(),
        TestInventory {
            cwds: Vec::new(),
            available: false,
            hangs: false,
        },
    ))
    .await;
    assert_eq!(
        candidate(&without_herdr, &app.join("target")).reasons,
        ["Blocked: in-use check unavailable"]
    );

    let mut impatient = fixture.settings();
    impatient.budgets.observe_time = Duration::from_millis(200);
    let hung_herdr = self::scan(&fixture.service_with_herdr(
        impatient,
        TestInventory {
            cwds: Vec::new(),
            available: true,
            hangs: true,
        },
    ))
    .await;
    assert_eq!(
        candidate(&hung_herdr, &app.join("target")).reasons,
        ["Blocked: in-use check unavailable"]
    );
    assert!(
        hung_herdr
            .in_use_check
            .as_ref()
            .and_then(|check| check.reason.as_deref())
            .is_some_and(|reason| reason.contains("timed out"))
    );

    let herdr_pane = self::scan(&fixture.service(
        fixture.settings(),
        vec![app.join("target/debug"), app.clone()],
    ))
    .await;
    assert_eq!(
        candidate(&herdr_pane, &app.join("target")).reasons,
        ["Blocked: in use"]
    );
    let ancestor_only =
        self::scan(&fixture.service(fixture.settings(), vec![app.clone(), fixture.root.clone()]))
            .await;
    assert_eq!(
        candidate(&ancestor_only, &app.join("target")).safety,
        StorageSafety::Safe
    );
}

#[test]
fn relative_missing_paths_are_made_absolute_before_matching() {
    let resolved = super::canonical_or_lexical(Path::new("yard-storage-missing-dir/artifacts"));
    assert!(resolved.is_absolute(), "{}", resolved.display());
    assert!(resolved.ends_with("yard-storage-missing-dir/artifacts"));
    let cwd = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    assert_eq!(resolved, cwd.join("yard-storage-missing-dir/artifacts"));
}

/// A known worktree on a branch that is pushed and already on `origin/main`
/// (safe unless a test changes it).
async fn pushed_merged_worktree(fixture: &Fixture, name: &str) -> (PathBuf, PathBuf) {
    let base = fixture.base();
    let repo = fixture.root.join(format!("repo-{name}"));
    repository(
        &base,
        &format!("origin-{name}.git"),
        &repo,
        &[
            (".gitignore", "target/\n.worktrees/\n"),
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("config.local", "original\n"),
            ("src/lib.rs", "pub fn app() {}\n"),
        ],
    );
    git(&repo, &["remote", "set-head", "origin", "main"]);
    let worktree = repo.join(".worktrees").join(name);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            name,
            worktree.to_str().unwrap(),
            "main",
        ],
    );
    git(&worktree, &["push", "-q", "-u", "origin", name]);
    fixture.register_checkout(&worktree).await;
    (repo, worktree)
}

#[tokio::test]
async fn worktrees_with_hidden_index_edits_or_nested_repositories_are_blocked() {
    let fixture = Fixture::new().await;
    let (_, control) = pushed_merged_worktree(&fixture, "control").await;
    let (_, skip) = pushed_merged_worktree(&fixture, "skip").await;
    git(&skip, &["update-index", "--skip-worktree", "config.local"]);
    write(&skip.join("config.local"), "local edits\n");
    let (_, assumed) = pushed_merged_worktree(&fixture, "assumed").await;
    git(
        &assumed,
        &["update-index", "--assume-unchanged", "src/lib.rs"],
    );
    write(&assumed.join("src/lib.rs"), "pub fn unsaved() {}\n");
    let (repo, embedded) = pushed_merged_worktree(&fixture, "embedded").await;
    let nested = embedded.join("vendor/lib");
    fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    write(&nested.join("only-copy.rs"), "// never pushed\n");
    git(&nested, &["add", "-A"]);
    git(&nested, &["commit", "-q", "-m", "local only"]);
    git(&embedded, &["add", "vendor/lib"]);
    git(&embedded, &["commit", "-q", "-m", "gitlink"]);
    git(&embedded, &["push", "-q"]);
    git(&repo, &["merge", "-q", "--ff-only", "embedded"]);
    git(&repo, &["push", "-q", "origin", "main"]);

    let service = fixture.service(fixture.settings(), Vec::new());
    let scan = scan(&service).await;

    let control = candidate(&scan, &control);
    assert_eq!(control.safety, StorageSafety::Safe, "{:?}", control.reasons);
    for (path, count) in [(&skip, 1), (&assumed, 1)] {
        let hidden = candidate(&scan, path);
        assert_eq!(hidden.safety, StorageSafety::Blocked);
        assert_eq!(
            hidden.reasons,
            [format!(
                "Blocked: files hidden from git status ({count}, skip-worktree/assume-unchanged)"
            )]
        );
    }
    let embedded = candidate(&scan, &embedded);
    assert_eq!(embedded.class, StorageClass::GitWorktree);
    assert_eq!(embedded.safety, StorageSafety::Blocked);
    assert_eq!(
        embedded.reasons,
        ["Blocked: contains another Git repository or submodule"]
    );
}

#[tokio::test]
async fn a_root_that_is_a_linked_worktree_is_never_a_candidate() {
    let fixture = Fixture::new().await;
    let (_, worktree) = pushed_merged_worktree(&fixture, "rooted").await;
    let mut settings = fixture.settings();
    settings.roots = Some(vec![worktree.clone()]);
    let service = fixture.service(settings, Vec::new());
    let scan = scan(&service).await;
    assert!(!has_candidate(&scan, &worktree), "{:#?}", scan.candidates);
    assert!(
        scan.notes
            .iter()
            .any(|note| note.contains("is a linked Git worktree")),
        "{:?}",
        scan.notes
    );
}

#[tokio::test]
async fn git_checks_use_symlinked_dot_git_and_never_an_enclosing_repository() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let project = fixture.root.join("proj");
    repository(
        &base,
        "origin-sym.git",
        &project,
        &[
            ("Cargo.toml", "[package]\n"),
            ("target/tracked.txt", "tracked\n"),
        ],
    );
    let store = base.join("proj.git");
    fs::rename(project.join(".git"), &store).unwrap();
    symlink(&store, project.join(".git")).unwrap();

    let outer = fixture.root.join("outer");
    repository(
        &base,
        "origin-outer.git",
        &outer,
        &[(".gitignore", "ws/\n"), ("README", "x\n")],
    );
    let ws = outer.join("ws");
    write(&ws.join("workspace.toml"), "{}\n");
    fs::create_dir_all(ws.join("src/Pkg/.git")).unwrap();
    write(&ws.join("src/Pkg/Unversioned.java"), "class U {}\n");
    write(&ws.join("build/out.jar"), "jar");

    let service = fixture.service(fixture.settings(), Vec::new());
    let scan = scan(&service).await;

    let target = candidate(&scan, &project.join("target"));
    assert_eq!(target.safety, StorageSafety::Review);
    assert_eq!(target.reasons, ["Review: contains tracked files"]);
    let build = candidate(&scan, &ws.join("build"));
    assert_ne!(build.safety, StorageSafety::Safe, "{:?}", build.reasons);
    assert_ne!(
        build.source_state.as_ref().unwrap().status,
        StorageSourceStatus::Clean
    );
}

#[tokio::test]
async fn package_submodules_marked_ignore_all_still_report_changes() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let library = base.join("library");
    repository(&base, "origin-library.git", &library, &[("lib.rs", "x\n")]);
    let ws = fixture.root.join("ws");
    write(&ws.join("workspace.toml"), "{}\n");
    let package = ws.join("src/Pkg");
    repository(
        &base,
        "origin-pkg.git",
        &package,
        &[("Config", "package Pkg\n")],
    );
    let library_origin = base.join("origin-library.git");
    git(
        &package,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            library_origin.to_str().unwrap(),
            "sub",
        ],
    );
    git(
        &package,
        &["config", "-f", ".gitmodules", "submodule.sub.ignore", "all"],
    );
    git(&package, &["add", "-A"]);
    git(&package, &["commit", "-q", "-m", "submodule"]);
    git(&package, &["push", "-q"]);
    write(&package.join("sub/lib.rs"), "dirty\n");
    write(&ws.join("build/out.jar"), "jar");

    let service = fixture.service(fixture.settings(), Vec::new());
    let scan = scan(&service).await;

    let build = candidate(&scan, &ws.join("build"));
    let source = build.source_state.as_ref().unwrap();
    assert_eq!(source.status, StorageSourceStatus::NotClean, "{source:?}");
    assert_eq!(build.safety, StorageSafety::Review);
    assert!(
        build
            .reasons
            .contains(&"Review: 1 package has uncommitted changes".to_owned()),
        "{:?}",
        build.reasons
    );
}

#[test]
fn non_dumpable_processes_of_this_user_are_ignored_but_other_unreadable_ones_fail_closed() {
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::AtomicBool;

    let base = TempDir::new().unwrap();
    let owner = fs::symlink_metadata(base.path()).unwrap().uid();
    // The fake links are owned by `owner`; observing as `user` makes them
    // "owned by someone else" exactly like a non-dumpable process's
    // root-owned `/proc/<pid>/cwd`.
    let user = owner.wrapping_add(1);
    let proc_root = base.path().join("proc");
    let status = |uid: u32| format!("Name:\tsshd\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
    let sshd = proc_root.join("16662");
    fs::create_dir_all(&sshd).unwrap();
    write(&sshd.join("status"), &status(user));
    write(&sshd.join("cwd"), "unreadable link");
    write(&sshd.join("exe"), "unreadable link");
    let shell = proc_root.join("16700");
    fs::create_dir_all(&shell).unwrap();
    write(&shell.join("status"), &status(user));
    symlink("/work/app", shell.join("cwd")).unwrap();
    let cancel = AtomicBool::new(false);

    let observed = super::observe_processes(&proc_root, user, 100, &cancel).unwrap();
    assert_eq!(observed.paths, [PathBuf::from("/work/app")]);
    assert_eq!(observed.not_dumpable, ["16662 (sshd)"]);
    assert_eq!(observed.other_users, 0);
    let note = super::not_dumpable_note(&observed.not_dumpable).unwrap();
    assert!(
        note.contains("1 non-dumpable processes") && note.ends_with(": 16662 (sshd)"),
        "{note}"
    );
    assert_eq!(super::not_dumpable_note(&[]), None);
    let many: Vec<String> = (0..12).map(|pid| format!("{pid} (sshd)")).collect();
    assert!(
        super::not_dumpable_note(&many)
            .unwrap()
            .ends_with("9 (sshd) and 2 more")
    );

    // The same unreadable process whose links belong to this user is
    // unexplained: fail closed.
    fs::write(sshd.join("status"), status(owner)).unwrap();
    let error = super::observe_processes(&proc_root, owner, 100, &cancel).unwrap_err();
    assert!(error.contains("process 16662 of this user"), "{error}");
}

#[test]
fn the_default_entry_budget_outlasts_the_walk_time_budget() {
    // the dev host walks about 180k entries per second: 3M entries ran out
    // after 17 s and hid 65% of the space, long before the 120 s budget.
    let budgets = StorageScanBudgets::default();
    let entries_per_second = 180_000;
    assert!(
        budgets.max_entries >= entries_per_second * budgets.walk_time.as_secs(),
        "{budgets:?}"
    );
}

#[test]
fn a_non_dumpable_process_of_this_user_is_recognized_from_its_status() {
    let status = "Name:\tsetgid-tool\nUid:\t501\t501\t501\t501\nGid:\t20\t0\t0\t0\n";
    assert!(super::status_uids_include(Some(status), 501));
    assert!(!super::status_uids_include(Some(status), 502));
    assert!(!super::status_uids_include(Some("Name:\tx\n"), 501));
    assert!(!super::status_uids_include(None, 501));
}

#[tokio::test]
async fn worktree_listing_runs_only_for_repositories_with_a_discovered_worktree() {
    let fixture = Fixture::new().await;
    let (_, worktree) = pushed_merged_worktree(&fixture, "listed").await;
    let plain = fixture.root.join("plain-clone");
    repository(
        &fixture.base(),
        "origin-plain.git",
        &plain,
        &[("README.md", "plain\n")],
    );
    let log = fixture.base().join("git-calls.log");
    let wrapper = fixture.base().join("git-wrapper");
    write(
        &wrapper,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec git \"$@\"\n",
            log.display()
        ),
    );
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut settings = fixture.settings();
    settings.git_binary = wrapper.into_os_string();
    let scan = scan(&fixture.service(settings, Vec::new())).await;
    assert_eq!(
        candidate(&scan, &worktree).safety,
        StorageSafety::Safe,
        "{:#?}",
        candidate(&scan, &worktree)
    );
    let calls = fs::read_to_string(&log).unwrap();
    let listings: Vec<&str> = calls
        .lines()
        .filter(|line| line.contains("worktree list"))
        .collect();
    assert_eq!(listings.len(), 1, "{calls}");
    assert!(
        !calls
            .lines()
            .any(|line| line.contains(plain.to_str().unwrap())),
        "the plain clone costs no Git time: {calls}"
    );
}

#[tokio::test]
async fn shutdown_cancels_the_walk_and_fails_the_scan() {
    let fixture = Fixture::new().await;
    write(&fixture.root.join("app/Cargo.toml"), "[package]\n");
    write(&fixture.root.join("app/target/debug/app"), "x");
    let (_shutdown, receiver) = tokio::sync::watch::channel(true);
    let store: Arc<dyn YardStore> = fixture.store.clone();
    let source: Arc<dyn InventorySource> = Arc::new(TestInventory {
        cwds: Vec::new(),
        available: true,
        hangs: false,
    });
    let reconciliation = ReconciliationService::new(Arc::clone(&source), Arc::clone(&store));
    let service = StorageScanService::with_shutdown(
        fixture.settings(),
        store,
        source,
        reconciliation,
        Some(receiver),
    );
    let id = service.start_or_join().scan.id.unwrap();
    let mut scan = None;
    for _ in 0..400 {
        let current = service.get(&id).unwrap();
        if current.status != StorageScanStatus::Running {
            scan = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let scan = scan.expect("the scan finished");
    assert_eq!(scan.status, StorageScanStatus::Failed);
    assert!(
        scan.error
            .as_deref()
            .is_some_and(|error| error.contains("Yard is stopping")),
        "{:?}",
        scan.error
    );
    assert!(scan.candidates.is_empty());
}

/// An executable script at `path`.
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    write(path, body);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn a_copied_worktree_its_repository_does_not_list_is_report_only() {
    let fixture = Fixture::new().await;
    let (_, original) = pushed_merged_worktree(&fixture, "original").await;
    let copy = fixture.root.join("copy-of-wt");
    let status = Command::new("cp")
        .arg("-R")
        .arg(&original)
        .arg(&copy)
        .status()
        .unwrap();
    assert!(status.success());
    write(&copy.join("precious.txt"), "uncommitted work\n");
    fixture.register_checkout(&copy).await;

    let scan = scan(&fixture.service(fixture.settings(), Vec::new())).await;

    assert_eq!(
        candidate(&scan, &original).safety,
        StorageSafety::Safe,
        "control: the registered original is listed"
    );
    let copied = candidate(&scan, &copy);
    assert_eq!(copied.class, StorageClass::Unknown);
    assert_eq!(copied.safety, StorageSafety::Blocked);
    assert_eq!(
        copied.reasons,
        ["Report only: a linked worktree its repository does not list"]
    );
}

#[tokio::test]
async fn a_mainline_clone_without_origin_head_has_a_known_default_branch() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let origin = base.join("origin-mainline.git");
    git(&base, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    let repo = fixture.root.join("Pkg");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "mainline"]);
    write(&repo.join(".gitignore"), ".worktrees/\n");
    write(&repo.join("Config"), "package Pkg\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "mainline"]);
    let worktree = repo.join(".worktrees/feature");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            worktree.to_str().unwrap(),
            "mainline",
        ],
    );
    git(&worktree, &["push", "-q", "-u", "origin", "feature"]);
    fixture.register_checkout(&worktree).await;

    let merged = scan(&fixture.service(fixture.settings(), Vec::new())).await;
    let found = candidate(&merged, &worktree);
    assert_eq!(found.safety, StorageSafety::Safe, "{:?}", found.reasons);

    // Two conventional names and no `origin/HEAD`: which one is the
    // default is unknown, so the worktree stays blocked.
    git(&repo, &["push", "-q", "origin", "mainline:master"]);
    let ambiguous = scan(&fixture.service(fixture.settings(), Vec::new())).await;
    assert_eq!(
        candidate(&ambiguous, &worktree).reasons,
        ["Blocked: default branch unknown"]
    );
}

#[tokio::test]
async fn a_worktree_reports_its_real_upstream_even_under_another_name() {
    let fixture = Fixture::new().await;
    let (_, same) = pushed_merged_worktree(&fixture, "same").await;
    let (_, renamed) = pushed_merged_worktree(&fixture, "renamed").await;
    git(
        &renamed,
        &["push", "-q", "origin", "renamed:tracked-elsewhere"],
    );
    git(
        &renamed,
        &["branch", "-q", "--set-upstream-to=origin/tracked-elsewhere"],
    );

    let scan = scan(&fixture.service(fixture.settings(), Vec::new())).await;

    for (path, upstream) in [
        (&same, "origin/same"),
        (&renamed, "origin/tracked-elsewhere"),
    ] {
        let found = candidate(&scan, path);
        assert_eq!(found.safety, StorageSafety::Safe, "{:?}", found.reasons);
        let details = found.worktree.as_ref().unwrap();
        assert_eq!(details.upstream.as_deref(), Some(upstream), "{path:?}");
    }
}

#[tokio::test]
async fn worktree_push_state_blocks_unpushed_gone_and_local_upstreams() {
    let fixture = Fixture::new().await;
    let (_, ahead) = pushed_merged_worktree(&fixture, "ahead").await;
    write(&ahead.join("more.rs"), "pub fn more() {}\n");
    git(&ahead, &["add", "more.rs"]);
    git(&ahead, &["commit", "-q", "-m", "not pushed"]);
    let (_, gone) = pushed_merged_worktree(&fixture, "gone").await;
    git(&gone, &["push", "-q", "origin", "--delete", "gone"]);
    let (_, local) = pushed_merged_worktree(&fixture, "local").await;
    git(&local, &["branch", "-q", "--set-upstream-to=main"]);

    let scan = scan(&fixture.service(fixture.settings(), Vec::new())).await;

    for (path, reasons) in [
        (
            &ahead,
            vec![
                "Blocked: unpushed commits",
                "Blocked: not merged into origin/main",
            ],
        ),
        (&gone, vec!["Blocked: no upstream branch"]),
        (&local, vec!["Blocked: upstream is a local branch"]),
    ] {
        let worktree = candidate(&scan, path);
        assert_eq!(worktree.class, StorageClass::GitWorktree, "{path:?}");
        assert_eq!(worktree.safety, StorageSafety::Blocked, "{path:?}");
        assert_eq!(worktree.reasons, reasons, "{path:?}");
    }
}

#[tokio::test]
async fn workspace_packages_with_edits_untracked_files_or_stashes_send_build_and_env_to_review() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let workspace = |name: &str| {
        let ws = fixture.root.join(name);
        write(&ws.join("workspace.toml"), "{}\n");
        let package = ws.join("src/Pkg");
        repository(
            &base,
            &format!("origin-{name}.git"),
            &package,
            &[("Config", "package Pkg\n")],
        );
        write(&ws.join("build/out.jar"), "jar");
        write(&ws.join("env/runtime/lib.so"), "lib");
        (ws, package)
    };
    let (clean, _) = workspace("ws-clean");
    let (dirty, package) = workspace("ws-dirty");
    write(&package.join("Config"), "package Pkg\nedited\n");
    let (untracked, package) = workspace("ws-untracked");
    write(&package.join("New.java"), "class New {}\n");
    let (stashed, package) = workspace("ws-stash");
    write(&package.join("Config"), "package Pkg\nstashed\n");
    git(&package, &["stash", "-q"]);

    let scan = scan(&fixture.service(fixture.settings(), Vec::new())).await;

    for dir in ["build", "env"] {
        let control = candidate(&scan, &clean.join(dir));
        assert_eq!(control.safety, StorageSafety::Safe, "{:?}", control.reasons);
        for (ws, reason) in [
            (&dirty, "Review: 1 package has uncommitted changes"),
            (&untracked, "Review: 1 package has untracked files"),
            (&stashed, "Review: 1 package has stashed changes"),
        ] {
            let found = candidate(&scan, &ws.join(dir));
            assert_eq!(found.safety, StorageSafety::Review, "{ws:?}/{dir}");
            assert_eq!(found.reasons, [reason], "{ws:?}/{dir}");
            assert_eq!(
                found.source_state.as_ref().unwrap().status,
                StorageSourceStatus::NotClean
            );
        }
    }
}

#[tokio::test]
async fn a_package_refused_for_dubious_ownership_blocks_build_and_env() {
    let fixture = Fixture::new().await;
    let ws = fixture.root.join("ws");
    write(&ws.join("workspace.toml"), "{}\n");
    fs::create_dir_all(ws.join("src/Pkg/.git")).unwrap();
    write(&ws.join("build/out.jar"), "jar");
    write(&ws.join("env/runtime/lib.so"), "lib");
    let refusing = fixture.base().join("refusing-git");
    script(
        &refusing,
        "#!/bin/sh\necho \"fatal: detected dubious ownership in repository at /somewhere\" >&2\nexit 128\n",
    );
    let mut settings = fixture.settings();
    settings.git_binary = refusing.into_os_string();

    let scan = scan(&fixture.service(settings, Vec::new())).await;

    for dir in ["build", "env"] {
        let found = candidate(&scan, &ws.join(dir));
        assert_eq!(found.safety, StorageSafety::Blocked, "{:?}", found.reasons);
        assert_eq!(
            found.reasons,
            [
                "Blocked: Git refused a package repository (dubious ownership)",
                "Review: 1 package was refused by Git (dubious ownership)",
            ]
        );
    }
}

#[tokio::test]
async fn process_executables_and_unreadable_processes_of_this_user_count() {
    let fixture = Fixture::new().await;
    let app = fixture.root.join("app");
    write(&app.join("Cargo.toml"), "[package]\n");
    write(&app.join("target/debug/app"), "binary");
    let process = fixture.proc_root.join("7001");
    fs::create_dir_all(&process).unwrap();
    symlink(fixture.base(), process.join("cwd")).unwrap();
    symlink(app.join("target/debug/app"), process.join("exe")).unwrap();

    let scan = scan(&fixture.service(fixture.settings(), Vec::new())).await;
    assert_eq!(
        candidate(&scan, &app.join("target")).reasons,
        ["Blocked: in use"],
        "a process running a binary inside the candidate uses it"
    );

    fs::remove_file(process.join("exe")).unwrap();
    symlink("/usr/bin/true", process.join("exe")).unwrap();
    let free = self::scan(&fixture.service(fixture.settings(), Vec::new())).await;
    assert_eq!(
        candidate(&free, &app.join("target")).safety,
        StorageSafety::Safe,
        "control"
    );

    // `readlink` on a regular file fails (EINVAL), like a non-dumpable process.
    let hidden = fixture.proc_root.join("7002");
    fs::create_dir_all(&hidden).unwrap();
    write(&hidden.join("cwd"), "not a link");
    let unreadable = self::scan(&fixture.service(fixture.settings(), Vec::new())).await;
    assert_eq!(
        candidate(&unreadable, &app.join("target")).reasons,
        ["Blocked: in-use check unavailable"]
    );
    let check = unreadable.in_use_check.as_ref().unwrap();
    assert!(!check.available);
    assert!(
        check
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("process 7002 of this user")),
        "{check:?}"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn herdr_inventory_supplies_in_use_paths_and_owner_paths_only_for_bound_workspaces() {
    use yard_domain::{WorkspaceObservation, WorktreeObservation};

    let workspace = |id: &str, checkout: &str| WorkspaceObservation {
        runtime_id: id.to_owned(),
        order: 0,
        label: id.to_owned(),
        focused: false,
        active_tab_id: "tab-1".to_owned(),
        pane_count: 1,
        tab_count: 1,
        status: ObservedStatus::Idle,
        tokens: BTreeMap::new(),
        worktree: Some(WorktreeObservation {
            repository_key: "repo".to_owned(),
            repository_name: "repo".to_owned(),
            repository_root: "/work/repo".to_owned(),
            checkout_path: checkout.to_owned(),
            is_linked: true,
        }),
    };
    let pane = |id: &str, workspace: &str, cwd: &str, foreground: &str| PaneObservation {
        runtime_id: id.to_owned(),
        terminal_id: format!("terminal-{id}"),
        pane_instance_id: None,
        workspace_id: workspace.to_owned(),
        tab_id: "tab-1".to_owned(),
        focused: false,
        cwd: Some(cwd.to_owned()),
        foreground_cwd: Some(foreground.to_owned()),
        label: None,
        provider: None,
        display_provider: None,
        status: ObservedStatus::Idle,
        tokens: BTreeMap::new(),
        provider_session: None,
        revision: 1,
    };
    let inventory = RuntimeInventory {
        adapter: "herdr".to_owned(),
        session: "default".to_owned(),
        runtime_version: "0.8.0".to_owned(),
        protocol: 19,
        observed_at_unix_ms: 1,
        focus: FocusObservation::default(),
        workspaces: vec![
            workspace("bound", "/work/bound-checkout"),
            workspace("unbound", "/work/unbound-checkout"),
        ],
        tabs: Vec::new(),
        panes: vec![
            pane("p1", "bound", "/work/bound-cwd", "/work/bound-foreground"),
            pane(
                "p2",
                "unbound",
                "/work/unbound-cwd",
                "/work/unbound-foreground",
            ),
        ],
        workers: Vec::new(),
        child_agents: Vec::new(),
    };
    let binding = |session: &str, workspace: &str| yard_store::StorageWorkspaceBinding {
        project_id: format!("project-{workspace}-{session}"),
        project_name: "Project".to_owned(),
        adapter: "herdr".to_owned(),
        session: session.to_owned(),
        workspace_id: workspace.to_owned(),
    };
    let mut observations = super::Observations {
        in_use_paths: Vec::new(),
        herdr_error: None,
        live_recorded: Vec::new(),
        live_known: Vec::new(),
    };
    super::collect_inventory(
        &inventory,
        &[binding("default", "bound"), binding("other", "unbound")],
        &mut observations,
    );

    let in_use: Vec<&str> = observations
        .in_use_paths
        .iter()
        .map(|path| path.to_str().unwrap())
        .collect();
    for path in [
        "/work/bound-cwd",
        "/work/bound-foreground",
        "/work/unbound-cwd",
        "/work/unbound-foreground",
        "/work/bound-checkout",
        "/work/unbound-checkout",
    ] {
        assert!(in_use.contains(&path), "{path} in {in_use:?}");
    }
    assert_eq!(
        observations.live_recorded,
        [(
            "project-bound-default".to_owned(),
            "Project".to_owned(),
            PathBuf::from("/work/bound-checkout"),
        )]
    );
    assert_eq!(
        observations.live_known,
        [
            PathBuf::from("/work/bound-cwd"),
            PathBuf::from("/work/bound-foreground"),
        ]
    );
}

#[test]
fn server_config_excludes_every_yard_data_path_and_the_runtime_dir() {
    let config = crate::config::ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        herdr_binary: "herdr".into(),
        database_path: PathBuf::from("/data/yard.sqlite3"),
        artifact_path: PathBuf::from("/data/artifacts"),
        orchestrator_cwd: PathBuf::from("/work"),
        coordination_path: PathBuf::from("/data/coordination"),
        knowledge_path: PathBuf::from("/data/knowledge"),
        storage: crate::config::StorageConfig::default(),
        slack: crate::config::SlackConfig::Off,
    };
    let settings = StorageScanSettings::from_config(&config, Some(Path::new("/run/yard")));
    assert_eq!(
        settings.excluded_paths,
        [
            "/data/yard.sqlite3",
            "/data/yard.sqlite3-wal",
            "/data/yard.sqlite3-shm",
            "/data/artifacts",
            "/data/coordination",
            "/data/knowledge",
            "/run/yard",
        ]
        .map(PathBuf::from)
    );
    assert!(settings.current_exe.is_some());
    assert_eq!(settings.result_ttl, super::STORAGE_SCAN_TTL);
}

#[tokio::test]
async fn a_finished_scan_expires_after_its_ttl() {
    let fixture = Fixture::new().await;
    write(&fixture.root.join("app/Cargo.toml"), "[package]\n");
    write(&fixture.root.join("app/target/debug/app"), "x");
    let mut settings = fixture.settings();
    settings.result_ttl = Duration::from_secs(2);
    let service = fixture.service(settings, Vec::new());
    let finished = scan(&service).await;
    let id = finished.id.clone().unwrap();
    assert_eq!(
        finished.expires_at_unix_ms.unwrap() - finished.completed_at_unix_ms.unwrap(),
        2_000
    );
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(service.get(&id).is_none(), "an expired scan is gone");
}

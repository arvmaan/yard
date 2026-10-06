//! Pure storage classification rules (design D9, preview only).
//!
//! Nothing here touches the filesystem or runs processes. The walker and the
//! Git probes collect facts; these functions turn them into a class, an
//! owner, and a `safe | review | blocked` verdict with reasons. Every
//! uncertainty moves a candidate toward `blocked`, never toward `safe`.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    ops::Bound,
    path::{Path, PathBuf},
};

use yard_domain::{
    StorageClass, StorageOwner, StoragePackageIssue, StoragePackageState, StorageSafety,
    StorageSourceState, StorageSourceStatus,
};

/// Acknowledgement every workspace `env/` removal will require.
pub(crate) const WORKSPACE_ENV_ACKNOWLEDGEMENT: &str =
    "The workspace environment will be regenerated on the next build.";

/// Name prefix of a PR4 cleanup quarantine directory.
pub(crate) const RECLAIM_PREFIX: &str = ".yard-reclaim-";

/// Sibling markers present in one directory.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SiblingMarkers {
    pub cargo_toml: bool,
    pub package_json: bool,
    pub vite_config: bool,
    pub gradle: bool,
    pub workspace_marker: bool,
    pub src_dir: bool,
}

impl SiblingMarkers {
    /// Record one directory entry. `is_file` / `is_dir` must come from a
    /// no-follow file type, so a symlinked marker never counts.
    /// `workspace_markers` are the configured workspace root file names
    /// (`YARD_STORAGE_WORKSPACE_MARKERS`); none disables the workspace classes.
    pub(crate) fn observe(
        &mut self,
        name: &OsStr,
        is_file: bool,
        is_dir: bool,
        workspace_markers: &[String],
    ) {
        let Some(name) = name.to_str() else {
            return;
        };
        if is_file && workspace_markers.iter().any(|marker| marker == name) {
            self.workspace_marker = true;
        }
        if is_file {
            match name {
                "Cargo.toml" => self.cargo_toml = true,
                "package.json" => self.package_json = true,
                "build.gradle" | "build.gradle.kts" | "settings.gradle" | "settings.gradle.kts" => {
                    self.gradle = true;
                }
                _ => {
                    if let Some(extension) = name.strip_prefix("vite.config.")
                        && matches!(extension, "js" | "mjs" | "cjs" | "ts" | "mts" | "cts")
                    {
                        self.vite_config = true;
                    }
                }
            }
        } else if is_dir && name == "src" {
            self.src_dir = true;
        }
    }

    /// A marked workspace root has a configured marker file and a `src/`
    /// directory.
    pub(crate) const fn is_workspace_root(self) -> bool {
        self.workspace_marker && self.src_dir
    }
}

/// Class of a child directory of a marked workspace root, if any.
pub(crate) fn workspace_child_class(name: &OsStr) -> Option<StorageClass> {
    match name.to_str()? {
        "build" => Some(StorageClass::WorkspaceBuild),
        "env" => Some(StorageClass::WorkspaceEnv),
        ".build" | ".build-logs" => Some(StorageClass::WorkspaceLogs),
        _ => None,
    }
}

/// Class of a generated child directory: exact name plus a sibling marker.
pub(crate) fn generated_class(name: &OsStr, markers: SiblingMarkers) -> Option<StorageClass> {
    match name.to_str()? {
        "target" if markers.cargo_toml => Some(StorageClass::RustTarget),
        "node_modules" if markers.package_json => Some(StorageClass::NodeModules),
        "dist" if markers.vite_config => Some(StorageClass::ViteDist),
        "build" | ".gradle" if markers.gradle => Some(StorageClass::GradleOutput),
        _ => None,
    }
}

/// Whether `inner` is `outer` or lies inside it (component-wise).
pub(crate) fn contains(outer: &Path, inner: &Path) -> bool {
    inner.starts_with(outer)
}

/// In use when an observed path is equal to or inside `scope`. An
/// ancestor-only path (a shell at `$HOME`) never counts. `Path` orders
/// component-wise, so `scope` and everything inside it are contiguous and
/// start at the first observed path not less than `scope`.
pub(crate) fn is_in_use(scope: &Path, observed: &BTreeSet<PathBuf>) -> bool {
    observed
        .range::<Path, _>((Bound::Included(scope), Bound::Unbounded))
        .next()
        .is_some_and(|path| contains(scope, path))
}

/// Result of the in-use probes for one scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InUse {
    Free,
    Busy,
    Unavailable,
}

/// A path Yard recorded for an owner, canonicalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedPath {
    pub path: PathBuf,
    pub owner: StorageOwner,
}

/// The owner is the one with the longest recorded path that contains the
/// candidate. For workspace classes the match is on the workspace root: the
/// workspace must contain a recorded path (typically `<ws>/src/<Pkg>`).
/// Paths equal to or above a storage root are ignored, and a tie between
/// different owners gives no owner.
pub(crate) fn resolve_owner(
    candidate: &Path,
    marked_workspace: Option<&Path>,
    recorded: &[RecordedPath],
    roots: &[PathBuf],
) -> Option<StorageOwner> {
    // A path equal to a storage root or above one (e.g. a project created
    // from `~/workspaces`) says nothing about who owns what is below it.
    let matches: Vec<&RecordedPath> = recorded
        .iter()
        .filter(|record| !roots.iter().any(|root| contains(&record.path, root)))
        .filter(|record| match marked_workspace {
            Some(workspace) => contains(workspace, &record.path),
            None => contains(&record.path, candidate),
        })
        .collect();
    // Workspace classes: every recorded path inside the workspace competes (their
    // lengths say nothing). Otherwise the longest containing path wins.
    let best = match marked_workspace {
        Some(_) => 0,
        None => matches
            .iter()
            .map(|record| record.path.as_os_str().len())
            .max()?,
    };
    let mut owners = matches
        .into_iter()
        .filter(|record| record.path.as_os_str().len() >= best)
        .map(|record| &record.owner);
    let first = owners.next()?;
    // Different owners with an equally good claim: ambiguous, so none.
    owners
        .all(|owner| owner.kind == first.kind && owner.id == first.id)
        .then(|| first.clone())
}

/// Outcome of a Git probe that can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitCheck<T> {
    /// The candidate is not inside a Git work tree.
    NotApplicable,
    Checked(T),
    DubiousOwnership,
    Failed(String),
}

/// `git ls-files` / `git check-ignore` for generated output in a work tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GeneratedGitFacts {
    pub tracked: bool,
    pub ignored: bool,
}

/// Git state of a linked worktree (`status`, `merge-base`, flags).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct WorktreeGitFacts {
    pub changed: u64,
    pub untracked: u64,
    /// Skip-worktree / assume-unchanged entries `git status` cannot see.
    pub hidden_from_status: u64,
    pub ignored_user_files: u64,
    pub detached: bool,
    pub has_upstream: bool,
    /// The upstream `git status` reports, for example `origin/feature`.
    pub upstream: Option<String>,
    pub upstream_is_local: bool,
    /// `HEAD` is an ancestor of its upstream; `None` when unknown.
    pub pushed: Option<bool>,
    /// Remote-tracking default branch, for example `origin/main`.
    pub default_branch: Option<String>,
    /// `HEAD` is an ancestor of the default branch; `None` when unknown.
    pub merged: Option<bool>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeFacts {
    pub known_to_yard: bool,
    pub main: bool,
    pub locked: bool,
    pub prunable: bool,
    pub state: GitCheck<WorktreeGitFacts>,
}

/// Summary of a marked workspace's nested repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceSummary {
    pub state: StorageSourceState,
    pub dubious_ownership: bool,
}

/// Read-only findings for one `src/<Pkg>` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageProbe {
    pub name: String,
    pub path: PathBuf,
    pub result: PackageResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackageResult {
    NotAGitRepository,
    Git(GitCheck<PackageGitFacts>),
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PackageGitFacts {
    pub changed: u64,
    pub untracked: u64,
    pub detached: bool,
    pub has_upstream: bool,
    pub upstream_gone: bool,
    pub ahead: Option<u64>,
    pub stash: bool,
}

fn package_issues(result: &PackageResult) -> (Vec<StoragePackageIssue>, Option<u64>) {
    let mut issues = Vec::new();
    let mut ahead = None;
    match result {
        PackageResult::NotAGitRepository => issues.push(StoragePackageIssue::NotAGitRepository),
        PackageResult::Git(GitCheck::NotApplicable) => {
            issues.push(StoragePackageIssue::ProbeFailed);
        }
        PackageResult::Git(GitCheck::DubiousOwnership) => {
            issues.push(StoragePackageIssue::DubiousOwnership);
        }
        PackageResult::Git(GitCheck::Failed(_)) => issues.push(StoragePackageIssue::ProbeFailed),
        PackageResult::Git(GitCheck::Checked(facts)) => {
            if facts.changed > 0 {
                issues.push(StoragePackageIssue::UncommittedChanges);
            }
            if facts.untracked > 0 {
                issues.push(StoragePackageIssue::UntrackedFiles);
            }
            if facts.detached {
                issues.push(StoragePackageIssue::DetachedHead);
            } else if !facts.has_upstream {
                issues.push(StoragePackageIssue::NoUpstream);
            } else if facts.upstream_gone {
                issues.push(StoragePackageIssue::UpstreamGone);
            }
            ahead = facts.ahead;
            if facts.ahead.is_some_and(|ahead| ahead > 0) {
                issues.push(StoragePackageIssue::UnpushedCommits);
            }
            if facts.stash {
                issues.push(StoragePackageIssue::Stash);
            }
        }
    }
    issues.sort();
    (issues, ahead)
}

fn issue_phrase(issue: StoragePackageIssue, count: usize) -> String {
    let one = count == 1;
    let noun = if one { "package" } else { "packages" };
    let phrase = match issue {
        StoragePackageIssue::UncommittedChanges if one => "has uncommitted changes",
        StoragePackageIssue::UncommittedChanges => "have uncommitted changes",
        StoragePackageIssue::UntrackedFiles if one => "has untracked files",
        StoragePackageIssue::UntrackedFiles => "have untracked files",
        StoragePackageIssue::UnpushedCommits if one => "has unpushed commits",
        StoragePackageIssue::UnpushedCommits => "have unpushed commits",
        StoragePackageIssue::NoUpstream if one => "has no upstream branch",
        StoragePackageIssue::NoUpstream => "have no upstream branch",
        StoragePackageIssue::UpstreamGone if one => "tracks a deleted upstream branch",
        StoragePackageIssue::UpstreamGone => "track a deleted upstream branch",
        StoragePackageIssue::Stash if one => "has stashed changes",
        StoragePackageIssue::Stash => "have stashed changes",
        StoragePackageIssue::DetachedHead if one => "has a detached HEAD",
        StoragePackageIssue::DetachedHead => "have a detached HEAD",
        StoragePackageIssue::NotAGitRepository if one => "is not a Git repository",
        StoragePackageIssue::NotAGitRepository => "are not Git repositories",
        StoragePackageIssue::DubiousOwnership if one => "was refused by Git (dubious ownership)",
        StoragePackageIssue::DubiousOwnership => "were refused by Git (dubious ownership)",
        StoragePackageIssue::ProbeFailed => "could not be checked",
    };
    format!("{count} {noun} {phrase}")
}

/// Combine package probes into a workspace `source_state`.
///
/// Any concrete finding makes it `not_clean`; a failed probe or an
/// incomplete package listing without findings makes it `unknown`.
pub(crate) fn source_summary(packages: &[PackageProbe], listing_complete: bool) -> SourceSummary {
    let mut package_states = Vec::new();
    let mut counts = std::collections::BTreeMap::<StoragePackageIssue, usize>::new();
    for package in packages {
        let (issues, ahead) = package_issues(&package.result);
        for issue in &issues {
            *counts.entry(*issue).or_default() += 1;
        }
        if !issues.is_empty() {
            package_states.push(StoragePackageState {
                name: package.name.clone(),
                path: package.path.to_string_lossy().into_owned(),
                issues,
                ahead,
            });
        }
    }
    let concrete = counts
        .keys()
        .any(|issue| !matches!(issue, StoragePackageIssue::ProbeFailed));
    let failed = counts.contains_key(&StoragePackageIssue::ProbeFailed);
    let status = if concrete {
        StorageSourceStatus::NotClean
    } else if failed || !listing_complete {
        StorageSourceStatus::Unknown
    } else {
        StorageSourceStatus::Clean
    };
    let mut summary: Vec<String> = counts
        .iter()
        .map(|(issue, count)| issue_phrase(*issue, *count))
        .collect();
    if !listing_complete {
        summary.push("Not every package was checked".to_owned());
    }
    SourceSummary {
        dubious_ownership: counts.contains_key(&StoragePackageIssue::DubiousOwnership),
        state: StorageSourceState {
            status,
            package_count: u64::try_from(packages.len()).unwrap_or(u64::MAX),
            summary,
            packages: package_states,
        },
    }
}

/// Everything the verdict depends on for one candidate.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(crate) struct CandidateFacts<'a> {
    pub class: StorageClass,
    /// Sizing finished within the budgets (not `estimate_truncated`).
    pub complete: bool,
    /// A `.git` entry exists inside a generated or unknown candidate.
    pub contains_git: bool,
    /// A `.git` entry exists below a linked worktree's root (a nested
    /// repository or submodule whose commits may exist nowhere else).
    pub contains_nested_repo: bool,
    pub crosses_device: bool,
    /// The path canonicalized to itself under a root after the walk.
    pub path_verified: bool,
    pub in_use: &'a InUse,
    pub generated_git: &'a GitCheck<GeneratedGitFacts>,
    /// Workspace `build/` and `env/`: the workspace source state.
    pub source: Option<&'a SourceSummary>,
    /// Workspace `build/`: `*.log` newer than retention. `workspace_logs`: newest
    /// entry newer than retention.
    pub recent_logs: bool,
    pub worktree: Option<&'a WorktreeFacts>,
    /// Why an `unknown` candidate is report-only.
    pub unknown_detail: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub safety: StorageSafety,
    pub reasons: Vec<String>,
}

#[derive(Default)]
struct Reasons {
    blocked: Vec<String>,
    review: Vec<String>,
}

impl Reasons {
    fn block(&mut self, reason: impl Into<String>) {
        self.blocked.push(format!("Blocked: {}", reason.into()));
    }

    fn review(&mut self, reason: impl Into<String>) {
        self.review.push(format!("Review: {}", reason.into()));
    }
}

/// Decide `safe | review | blocked` for one candidate.
pub(crate) fn classify(facts: &CandidateFacts<'_>) -> Verdict {
    let mut reasons = Reasons::default();
    if facts.class == StorageClass::Unknown {
        reasons.blocked.push(format!(
            "Report only: {}",
            facts
                .unknown_detail
                .unwrap_or("Yard does not recognize this directory")
        ));
    }
    if facts.contains_git && facts.class != StorageClass::GitWorktree {
        reasons.block("contains a Git repository");
    }
    if facts.contains_nested_repo && facts.class == StorageClass::GitWorktree {
        reasons.block("contains another Git repository or submodule");
    }
    if facts.crosses_device {
        reasons.block("contains another file system");
    }
    if !facts.path_verified {
        reasons.block("path changed during the scan");
    }
    match facts.in_use {
        InUse::Free => {}
        InUse::Busy => reasons.block("in use"),
        InUse::Unavailable => reasons.block("in-use check unavailable"),
    }
    if let Some(worktree) = facts.worktree {
        worktree_reasons(worktree, &mut reasons);
    } else if facts.class == StorageClass::GitWorktree {
        // A worktree without confirmed facts fails closed.
        reasons.block("Git state unknown");
    }
    match facts.generated_git {
        GitCheck::NotApplicable => {}
        GitCheck::Checked(git) => {
            if git.tracked {
                reasons.review("contains tracked files");
            } else if !git.ignored {
                reasons.review("not ignored by Git");
            }
        }
        GitCheck::DubiousOwnership => {
            reasons.block("Git refused the repository (dubious ownership)");
        }
        GitCheck::Failed(error) => reasons.block(format!("Git check failed ({error})")),
    }
    if facts.class == StorageClass::NodeModules {
        reasons.review("dependencies must be reinstalled");
    }
    if let Some(source) = facts.source {
        if source.dubious_ownership {
            reasons.block("Git refused a package repository (dubious ownership)");
        }
        match source.state.status {
            StorageSourceStatus::Clean => {}
            StorageSourceStatus::NotClean | StorageSourceStatus::Unknown => {
                if source.state.summary.is_empty() {
                    reasons.review("source state unknown");
                }
                for line in &source.state.summary {
                    reasons.review(line.clone());
                }
            }
        }
    }
    if facts.recent_logs {
        reasons.review("contains recent build logs");
    }
    if !facts.complete {
        reasons.review("size estimate truncated");
    }
    let safety = if !reasons.blocked.is_empty() {
        StorageSafety::Blocked
    } else if !reasons.review.is_empty() {
        StorageSafety::Review
    } else {
        StorageSafety::Safe
    };
    let mut all = reasons.blocked;
    all.extend(reasons.review);
    Verdict {
        safety,
        reasons: all,
    }
}

fn worktree_reasons(worktree: &WorktreeFacts, reasons: &mut Reasons) {
    if !worktree.known_to_yard {
        reasons.block("not a Yard project checkout");
    }
    if worktree.main {
        reasons.block("main worktree");
    }
    if worktree.locked {
        reasons.block("locked worktree");
    }
    if worktree.prunable {
        reasons.block("prunable worktree");
    }
    match &worktree.state {
        GitCheck::NotApplicable => reasons.block("Git state unknown"),
        GitCheck::DubiousOwnership => {
            reasons.block("Git refused the repository (dubious ownership)");
        }
        GitCheck::Failed(error) => reasons.block(format!("Git check failed ({error})")),
        GitCheck::Checked(git) => {
            if git.changed > 0 {
                reasons.block("uncommitted changes");
            }
            if git.untracked > 0 {
                reasons.block(format!("untracked files ({})", git.untracked));
            }
            if git.hidden_from_status > 0 {
                reasons.block(format!(
                    "files hidden from git status ({}, skip-worktree/assume-unchanged)",
                    git.hidden_from_status
                ));
            }
            if git.ignored_user_files > 0 {
                reasons.block(format!("ignored user files ({})", git.ignored_user_files));
            }
            if git.detached {
                reasons.block("detached HEAD");
            } else if !git.has_upstream {
                reasons.block("no upstream branch");
            } else if git.upstream_is_local {
                reasons.block("upstream is a local branch");
            } else {
                match git.pushed {
                    Some(true) => {}
                    Some(false) => reasons.block("unpushed commits"),
                    None => reasons.block("push state unknown"),
                }
            }
            match (&git.default_branch, git.merged) {
                (None, _) => reasons.block("default branch unknown"),
                (Some(_), Some(true)) => {}
                (Some(branch), Some(false)) => reasons.block(format!("not merged into {branch}")),
                (Some(_), None) => reasons.block("merge state unknown"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use yard_domain::StorageOwnerKind;

    use super::*;

    fn markers_with(names: &[(&str, bool)], workspace_markers: &[String]) -> SiblingMarkers {
        let mut markers = SiblingMarkers::default();
        for (name, is_dir) in names {
            markers.observe(OsStr::new(name), !is_dir, *is_dir, workspace_markers);
        }
        markers
    }

    fn markers(names: &[(&str, bool)]) -> SiblingMarkers {
        markers_with(names, &["workspace.toml".to_owned()])
    }

    fn facts(class: StorageClass) -> CandidateFacts<'static> {
        CandidateFacts {
            class,
            complete: true,
            contains_git: false,
            contains_nested_repo: false,
            crosses_device: false,
            path_verified: true,
            in_use: &InUse::Free,
            generated_git: &GitCheck::NotApplicable,
            source: None,
            recent_logs: false,
            worktree: None,
            unknown_detail: None,
        }
    }

    fn clean_worktree() -> WorktreeFacts {
        WorktreeFacts {
            known_to_yard: true,
            main: false,
            locked: false,
            prunable: false,
            state: GitCheck::Checked(WorktreeGitFacts {
                has_upstream: true,
                pushed: Some(true),
                default_branch: Some("origin/main".to_owned()),
                merged: Some(true),
                ..WorktreeGitFacts::default()
            }),
        }
    }

    #[test]
    fn classes_need_exact_names_and_sibling_markers() {
        let cargo = markers(&[("Cargo.toml", false)]);
        assert_eq!(
            generated_class(OsStr::new("target"), cargo),
            Some(StorageClass::RustTarget)
        );
        assert_eq!(generated_class(OsStr::new("Target"), cargo), None);
        assert_eq!(generated_class(OsStr::new("target"), markers(&[])), None);
        assert_eq!(
            generated_class(OsStr::new("target"), markers(&[("Cargo.toml", true)])),
            None,
            "a directory named Cargo.toml is not a marker"
        );
        assert_eq!(
            generated_class(
                OsStr::new("node_modules"),
                markers(&[("package.json", false)])
            ),
            Some(StorageClass::NodeModules)
        );
        assert_eq!(
            generated_class(OsStr::new("dist"), markers(&[("vite.config.ts", false)])),
            Some(StorageClass::ViteDist)
        );
        assert_eq!(
            generated_class(OsStr::new("dist"), markers(&[("vite.config.json", false)])),
            None
        );
        assert_eq!(
            generated_class(OsStr::new("dist"), markers(&[("package.json", false)])),
            None
        );
        for marker in ["build.gradle", "settings.gradle.kts"] {
            let gradle = markers(&[(marker, false)]);
            assert_eq!(
                generated_class(OsStr::new("build"), gradle),
                Some(StorageClass::GradleOutput)
            );
            assert_eq!(
                generated_class(OsStr::new(".gradle"), gradle),
                Some(StorageClass::GradleOutput)
            );
        }
        let marked = markers(&[("workspace.toml", false), ("src", true)]);
        assert!(marked.is_workspace_root());
        assert!(!markers(&[("workspace.toml", false)]).is_workspace_root());
        assert!(!markers(&[("workspace.toml", true), ("src", true)]).is_workspace_root());
        assert!(
            !markers_with(&[("workspace.toml", false), ("src", true)], &[]).is_workspace_root(),
            "no configured marker: workspace classes are disabled"
        );
        assert!(
            !markers(&[("other.toml", false), ("src", true)]).is_workspace_root(),
            "only a configured marker name counts"
        );
        assert_eq!(
            workspace_child_class(OsStr::new("env")),
            Some(StorageClass::WorkspaceEnv)
        );
        assert_eq!(
            workspace_child_class(OsStr::new(".build-logs")),
            Some(StorageClass::WorkspaceLogs)
        );
        assert_eq!(workspace_child_class(OsStr::new("src")), None);
    }

    #[test]
    fn in_use_counts_equal_or_inside_paths_but_never_ancestors() {
        let set = |paths: &[&str]| paths.iter().map(PathBuf::from).collect::<BTreeSet<_>>();
        let candidate = Path::new("/work/app/target");
        assert!(is_in_use(candidate, &set(&["/work/app/target"])));
        assert!(is_in_use(candidate, &set(&["/work/app/target/debug/app"])));
        assert!(!is_in_use(candidate, &set(&["/work/app", "/", "/work"])));
        assert!(!is_in_use(candidate, &set(&["/work/app/target2"])));
        assert!(!is_in_use(
            candidate,
            &set(&[
                "/work/app/target-old",
                "/work/app/target.bak",
                "/work/app/target2"
            ])
        ));
        assert!(is_in_use(
            candidate,
            &set(&[
                "/work/app",
                "/work/app/target-old",
                "/work/app/target/deep/x",
                "/work/app/target2",
                "/zzz"
            ])
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn owner_is_the_longest_containing_record_and_workspace_matches_inside_the_workspace() {
        let owner = |id: &str| StorageOwner {
            kind: StorageOwnerKind::Project,
            id: id.to_owned(),
            name: id.to_owned(),
        };
        let recorded = vec![
            RecordedPath {
                path: PathBuf::from("/work"),
                owner: owner("outer"),
            },
            RecordedPath {
                path: PathBuf::from("/work/app"),
                owner: owner("inner"),
            },
            RecordedPath {
                path: PathBuf::from("/ws/one/src/Pkg"),
                owner: owner("workspace"),
            },
        ];
        assert_eq!(
            resolve_owner(Path::new("/work/app/target"), None, &recorded, &[])
                .map(|owner| owner.id),
            Some("inner".to_owned())
        );
        assert_eq!(
            resolve_owner(Path::new("/other/target"), None, &recorded, &[]),
            None
        );
        assert_eq!(
            resolve_owner(
                Path::new("/ws/one/build"),
                Some(Path::new("/ws/one")),
                &recorded,
                &[]
            )
            .map(|owner| owner.id),
            Some("workspace".to_owned())
        );
        assert_eq!(
            resolve_owner(
                Path::new("/work/ws/build"),
                Some(Path::new("/work/ws")),
                &recorded,
                &[]
            ),
            None,
            "an outer project path does not own a marked workspace"
        );

        let mut shared = recorded.clone();
        for (path, id) in [
            ("/roots/ws", "token"),
            ("/roots/ws", "quic"),
            ("/roots", "above"),
        ] {
            shared.push(RecordedPath {
                path: PathBuf::from(path),
                owner: owner(id),
            });
        }
        let roots = [PathBuf::from("/roots/ws")];
        let solo = [RecordedPath {
            path: PathBuf::from("/roots/ws"),
            owner: owner("solo"),
        }];
        assert_eq!(
            resolve_owner(Path::new("/roots/ws/wt"), None, &solo, &[]).map(|owner| owner.id),
            Some("solo".to_owned()),
            "control: without roots the record would own the worktree"
        );
        assert_eq!(
            resolve_owner(Path::new("/roots/ws/wt"), None, &solo, &roots),
            None,
            "a recorded storage root owns nothing below it"
        );
        assert_eq!(
            resolve_owner(Path::new("/roots/ws/wt"), None, &shared, &roots),
            None,
            "a storage root or its ancestor recorded as a cwd owns nothing"
        );
        assert_eq!(
            resolve_owner(Path::new("/roots/ws/wt"), None, &shared, &[]),
            None,
            "different owners tied on one path: ambiguous"
        );
        shared.push(RecordedPath {
            path: PathBuf::from("/ws/one/src/Other"),
            owner: owner("other"),
        });
        assert_eq!(
            resolve_owner(
                Path::new("/ws/one/env"),
                Some(Path::new("/ws/one")),
                &shared,
                &[]
            ),
            None,
            "two projects inside one marked workspace: ambiguous"
        );
        shared.push(RecordedPath {
            path: PathBuf::from("/work/app"),
            owner: owner("inner"),
        });
        assert_eq!(
            resolve_owner(Path::new("/work/app/target"), None, &shared, &roots)
                .map(|owner| owner.id),
            Some("inner".to_owned()),
            "the same owner recorded twice is not a tie"
        );
    }

    #[test]
    fn generated_defaults_and_git_rules() {
        assert_eq!(
            classify(&facts(StorageClass::RustTarget)).safety,
            StorageSafety::Safe
        );
        assert_eq!(
            classify(&facts(StorageClass::ViteDist)).safety,
            StorageSafety::Safe
        );
        assert_eq!(
            classify(&facts(StorageClass::GradleOutput)).safety,
            StorageSafety::Safe
        );
        let node = classify(&facts(StorageClass::NodeModules));
        assert_eq!(node.safety, StorageSafety::Review);
        assert_eq!(node.reasons, ["Review: dependencies must be reinstalled"]);

        let tracked = GitCheck::Checked(GeneratedGitFacts {
            tracked: true,
            ignored: true,
        });
        let verdict = classify(&CandidateFacts {
            generated_git: &tracked,
            ..facts(StorageClass::RustTarget)
        });
        assert_eq!(verdict.safety, StorageSafety::Review);
        assert_eq!(verdict.reasons, ["Review: contains tracked files"]);
        let unignored = GitCheck::Checked(GeneratedGitFacts {
            tracked: false,
            ignored: false,
        });
        assert_eq!(
            classify(&CandidateFacts {
                generated_git: &unignored,
                ..facts(StorageClass::ViteDist)
            })
            .reasons,
            ["Review: not ignored by Git"]
        );
        let ignored = GitCheck::Checked(GeneratedGitFacts {
            tracked: false,
            ignored: true,
        });
        assert_eq!(
            classify(&CandidateFacts {
                generated_git: &ignored,
                ..facts(StorageClass::RustTarget)
            })
            .safety,
            StorageSafety::Safe
        );
        for failure in [
            GitCheck::DubiousOwnership,
            GitCheck::Failed("timeout".to_owned()),
        ] {
            assert_eq!(
                classify(&CandidateFacts {
                    generated_git: &failure,
                    ..facts(StorageClass::RustTarget)
                })
                .safety,
                StorageSafety::Blocked
            );
        }
    }

    #[test]
    fn truncation_caps_at_review_and_hard_blocks_stay_blocked() {
        let truncated = classify(&CandidateFacts {
            complete: false,
            ..facts(StorageClass::RustTarget)
        });
        assert_eq!(truncated.safety, StorageSafety::Review);
        assert_eq!(truncated.reasons, ["Review: size estimate truncated"]);
        let worktree = clean_worktree();
        assert_eq!(
            classify(&CandidateFacts {
                complete: false,
                worktree: Some(&worktree),
                ..facts(StorageClass::GitWorktree)
            })
            .safety,
            StorageSafety::Review
        );
        let blocked = classify(&CandidateFacts {
            complete: false,
            contains_git: true,
            ..facts(StorageClass::NodeModules)
        });
        assert_eq!(blocked.safety, StorageSafety::Blocked);
        assert_eq!(blocked.reasons[0], "Blocked: contains a Git repository");
        assert_eq!(
            classify(&CandidateFacts {
                crosses_device: true,
                ..facts(StorageClass::RustTarget)
            })
            .reasons,
            ["Blocked: contains another file system"]
        );
        assert_eq!(
            classify(&CandidateFacts {
                path_verified: false,
                ..facts(StorageClass::ViteDist)
            })
            .reasons,
            ["Blocked: path changed during the scan"]
        );
        let unknown = classify(&CandidateFacts {
            unknown_detail: Some("orphan cleanup quarantine"),
            ..facts(StorageClass::Unknown)
        });
        assert_eq!(unknown.safety, StorageSafety::Blocked);
        assert_eq!(unknown.reasons, ["Report only: orphan cleanup quarantine"]);
    }

    #[test]
    fn in_use_and_unavailable_probes_block() {
        assert_eq!(
            classify(&CandidateFacts {
                in_use: &InUse::Busy,
                ..facts(StorageClass::RustTarget)
            })
            .reasons,
            ["Blocked: in use"]
        );
        assert_eq!(
            classify(&CandidateFacts {
                in_use: &InUse::Unavailable,
                ..facts(StorageClass::ViteDist)
            })
            .reasons,
            ["Blocked: in-use check unavailable"]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn workspace_source_state_and_logs_move_build_and_env_to_review() {
        let clean = source_summary(
            &[PackageProbe {
                name: "Clean".to_owned(),
                path: PathBuf::from("/ws/src/Clean"),
                result: PackageResult::Git(GitCheck::Checked(PackageGitFacts {
                    has_upstream: true,
                    ahead: Some(0),
                    ..PackageGitFacts::default()
                })),
            }],
            true,
        );
        assert_eq!(clean.state.status, StorageSourceStatus::Clean);
        assert!(clean.state.summary.is_empty());
        assert_eq!(
            classify(&CandidateFacts {
                source: Some(&clean),
                ..facts(StorageClass::WorkspaceEnv)
            })
            .safety,
            StorageSafety::Safe
        );
        let ahead = source_summary(
            &[
                PackageProbe {
                    name: "Ahead".to_owned(),
                    path: PathBuf::from("/ws/src/Ahead"),
                    result: PackageResult::Git(GitCheck::Checked(PackageGitFacts {
                        has_upstream: true,
                        ahead: Some(1),
                        ..PackageGitFacts::default()
                    })),
                },
                PackageProbe {
                    name: "Plain".to_owned(),
                    path: PathBuf::from("/ws/src/Plain"),
                    result: PackageResult::NotAGitRepository,
                },
            ],
            true,
        );
        assert_eq!(ahead.state.status, StorageSourceStatus::NotClean);
        assert_eq!(
            ahead.state.summary,
            [
                "1 package has unpushed commits",
                "1 package is not a Git repository"
            ]
        );
        assert_eq!(ahead.state.packages[0].ahead, Some(1));
        let verdict = classify(&CandidateFacts {
            source: Some(&ahead),
            ..facts(StorageClass::WorkspaceBuild)
        });
        assert_eq!(verdict.safety, StorageSafety::Review);
        assert_eq!(
            verdict.reasons,
            [
                "Review: 1 package has unpushed commits",
                "Review: 1 package is not a Git repository"
            ]
        );
        let unknown = source_summary(&[], false);
        assert_eq!(unknown.state.status, StorageSourceStatus::Unknown);
        let failed = source_summary(
            &[PackageProbe {
                name: "Slow".to_owned(),
                path: PathBuf::from("/ws/src/Slow"),
                result: PackageResult::Git(GitCheck::Failed("timeout".to_owned())),
            }],
            true,
        );
        assert_eq!(failed.state.status, StorageSourceStatus::Unknown);
        assert_eq!(
            classify(&CandidateFacts {
                source: Some(&failed),
                ..facts(StorageClass::WorkspaceEnv)
            })
            .safety,
            StorageSafety::Review
        );
        let dubious = source_summary(
            &[PackageProbe {
                name: "Foreign".to_owned(),
                path: PathBuf::from("/ws/src/Foreign"),
                result: PackageResult::Git(GitCheck::DubiousOwnership),
            }],
            true,
        );
        assert_eq!(
            classify(&CandidateFacts {
                source: Some(&dubious),
                ..facts(StorageClass::WorkspaceEnv)
            })
            .safety,
            StorageSafety::Blocked
        );
        assert_eq!(
            classify(&CandidateFacts {
                source: Some(&clean),
                recent_logs: true,
                ..facts(StorageClass::WorkspaceBuild)
            })
            .reasons,
            ["Review: contains recent build logs"]
        );
    }

    #[test]
    fn worktrees_are_safe_only_when_every_condition_holds() {
        type Edit = fn(&mut WorktreeFacts);
        fn git(worktree: &mut WorktreeFacts) -> &mut WorktreeGitFacts {
            match &mut worktree.state {
                GitCheck::Checked(git) => git,
                _ => unreachable!(),
            }
        }
        let clean = clean_worktree();
        assert_eq!(
            classify(&CandidateFacts {
                worktree: Some(&clean),
                ..facts(StorageClass::GitWorktree)
            }),
            Verdict {
                safety: StorageSafety::Safe,
                reasons: Vec::new()
            }
        );
        let with = |edit: fn(&mut WorktreeFacts)| {
            let mut worktree = clean_worktree();
            edit(&mut worktree);
            classify(&CandidateFacts {
                worktree: Some(&worktree),
                ..facts(StorageClass::GitWorktree)
            })
        };
        let cases: [(Edit, &str); 12] = [
            (
                |w| w.known_to_yard = false,
                "Blocked: not a Yard project checkout",
            ),
            (|w| w.locked = true, "Blocked: locked worktree"),
            (|w| w.prunable = true, "Blocked: prunable worktree"),
            (|w| w.main = true, "Blocked: main worktree"),
            (|w| git(w).changed = 2, "Blocked: uncommitted changes"),
            (|w| git(w).untracked = 3, "Blocked: untracked files (3)"),
            (
                |w| git(w).ignored_user_files = 3,
                "Blocked: ignored user files (3)",
            ),
            (|w| git(w).detached = true, "Blocked: detached HEAD"),
            (
                |w| git(w).has_upstream = false,
                "Blocked: no upstream branch",
            ),
            (|w| git(w).pushed = Some(false), "Blocked: unpushed commits"),
            (
                |w| git(w).merged = Some(false),
                "Blocked: not merged into origin/main",
            ),
            (
                |w| w.state = GitCheck::DubiousOwnership,
                "Blocked: Git refused the repository (dubious ownership)",
            ),
        ];
        for (edit, reason) in cases {
            let verdict = with(edit);
            assert_eq!(verdict.safety, StorageSafety::Blocked, "{reason}");
            assert_eq!(verdict.reasons, [reason]);
        }
        assert_eq!(
            with(|w| git(w).default_branch = None).reasons,
            ["Blocked: default branch unknown"]
        );
        assert_eq!(
            with(|w| git(w).upstream_is_local = true).reasons,
            ["Blocked: upstream is a local branch"]
        );
        assert_eq!(
            classify(&facts(StorageClass::GitWorktree)),
            Verdict {
                safety: StorageSafety::Blocked,
                reasons: vec!["Blocked: Git state unknown".to_owned()]
            },
            "a worktree without confirmed facts fails closed"
        );
    }
}

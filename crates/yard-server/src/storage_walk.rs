//! Blocking, read-only, no-follow storage walker (runs in `spawn_blocking`).
//!
//! The walk is single-threaded and sequential, depth-limited, and bounded
//! by an entry budget and a deadline. It never follows symlinks: entry types
//! come from `DirEntry::file_type` and sizes from `symlink_metadata`. It never
//! descends into `.git`, a workspace `src/`, an excluded Yard path, or another
//! file system. Bytes are allocated blocks, attributed to the innermost
//! candidate so each byte is counted once. A file with more than one hard
//! link is counted once per candidate: reclaimable when every link was seen
//! inside that candidate, else shared (not reclaimable).

use std::{
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs::{self, FileType, Metadata},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime},
};

use yard_domain::{StorageClass, StorageTruncationReason};

use crate::storage_classify::{
    RECLAIM_PREFIX, SiblingMarkers, contains, generated_class, workspace_child_class,
};

const MAX_GIT_FILE_BYTES: u64 = 4096;
const DEADLINE_CHECK_INTERVAL: u64 = 256;
/// How many multiply linked inodes are tracked; beyond this every further
/// link is counted as shared (never reclaimable).
const MAX_TRACKED_HARD_LINKS: usize = 1_000_000;
/// How many never-searched directories are named in the result.
pub(crate) const MAX_UNDISCOVERED_PATHS: usize = 20;

#[derive(Debug, Clone)]
pub(crate) struct WalkSettings {
    /// Canonical roots.
    pub roots: Vec<PathBuf>,
    /// Canonical paths no candidate may be inside or contain.
    pub excluded: Vec<PathBuf>,
    pub max_entries: u64,
    pub deadline: Instant,
    pub max_discovery_depth: usize,
    pub max_size_depth: usize,
    pub max_candidates: usize,
    pub max_workspace_packages: usize,
    /// File names that mark a workspace root (next to a `src/` directory).
    /// Empty disables the workspace classes.
    pub workspace_markers: Vec<String>,
    /// Set when Yard is stopping; the walk ends at the next check.
    pub cancel: Arc<AtomicBool>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(crate) struct WalkCandidate {
    pub path: PathBuf,
    /// `GitWorktree` is provisional until the repository lists it.
    pub class: StorageClass,
    pub unknown_detail: Option<&'static str>,
    pub parent: Option<usize>,
    /// Where the enclosing Git work tree is looked up (for the
    /// generated-output rule); the scan asks Git to resolve it.
    pub repo_scope: Option<RepoScope>,
    pub marked_workspace: Option<usize>,
    pub bytes: u64,
    pub shared_bytes: u64,
    pub file_count: u64,
    pub newest_mtime: Option<SystemTime>,
    pub newest_log_mtime: Option<SystemTime>,
    pub contains_git: bool,
    /// A `.git` entry (of any type) lies below this candidate's root.
    pub contains_nested_repo: bool,
    pub crosses_device: bool,
    /// Sizing finished within the budgets.
    pub complete: bool,
    /// After the walk, the path still canonicalizes to itself and lies under
    /// a root (it was not swapped for a symlink while the scan ran).
    pub path_verified: bool,
}

/// Where a candidate's enclosing repository is looked up. The walk only
/// records where `.git` entries are; Git decides which of them are valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoScope {
    /// Whatever Git discovers for `WalkSettings::roots[index]`.
    Root(usize),
    /// `WalkOutput::nested_git[index]`, or its enclosing scope when Git does
    /// not accept that `.git`.
    Nested(usize),
}

/// A directory inside the walk that holds a `.git` entry (of any type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NestedGit {
    pub path: PathBuf,
    pub enclosing: RepoScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePackage {
    pub name: String,
    pub path: PathBuf,
    pub has_git: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct MarkedWorkspace {
    pub path: PathBuf,
    pub packages: Vec<WorkspacePackage>,
    pub listing_complete: bool,
}

#[derive(Debug, Default)]
pub(crate) struct WalkOutput {
    pub candidates: Vec<WalkCandidate>,
    pub nested_git: Vec<NestedGit>,
    pub workspaces: Vec<MarkedWorkspace>,
    pub entries: u64,
    pub truncation: BTreeSet<StorageTruncationReason>,
    pub unreadable: u64,
    pub depth_limited: u64,
    pub skipped_roots: Vec<(PathBuf, &'static str)>,
    /// Roots that are themselves linked worktrees (never a candidate: that
    /// would offer the whole configured root for removal).
    pub worktree_roots: Vec<PathBuf>,
    /// Directories that were never searched for candidates because a budget
    /// ran out (the first `MAX_UNDISCOVERED_PATHS`, in walk order), and their
    /// total count. Candidates below them are missing from the result.
    pub undiscovered: Vec<PathBuf>,
    pub undiscovered_count: u64,
    /// The walk stopped because `WalkSettings::cancel` was set.
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Discover,
    Size,
}

#[derive(Debug)]
struct Frame {
    path: PathBuf,
    depth: usize,
    size_depth: usize,
    mode: Mode,
    attribution: Option<usize>,
    repo_scope: RepoScope,
    dev: u64,
    parent_is_worktrees: bool,
}

struct Walker<'a> {
    settings: &'a WalkSettings,
    output: WalkOutput,
    /// Discovery runs to completion before any candidate is sized, so a
    /// budget that runs out while sizing never hides an undiscovered sibling.
    discover_stack: Vec<Frame>,
    size_stack: Vec<Frame>,
    /// Per-entry `lstat` calls since the last deadline check.
    stat_ops: u64,
    stopped: bool,
    /// Multiply linked files per (candidate, device, inode).
    hard_links: HashMap<(usize, u64, u64), HardLink>,
}

/// One multiply linked inode inside one candidate.
#[derive(Debug)]
struct HardLink {
    links_seen: u64,
    nlink: u64,
    allocated: u64,
}

/// Walk every root. Never panics on I/O errors; unreadable directories mark
/// the affected candidate as truncated.
pub(crate) fn walk(settings: &WalkSettings) -> WalkOutput {
    let mut walker = Walker {
        settings,
        output: WalkOutput::default(),
        discover_stack: Vec::new(),
        size_stack: Vec::new(),
        stat_ops: 0,
        stopped: false,
        hard_links: HashMap::new(),
    };
    for (index, root) in settings.roots.iter().enumerate() {
        walker.push_root(index, root);
    }
    // Roots are popped in configuration order.
    walker.discover_stack.reverse();
    while let Some(frame) = walker
        .discover_stack
        .pop()
        .or_else(|| walker.size_stack.pop())
    {
        if walker.budget_exhausted() {
            walker.abandon(&frame);
            break;
        }
        match frame.mode {
            Mode::Discover => walker.discover(&frame),
            Mode::Size => walker.size(&frame),
        }
        if walker.stopped {
            walker.abandon(&frame);
            break;
        }
    }
    let remaining: Vec<Frame> = walker
        .discover_stack
        .drain(..)
        .rev()
        .chain(walker.size_stack.drain(..))
        .collect();
    for frame in &remaining {
        walker.abandon(frame);
    }
    walker.settle_hard_links();
    for candidate in &mut walker.output.candidates {
        candidate.path_verified = settings
            .roots
            .iter()
            .any(|root| contains(root, &candidate.path))
            && fs::canonicalize(&candidate.path).is_ok_and(|path| path == candidate.path);
    }
    walker.output
}

impl Walker<'_> {
    fn push_root(&mut self, index: usize, root: &Path) {
        if self.is_excluded(root) {
            self.output
                .skipped_roots
                .push((root.to_path_buf(), "inside a Yard data or runtime path"));
            return;
        }
        let Ok(metadata) = fs::symlink_metadata(root) else {
            self.output
                .skipped_roots
                .push((root.to_path_buf(), "not accessible"));
            return;
        };
        if !metadata.is_dir() {
            self.output
                .skipped_roots
                .push((root.to_path_buf(), "not a directory"));
            return;
        }
        for ancestor in root.ancestors().skip(1) {
            if inside_workspace_src(root, ancestor, &self.settings.workspace_markers) {
                self.output
                    .skipped_roots
                    .push((root.to_path_buf(), "inside a marked workspace src/"));
                return;
            }
        }
        self.push(Frame {
            path: root.to_path_buf(),
            depth: 0,
            size_depth: 0,
            mode: Mode::Discover,
            attribution: None,
            // Git, not the presence of an ancestor `.git`, decides which
            // repository (if any) encloses the root.
            repo_scope: RepoScope::Root(index),
            dev: metadata.dev(),
            // A root itself is never reported as an unknown `.worktrees` entry.
            parent_is_worktrees: false,
        });
    }

    fn is_excluded(&self, path: &Path) -> bool {
        self.settings
            .excluded
            .iter()
            .any(|excluded| contains(excluded, path))
    }

    fn contains_excluded(&self, path: &Path) -> bool {
        self.settings
            .excluded
            .iter()
            .any(|excluded| contains(path, excluded))
    }

    fn push(&mut self, frame: Frame) {
        match frame.mode {
            Mode::Discover => self.discover_stack.push(frame),
            Mode::Size => self.size_stack.push(frame),
        }
    }

    /// A frame that will never run: its candidates are incomplete, and a
    /// discovery frame's subtree was never searched.
    fn abandon(&mut self, frame: &Frame) {
        self.mark_chain_incomplete(frame.attribution);
        if frame.mode == Mode::Discover {
            self.output.undiscovered_count += 1;
            if self.output.undiscovered.len() < MAX_UNDISCOVERED_PATHS {
                self.output.undiscovered.push(frame.path.clone());
            }
        }
    }

    fn check_cancel(&mut self) -> bool {
        if !self.stopped && self.settings.cancel.load(Ordering::Relaxed) {
            self.output.cancelled = true;
            self.stopped = true;
        }
        self.stopped
    }

    /// Before one per-entry `lstat`: every `DEADLINE_CHECK_INTERVAL` calls,
    /// report whether the deadline passed or the walk was cancelled.
    fn tick(&mut self) -> bool {
        if self.stopped {
            return true;
        }
        self.stat_ops += 1;
        if self.stat_ops % DEADLINE_CHECK_INTERVAL == 0 {
            return self.budget_exhausted();
        }
        false
    }

    /// Count one entry and report whether a budget has run out.
    fn count_entry(&mut self) -> bool {
        self.output.entries += 1;
        if self.check_cancel() {
            return true;
        }
        if self.output.entries > self.settings.max_entries {
            self.output
                .truncation
                .insert(StorageTruncationReason::EntryBudget);
            self.stopped = true;
        } else if self.output.entries % DEADLINE_CHECK_INTERVAL == 0
            && Instant::now() >= self.settings.deadline
        {
            self.output
                .truncation
                .insert(StorageTruncationReason::TimeBudget);
            self.stopped = true;
        }
        self.stopped
    }

    fn budget_exhausted(&mut self) -> bool {
        if self.check_cancel() {
            return true;
        }
        if Instant::now() >= self.settings.deadline {
            self.output
                .truncation
                .insert(StorageTruncationReason::TimeBudget);
            self.stopped = true;
        }
        self.stopped
    }

    fn mark_chain_incomplete(&mut self, mut attribution: Option<usize>) {
        while let Some(index) = attribution {
            let candidate = &mut self.output.candidates[index];
            candidate.complete = false;
            attribution = candidate.parent;
        }
    }

    fn mark_chain_nested_repo(&mut self, mut attribution: Option<usize>) {
        while let Some(index) = attribution {
            let candidate = &mut self.output.candidates[index];
            candidate.contains_nested_repo = true;
            attribution = candidate.parent;
        }
    }

    fn mark_chain_crosses_device(&mut self, mut attribution: Option<usize>) {
        while let Some(index) = attribution {
            let candidate = &mut self.output.candidates[index];
            candidate.crosses_device = true;
            attribution = candidate.parent;
        }
    }

    fn add_entry(&mut self, attribution: Option<usize>, name: &OsStr, metadata: &Metadata) {
        let Some(index) = attribution else {
            return;
        };
        let candidate = &mut self.output.candidates[index];
        let allocated = metadata.blocks().saturating_mul(512);
        if !metadata.is_dir() && metadata.nlink() > 1 {
            let key = (index, metadata.dev(), metadata.ino());
            if let Some(link) = self.hard_links.get_mut(&key) {
                link.links_seen += 1;
            } else if self.hard_links.len() < MAX_TRACKED_HARD_LINKS {
                self.hard_links.insert(
                    key,
                    HardLink {
                        links_seen: 1,
                        nlink: metadata.nlink(),
                        allocated,
                    },
                );
            } else {
                candidate.shared_bytes = candidate.shared_bytes.saturating_add(allocated);
            }
        } else {
            candidate.bytes = candidate.bytes.saturating_add(allocated);
        }
        let modified = metadata.modified().ok();
        if !metadata.is_dir() {
            candidate.file_count += 1;
            if metadata.is_file() && name.as_encoded_bytes().ends_with(b".log") {
                candidate.newest_log_mtime = newest(candidate.newest_log_mtime, modified);
            }
        }
        candidate.newest_mtime = newest(candidate.newest_mtime, modified);
    }

    /// Count each multiply linked inode once: reclaimable when all of its
    /// links lie inside the candidate, otherwise shared.
    fn settle_hard_links(&mut self) {
        for ((index, _, _), link) in self.hard_links.drain() {
            let candidate = &mut self.output.candidates[index];
            if link.links_seen >= link.nlink {
                candidate.bytes = candidate.bytes.saturating_add(link.allocated);
            } else {
                candidate.shared_bytes = candidate.shared_bytes.saturating_add(link.allocated);
            }
        }
    }

    fn create_candidate(
        &mut self,
        path: PathBuf,
        class: StorageClass,
        parent: Option<usize>,
        repo_scope: Option<RepoScope>,
        marked_workspace: Option<usize>,
    ) -> Option<usize> {
        if self.output.candidates.len() >= self.settings.max_candidates {
            self.output
                .truncation
                .insert(StorageTruncationReason::CandidateBudget);
            self.mark_chain_incomplete(parent);
            return None;
        }
        self.output.candidates.push(WalkCandidate {
            path,
            class,
            unknown_detail: None,
            parent,
            repo_scope,
            marked_workspace,
            bytes: 0,
            shared_bytes: 0,
            file_count: 0,
            newest_mtime: None,
            newest_log_mtime: None,
            contains_git: false,
            contains_nested_repo: false,
            crosses_device: false,
            complete: true,
            path_verified: false,
        });
        Some(self.output.candidates.len() - 1)
    }

    fn read_listing(&mut self, frame: &Frame) -> Option<Vec<(OsString, FileType)>> {
        let Ok(entries) = fs::read_dir(&frame.path) else {
            self.output.unreadable += 1;
            self.mark_chain_incomplete(frame.attribution);
            return None;
        };
        let mut listing = Vec::new();
        for entry in entries {
            if self.count_entry() {
                return None;
            }
            if let Ok(entry) = entry.and_then(|entry| Ok((entry.file_name(), entry.file_type()?))) {
                listing.push(entry);
            } else {
                self.output.unreadable += 1;
                self.mark_chain_incomplete(frame.attribution);
            }
        }
        Some(listing)
    }

    #[allow(clippy::too_many_lines)]
    fn discover(&mut self, frame: &Frame) {
        let Some(listing) = self.read_listing(frame) else {
            return;
        };
        let mut markers = SiblingMarkers::default();
        let mut git_dir = false;
        let mut git_file = false;
        let mut git_any = false;
        for (name, file_type) in &listing {
            markers.observe(
                name,
                file_type.is_file(),
                file_type.is_dir(),
                &self.settings.workspace_markers,
            );
            if name == ".git" {
                git_any = true;
                git_dir |= file_type.is_dir();
                git_file |= file_type.is_file();
            }
        }
        let mut attribution = frame.attribution;
        let mut repo_scope = frame.repo_scope;
        if git_any {
            // Git accepts a `.git` of any type (including a symlink); the
            // scan asks Git whether it is valid and otherwise falls back to
            // the enclosing scope.
            self.output.nested_git.push(NestedGit {
                path: frame.path.clone(),
                enclosing: frame.repo_scope,
            });
            repo_scope = RepoScope::Nested(self.output.nested_git.len() - 1);
            // Every enclosing candidate now holds another repository.
            self.mark_chain_nested_repo(attribution);
        }
        let is_workspace = markers.is_workspace_root();
        let linked = git_file && is_linked_worktree_git_file(&frame.path.join(".git"));
        if linked && frame.depth == 0 {
            self.output.worktree_roots.push(frame.path.clone());
        } else if linked && !self.contains_excluded(&frame.path) {
            if let Some(index) = self.create_candidate(
                frame.path.clone(),
                StorageClass::GitWorktree,
                attribution,
                None,
                None,
            ) {
                attribution = Some(index);
            }
        } else if frame.parent_is_worktrees
            && !git_dir
            && !linked
            && !is_workspace
            && !self.contains_excluded(&frame.path)
        {
            if let Some(index) = self.create_candidate(
                frame.path.clone(),
                StorageClass::Unknown,
                attribution,
                None,
                None,
            ) {
                self.output.candidates[index].unknown_detail = Some(if git_file {
                    "a .worktrees entry that is not a registered Git worktree"
                } else {
                    "a .worktrees entry that is not a marked workspace or Git worktree"
                });
                self.size_listing(frame, &listing, Some(index));
                return;
            }
        }
        let in_worktrees_dir = frame
            .path
            .file_name()
            .is_some_and(|name| name == ".worktrees");
        let workspace = is_workspace.then(|| {
            self.output.workspaces.push(MarkedWorkspace {
                path: frame.path.clone(),
                packages: Vec::new(),
                listing_complete: false,
            });
            self.output.workspaces.len() - 1
        });

        for (name, file_type) in listing {
            if self.tick() {
                // The rest of this listing is never looked at.
                self.mark_chain_incomplete(attribution);
                return;
            }
            let child = frame.path.join(&name);
            if !file_type.is_dir() || name == ".git" {
                // Files, symlinks, and `.git` are never followed; inside a
                // candidate their own blocks still count.
                if attribution.is_some()
                    && let Ok(metadata) = fs::symlink_metadata(&child)
                {
                    self.add_entry(attribution, &name, &metadata);
                }
                continue;
            }
            if self.is_excluded(&child) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&child) else {
                self.output.unreadable += 1;
                self.mark_chain_incomplete(attribution);
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            if metadata.dev() != frame.dev {
                self.mark_chain_crosses_device(attribution);
                continue;
            }
            if let Some(workspace) = workspace {
                if name == "src" {
                    self.list_workspace_packages(workspace, &child);
                    continue;
                }
                if let Some(class) = workspace_child_class(&name) {
                    // Workspace build/env sit outside the nested repositories;
                    // logs still get the generated-output Git rule.
                    let class_repo_scope =
                        (class == StorageClass::WorkspaceLogs).then_some(repo_scope);
                    self.push_candidate(
                        frame,
                        child,
                        &name,
                        &metadata,
                        class,
                        attribution,
                        class_repo_scope,
                        Some(workspace),
                    );
                    continue;
                }
            }
            if name
                .as_encoded_bytes()
                .starts_with(RECLAIM_PREFIX.as_bytes())
            {
                if let Some(index) = self.push_candidate(
                    frame,
                    child,
                    &name,
                    &metadata,
                    StorageClass::Unknown,
                    attribution,
                    None,
                    None,
                ) {
                    self.output.candidates[index].unknown_detail =
                        Some("an orphan cleanup quarantine directory");
                }
                continue;
            }
            if let Some(class) = generated_class(&name, markers) {
                self.push_candidate(
                    frame,
                    child,
                    &name,
                    &metadata,
                    class,
                    attribution,
                    Some(repo_scope),
                    None,
                );
                continue;
            }
            self.add_entry(attribution, &name, &metadata);
            if frame.depth + 1 > self.settings.max_discovery_depth {
                if attribution.is_some() {
                    self.push(Frame {
                        path: child,
                        depth: frame.depth + 1,
                        size_depth: 1,
                        mode: Mode::Size,
                        attribution,
                        repo_scope,
                        dev: frame.dev,
                        parent_is_worktrees: false,
                    });
                } else {
                    self.output.depth_limited += 1;
                }
                continue;
            }
            self.push(Frame {
                path: child,
                depth: frame.depth + 1,
                size_depth: 0,
                mode: Mode::Discover,
                attribution,
                repo_scope,
                dev: frame.dev,
                parent_is_worktrees: in_worktrees_dir,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_candidate(
        &mut self,
        frame: &Frame,
        path: PathBuf,
        name: &OsStr,
        metadata: &Metadata,
        class: StorageClass,
        parent: Option<usize>,
        repo_scope: Option<RepoScope>,
        marked_workspace: Option<usize>,
    ) -> Option<usize> {
        if self.contains_excluded(&path) {
            return None;
        }
        let index =
            self.create_candidate(path.clone(), class, parent, repo_scope, marked_workspace)?;
        self.add_entry(Some(index), name, metadata);
        self.push(Frame {
            path,
            depth: frame.depth + 1,
            size_depth: 1,
            mode: Mode::Size,
            attribution: Some(index),
            repo_scope: frame.repo_scope,
            dev: frame.dev,
            parent_is_worktrees: false,
        });
        Some(index)
    }

    /// Size an already-read listing (the directory became a candidate).
    fn size_listing(
        &mut self,
        frame: &Frame,
        listing: &[(OsString, FileType)],
        attribution: Option<usize>,
    ) {
        for (name, _) in listing {
            if self.tick() {
                self.mark_chain_incomplete(attribution);
                return;
            }
            let child = frame.path.join(name);
            if let Ok(metadata) = fs::symlink_metadata(&child) {
                self.size_entry(frame, attribution, child, name, &metadata, 1);
            } else {
                self.output.unreadable += 1;
                self.mark_chain_incomplete(attribution);
            }
        }
    }

    fn size(&mut self, frame: &Frame) {
        let Ok(entries) = fs::read_dir(&frame.path) else {
            self.output.unreadable += 1;
            self.mark_chain_incomplete(frame.attribution);
            return;
        };
        for entry in entries {
            if self.count_entry() {
                return;
            }
            let Ok(entry) = entry else {
                self.output.unreadable += 1;
                self.mark_chain_incomplete(frame.attribution);
                continue;
            };
            // `DirEntry::metadata` does not traverse symlinks.
            match entry.metadata() {
                Ok(metadata) => self.size_entry(
                    frame,
                    frame.attribution,
                    entry.path(),
                    &entry.file_name(),
                    &metadata,
                    frame.size_depth + 1,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    self.output.unreadable += 1;
                    self.mark_chain_incomplete(frame.attribution);
                }
            }
        }
    }

    fn size_entry(
        &mut self,
        frame: &Frame,
        attribution: Option<usize>,
        path: PathBuf,
        name: &OsStr,
        metadata: &Metadata,
        size_depth: usize,
    ) {
        self.add_entry(attribution, name, metadata);
        if name == ".git"
            && let Some(index) = attribution
        {
            self.output.candidates[index].contains_git = true;
            self.mark_chain_nested_repo(attribution);
        }
        if !metadata.is_dir() || name == ".git" {
            return;
        }
        if metadata.dev() != frame.dev {
            self.mark_chain_crosses_device(attribution);
            return;
        }
        if size_depth > self.settings.max_size_depth {
            self.mark_chain_incomplete(attribution);
            return;
        }
        self.push(Frame {
            path,
            depth: frame.depth + 1,
            size_depth,
            mode: Mode::Size,
            attribution,
            repo_scope: frame.repo_scope,
            dev: frame.dev,
            parent_is_worktrees: false,
        });
    }

    /// List `src/<Pkg>` directories without descending into them.
    fn list_workspace_packages(&mut self, workspace: usize, src: &Path) {
        let Ok(entries) = fs::read_dir(src) else {
            self.output.unreadable += 1;
            return;
        };
        let mut packages = Vec::new();
        let mut complete = true;
        for entry in entries {
            if self.count_entry() {
                complete = false;
                break;
            }
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                complete = false;
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            if packages.len() >= self.settings.max_workspace_packages {
                complete = false;
                break;
            }
            let path = entry.path();
            packages.push(WorkspacePackage {
                name: entry.file_name().to_string_lossy().into_owned(),
                has_git: fs::symlink_metadata(path.join(".git")).is_ok(),
                path,
            });
        }
        packages.sort_by(|left, right| left.path.cmp(&right.path));
        let record = &mut self.output.workspaces[workspace];
        record.packages = packages;
        record.listing_complete = complete;
    }
}

fn newest(current: Option<SystemTime>, candidate: Option<SystemTime>) -> Option<SystemTime> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (current, candidate) => current.or(candidate),
    }
}

/// Whether `root` lies inside `<ancestor>/src` of a marked workspace.
fn inside_workspace_src(root: &Path, ancestor: &Path, workspace_markers: &[String]) -> bool {
    let src = ancestor.join("src");
    contains(&src, root)
        && workspace_markers.iter().any(|marker| {
            fs::symlink_metadata(ancestor.join(marker)).is_ok_and(|meta| meta.is_file())
        })
        && fs::symlink_metadata(&src).is_ok_and(|meta| meta.is_dir())
}

/// A linked worktree's `.git` file points at `<common>/worktrees/<name>`;
/// a submodule's points at `<common>/modules/<name>`.
fn is_linked_worktree_git_file(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut contents = String::new();
    if file
        .take(MAX_GIT_FILE_BYTES)
        .read_to_string(&mut contents)
        .is_err()
    {
        return false;
    }
    let Some(gitdir) = contents
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir: "))
    else {
        return false;
    };
    Path::new(gitdir.trim())
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "worktrees")
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::symlink, time::Duration};

    use tempfile::TempDir;

    use super::*;

    fn settings(root: &Path) -> WalkSettings {
        WalkSettings {
            roots: vec![fs::canonicalize(root).unwrap()],
            excluded: Vec::new(),
            max_entries: 100_000,
            deadline: Instant::now() + Duration::from_secs(60),
            max_discovery_depth: 12,
            max_size_depth: 64,
            max_candidates: 1_000,
            max_workspace_packages: 64,
            workspace_markers: vec!["workspace.toml".to_owned()],
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7_u8; bytes]).unwrap();
    }

    fn find<'a>(output: &'a WalkOutput, suffix: &str) -> Option<&'a WalkCandidate> {
        output
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with(suffix))
    }

    #[test]
    fn finds_generated_classes_by_name_and_marker_without_following_symlinks() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("app/Cargo.toml"), 10);
        write(&root.join("app/target/debug/big"), 64 * 1024);
        write(&root.join("web/package.json"), 10);
        write(&root.join("web/node_modules/x/index.js"), 100);
        write(&root.join("web/vite.config.ts"), 10);
        write(&root.join("web/dist/index.html"), 100);
        write(&root.join("plain/target/file"), 100);
        write(&root.join("outside/secret/huge"), 256 * 1024);
        symlink(root.join("outside/secret"), root.join("app/target/link")).unwrap();
        symlink(root.join("outside"), root.join("plain/linked")).unwrap();
        write(&root.join("app/target/hard"), 8 * 1024);
        fs::hard_link(root.join("app/target/hard"), root.join("outside/hard-copy")).unwrap();

        let output = walk(&settings(root));

        let target = find(&output, "app/target").expect("rust target");
        assert_eq!(target.class, StorageClass::RustTarget);
        assert!(target.path_verified);
        assert!(target.bytes >= 64 * 1024, "{}", target.bytes);
        assert!(
            target.bytes < 200 * 1024,
            "the symlinked 256 KiB file must not be counted: {}",
            target.bytes
        );
        assert!(target.shared_bytes >= 8 * 1024, "hard link is shared");
        assert!(target.complete && !target.contains_git);
        assert_eq!(
            find(&output, "web/node_modules").unwrap().class,
            StorageClass::NodeModules
        );
        assert_eq!(
            find(&output, "web/dist").unwrap().class,
            StorageClass::ViteDist
        );
        assert!(
            find(&output, "plain/target").is_none(),
            "no Cargo.toml marker"
        );
        let canonical = fs::canonicalize(root).unwrap();
        assert!(
            output
                .candidates
                .iter()
                .all(|candidate| !candidate.path.starts_with(canonical.join("plain/linked"))),
            "the symlinked directory is never followed"
        );
    }

    #[test]
    fn a_hard_linked_file_counts_once_and_is_reclaimable_when_every_link_is_inside() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let block = |path: &Path| fs::symlink_metadata(path).unwrap().blocks() * 512;
        write(&root.join("app/Cargo.toml"), 10);
        write(&root.join("app/target/debug/plain"), 64 * 1024);
        write(&root.join("app/target/debug/inside"), 128 * 1024);
        fs::create_dir_all(root.join("app/target/release")).unwrap();
        fs::hard_link(
            root.join("app/target/debug/inside"),
            root.join("app/target/release/inside"),
        )
        .unwrap();
        write(&root.join("app/target/debug/escapes"), 32 * 1024);
        fs::hard_link(
            root.join("app/target/debug/escapes"),
            root.join("app/escapes-copy"),
        )
        .unwrap();

        let output = walk(&settings(root));

        let target = find(&output, "app/target").unwrap();
        let plain = block(&root.join("app/target/debug/plain"));
        let inside = block(&root.join("app/target/debug/inside"));
        let escapes = block(&root.join("app/target/debug/escapes"));
        let directories = ["app/target", "app/target/debug", "app/target/release"]
            .iter()
            .map(|dir| block(&root.join(dir)))
            .sum::<u64>();
        assert_eq!(
            target.bytes,
            plain + inside + directories,
            "both links inside: counted once, reclaimable"
        );
        assert_eq!(
            target.shared_bytes, escapes,
            "one link outside: shared, once"
        );
    }

    #[test]
    fn discovery_never_follows_a_symlinked_directory_out_of_the_root() {
        let temp = TempDir::new().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let root = base.join("root");
        let outside = base.join("outside");
        write(&outside.join("Cargo.toml"), 10);
        write(&outside.join("target/debug/app"), 4096);
        write(&root.join("plain/README"), 10);
        symlink(&outside, root.join("plain/linked")).unwrap();
        symlink(&outside, root.join("linked-root")).unwrap();
        write(&root.join("real/Cargo.toml"), 10);
        write(&root.join("real/target/debug/app"), 4096);

        let output = walk(&settings(&root));

        assert!(
            find(&output, "real/target").is_some(),
            "control: a real candidate is found"
        );
        assert!(
            output.candidates.iter().all(|candidate| {
                !candidate.path.starts_with(&outside)
                    && !candidate.path.starts_with(root.join("plain/linked"))
                    && !candidate.path.starts_with(root.join("linked-root"))
            }),
            "{:#?}",
            output.candidates
        );
    }

    #[test]
    fn sizes_count_allocated_blocks_not_apparent_length() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("app/Cargo.toml"), 10);
        fs::create_dir_all(root.join("app/target")).unwrap();
        fs::File::create(root.join("app/target/sparse"))
            .unwrap()
            .set_len(1 << 30)
            .unwrap();

        let output = walk(&settings(root));

        let target = find(&output, "app/target").unwrap();
        assert!(
            target.bytes < 16 * 1024 * 1024,
            "a sparse 1 GiB file is reported by its allocated blocks: {}",
            target.bytes
        );
    }

    #[test]
    fn nothing_inside_an_excluded_directory_is_a_candidate() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("data/artifacts/Cargo.toml"), 10);
        write(&root.join("data/artifacts/target/debug/app"), 100);
        write(&root.join("app/Cargo.toml"), 10);
        write(&root.join("app/target/debug/app"), 100);
        let mut excluded = settings(root);
        excluded.excluded = vec![fs::canonicalize(root.join("data/artifacts")).unwrap()];

        let output = walk(&excluded);

        assert!(find(&output, "app/target").is_some(), "control");
        assert!(
            find(&output, "data/artifacts/target").is_none(),
            "{:#?}",
            output.candidates
        );
        assert!(
            find(&walk(&settings(root)), "data/artifacts/target").is_some(),
            "control: found without the exclusion"
        );
    }

    #[test]
    fn marked_workspace_protects_src_and_skips_package_build_symlinks() {
        let temp = TempDir::new().unwrap();
        let ws = temp.path().join(".worktrees/ws");
        write(&ws.join("workspace.toml"), 10);
        write(&ws.join("build/Pkg/out.jar"), 32 * 1024);
        write(&ws.join("env/runtime/lib.so"), 16 * 1024);
        write(&ws.join(".build-logs/last.log"), 100);
        write(&ws.join("src/Pkg/Cargo.toml"), 10);
        write(&ws.join("src/Pkg/target/debug/x"), 100);
        fs::create_dir_all(ws.join("src/Pkg/.git")).unwrap();
        fs::create_dir_all(ws.join("src/Loose")).unwrap();
        symlink(ws.join("build/Pkg"), ws.join("src/Pkg/build")).unwrap();
        write(&temp.path().join(".worktrees/stray/notes.txt"), 4096);

        let output = walk(&settings(temp.path()));

        assert_eq!(
            find(&output, "ws/build").unwrap().class,
            StorageClass::WorkspaceBuild
        );
        assert_eq!(
            find(&output, "ws/env").unwrap().class,
            StorageClass::WorkspaceEnv
        );
        assert_eq!(
            find(&output, "ws/.build-logs").unwrap().class,
            StorageClass::WorkspaceLogs
        );
        assert!(find(&output, "ws/build").unwrap().repo_scope.is_none());
        assert!(
            output
                .candidates
                .iter()
                .all(|candidate| !candidate.path.to_string_lossy().contains("/src/")),
            "nothing inside a workspace src/ is a candidate: {:?}",
            output.candidates
        );
        assert_eq!(output.workspaces.len(), 1);
        let packages = &output.workspaces[0].packages;
        assert!(output.workspaces[0].listing_complete);
        assert_eq!(
            packages
                .iter()
                .map(|package| (package.name.as_str(), package.has_git))
                .collect::<Vec<_>>(),
            [("Loose", false), ("Pkg", true)]
        );
        let stray = find(&output, ".worktrees/stray").unwrap();
        assert_eq!(stray.class, StorageClass::Unknown);
        assert!(stray.bytes >= 4096);
        assert!(
            find(&output, ".worktrees/ws").is_none(),
            "the workspace itself is never a candidate"
        );

        let mut unmarked = settings(temp.path());
        unmarked.workspace_markers = Vec::new();
        let output = walk(&unmarked);
        assert!(
            output.workspaces.is_empty(),
            "no configured marker: no workspace"
        );
        assert!(
            output
                .candidates
                .iter()
                .all(|candidate| !candidate.class.is_workspace()),
            "no configured marker: no workspace classes: {:?}",
            output.candidates
        );
    }

    #[test]
    fn budgets_truncate_candidates_and_exclusions_hide_them() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("app/Cargo.toml"), 10);
        for index in 0..50 {
            write(&root.join(format!("app/target/debug/f{index}")), 10);
        }
        let mut limited = settings(root);
        limited.max_entries = 20;
        let output = walk(&limited);
        assert!(
            output
                .truncation
                .contains(&StorageTruncationReason::EntryBudget)
        );
        assert!(!find(&output, "app/target").unwrap().complete);

        let mut excluded = settings(root);
        excluded.excluded = vec![fs::canonicalize(root.join("app/target/debug/f3")).unwrap()];
        let output = walk(&excluded);
        assert!(find(&output, "app/target").is_none());

        let mut expired = settings(root);
        expired.deadline = Instant::now();
        let output = walk(&expired);
        assert!(
            output
                .truncation
                .contains(&StorageTruncationReason::TimeBudget)
        );
    }

    #[test]
    fn discovery_finishes_before_sizing_so_budgets_never_hide_siblings() {
        let temp = TempDir::new().unwrap();
        let worktrees = temp.path().join(".worktrees");
        for ws in ["ws1", "ws2", "ws3", "ws4"] {
            let ws = worktrees.join(ws);
            write(&ws.join("workspace.toml"), 10);
            fs::create_dir_all(ws.join("src")).unwrap();
            write(&ws.join("build/out.jar"), 10);
            for index in 0..100 {
                write(&ws.join(format!("env/f{index}")), 10);
            }
        }
        let mut limited = settings(temp.path());
        limited.max_entries = 150;
        let output = walk(&limited);
        assert!(
            output
                .truncation
                .contains(&StorageTruncationReason::EntryBudget)
        );
        assert_eq!(output.workspaces.len(), 4, "{:?}", output.workspaces);
        for ws in ["ws1", "ws2", "ws3", "ws4"] {
            for class in ["env", "build"] {
                assert!(
                    find(&output, &format!("{ws}/{class}")).is_some(),
                    "{ws}/{class} missing: {:?}",
                    output.candidates
                );
            }
        }
        assert!(
            output
                .candidates
                .iter()
                .any(|candidate| !candidate.complete),
            "the unsized candidates are truncated"
        );
        assert!(output.undiscovered.is_empty() && output.undiscovered_count == 0);

        // When discovery itself runs out, the unsearched directories are named.
        let mut tiny = settings(temp.path());
        tiny.max_entries = 3;
        let output = walk(&tiny);
        assert!(output.undiscovered_count > 0);
        assert!(
            output
                .undiscovered
                .iter()
                .any(|path| path.starts_with(fs::canonicalize(&worktrees).unwrap())),
            "{:?}",
            output.undiscovered
        );
    }

    #[test]
    fn cancel_and_the_deadline_stop_the_walk_between_entries() {
        let temp = TempDir::new().unwrap();
        write(&temp.path().join("app/Cargo.toml"), 10);
        write(&temp.path().join("app/target/debug/x"), 10);
        let cancelled = settings(temp.path());
        cancelled.cancel.store(true, Ordering::Relaxed);
        let output = walk(&cancelled);
        assert!(output.cancelled);
        assert!(output.candidates.is_empty());

        // The per-entry `lstat` loops check the deadline too.
        let mut expired = settings(temp.path());
        expired.deadline = Instant::now();
        let mut walker = Walker {
            settings: &expired,
            output: WalkOutput::default(),
            discover_stack: Vec::new(),
            size_stack: Vec::new(),
            stat_ops: 0,
            stopped: false,
            hard_links: HashMap::new(),
        };
        let stopped = (0..DEADLINE_CHECK_INTERVAL).any(|_| walker.tick());
        assert!(stopped && walker.stopped);
        assert!(
            walker
                .output
                .truncation
                .contains(&StorageTruncationReason::TimeBudget)
        );
    }

    #[test]
    fn candidates_that_do_not_canonicalize_to_themselves_are_unverified() {
        let temp = TempDir::new().unwrap();
        write(&temp.path().join("real/app/Cargo.toml"), 10);
        write(&temp.path().join("real/app/target/debug/x"), 10);
        let alias = temp.path().join("alias");
        symlink(temp.path().join("real"), &alias).unwrap();
        let mut aliased = settings(temp.path());
        aliased.roots = vec![fs::canonicalize(temp.path()).unwrap().join("alias/app")];
        let output = walk(&aliased);
        let target = find(&output, "alias/app/target").expect("target under the alias");
        assert!(!target.path_verified);
    }

    #[test]
    fn git_inside_a_generated_directory_is_flagged() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("web/package.json"), 10);
        write(&root.join("web/node_modules/dep/.git/HEAD"), 10);
        let output = walk(&settings(root));
        assert!(find(&output, "web/node_modules").unwrap().contains_git);
    }

    #[test]
    fn dot_git_entries_are_recorded_for_git_to_validate_never_trusted() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        // A `.git` above the root is never looked at by the walk.
        fs::create_dir_all(root.join(".git")).unwrap();
        let root = &fs::canonicalize(root).unwrap().join("root");
        write(&root.join("app/Cargo.toml"), 10);
        write(&root.join("app/target/out"), 10);
        write(&root.join("web/.git"), 10);
        write(&root.join("web/package.json"), 10);
        write(&root.join("web/node_modules/dep.js"), 10);
        write(&root.join("web/sub/.git"), 10);
        write(&root.join("web/sub/Cargo.toml"), 10);
        write(&root.join("web/sub/target/out"), 10);
        let output = walk(&settings(root));
        assert_eq!(
            output.nested_git,
            [
                NestedGit {
                    path: root.join("web"),
                    enclosing: RepoScope::Root(0),
                },
                NestedGit {
                    path: root.join("web/sub"),
                    enclosing: RepoScope::Nested(0),
                },
            ]
        );
        let scope = |suffix| find(&output, suffix).unwrap().repo_scope;
        assert_eq!(scope("app/target"), Some(RepoScope::Root(0)));
        assert_eq!(scope("web/node_modules"), Some(RepoScope::Nested(0)));
        assert_eq!(scope("web/sub/target"), Some(RepoScope::Nested(1)));
    }
}

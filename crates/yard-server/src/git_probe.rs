//! Read-only Git probes for the storage scan.
//!
//! Every call is argv only (never a shell), has a timeout, is killed when
//! dropped, and reads bounded output, following
//! `yard-herdr/src/discovery.rs`. Git runs with fsmonitor and the untracked
//! cache disabled, without system config, without optional locks, never
//! prompts, and in the C locale. A "dubious ownership" refusal is reported
//! separately so the candidate can be blocked.

use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{Instant, timeout},
};

use crate::storage_classify::{GeneratedGitFacts, GitCheck, PackageGitFacts, WorktreeGitFacts};

const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_SMALL_STDOUT_BYTES: usize = 64 * 1024;
const MAX_STATUS_STDOUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_WORKTREE_LIST_BYTES: usize = 1024 * 1024;

/// Environment variables that could redirect `git -C` to another repository.
/// `GIT_CONFIG_PARAMETERS` and `GIT_CONFIG_COUNT` could inject configuration.
const REDIRECTING_GIT_ENV: [&str; 10] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitError {
    Spawn(String),
    Timeout,
    DeadlineExceeded,
    OutputTooLarge,
    DubiousOwnership,
    Failed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "git could not start: {error}"),
            Self::Timeout => formatter.write_str("git timed out"),
            Self::DeadlineExceeded => formatter.write_str("scan time budget exhausted"),
            Self::OutputTooLarge => formatter.write_str("git output too large"),
            Self::DubiousOwnership => formatter.write_str("dubious ownership"),
            Self::Failed(error) => formatter.write_str(error),
        }
    }
}

impl<T> From<GitError> for GitCheck<T> {
    fn from(error: GitError) -> Self {
        match error {
            GitError::DubiousOwnership => Self::DubiousOwnership,
            error => Self::Failed(error.to_string()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct GitOutput {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
}

/// Runs read-only Git commands under a per-command timeout and a shared
/// scan deadline.
#[derive(Debug, Clone)]
pub(crate) struct GitRunner {
    binary: OsString,
    command_timeout: Duration,
    deadline: Instant,
    env: Vec<(OsString, OsString)>,
}

impl GitRunner {
    pub(crate) fn new(binary: OsString, command_timeout: Duration, deadline: Instant) -> Self {
        Self {
            binary,
            command_timeout,
            deadline,
            env: Vec::new(),
        }
    }

    /// Extra environment for every command (tests isolate global config).
    pub(crate) fn with_env(mut self, env: Vec<(OsString, OsString)>) -> Self {
        self.env = env;
        self
    }

    /// Run `git -C <dir> <args>` and accept only the listed exit codes.
    /// Git must find the repository at `dir` itself.
    pub(crate) async fn run(
        &self,
        dir: &Path,
        args: &[&OsStr],
        max_stdout: usize,
        accepted_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        self.run_with(dir, args, max_stdout, accepted_codes, Discovery::Pinned)
            .await
    }

    async fn run_with(
        &self,
        dir: &Path,
        args: &[&OsStr],
        max_stdout: usize,
        accepted_codes: &[i32],
        discovery: Discovery<'_>,
    ) -> Result<GitOutput, GitError> {
        let now = Instant::now();
        if now >= self.deadline {
            return Err(GitError::DeadlineExceeded);
        }
        let limit = self.command_timeout.min(self.deadline - now);
        let mut command = Command::new(&self.binary);
        command
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-c")
            .arg("core.untrackedCache=false")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in REDIRECTING_GIT_ENV {
            command.env_remove(variable);
        }
        match discovery {
            // Pin repository discovery to `dir`: when its `.git` is missing or
            // invalid, Git must fail instead of reporting an enclosing
            // repository.
            Discovery::Pinned => {
                if let Some(parent) = dir.parent() {
                    command.env("GIT_CEILING_DIRECTORIES", parent);
                }
            }
            Discovery::Upward([]) => {
                command.env_remove("GIT_CEILING_DIRECTORIES");
            }
            Discovery::Upward(ceilings) => {
                let joined = std::env::join_paths(ceilings).map_err(|_| {
                    GitError::Failed("a Git ceiling directory contains ':'".to_owned())
                })?;
                command.env("GIT_CEILING_DIRECTORIES", joined);
            }
        }
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
        let mut child = command
            .spawn()
            .map_err(|error| GitError::Spawn(error.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| GitError::Spawn("piped git stdout was not available".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| GitError::Spawn("piped git stderr was not available".to_owned()))?;
        let operation = async move {
            let (stdout, stderr, status) = tokio::try_join!(
                read_bounded(stdout, max_stdout),
                read_bounded(stderr, MAX_STDERR_BYTES),
                async { child.wait().await.map_err(RunError::Io) }
            )?;
            Ok::<_, RunError>((status, stdout, stderr))
        };
        let (status, stdout, stderr) = timeout(limit, operation)
            .await
            .map_err(|_| {
                if Instant::now() >= self.deadline {
                    GitError::DeadlineExceeded
                } else {
                    GitError::Timeout
                }
            })?
            .map_err(|error| match error {
                RunError::Io(error) => GitError::Spawn(error.to_string()),
                RunError::TooLarge => GitError::OutputTooLarge,
            })?;
        let stderr = String::from_utf8_lossy(&stderr);
        if is_dubious_ownership(&stderr) {
            return Err(GitError::DubiousOwnership);
        }
        let code = status.code();
        if code.is_some_and(|code| accepted_codes.contains(&code)) {
            return Ok(GitOutput { code, stdout });
        }
        let message = stderr.trim();
        Err(GitError::Failed(if message.is_empty() {
            format!("git exited with {status}")
        } else {
            first_line(message)
        }))
    }
}

/// Where Git may look for the repository of `git -C <dir>`.
#[derive(Debug, Clone, Copy)]
enum Discovery<'a> {
    /// Only at `dir` itself.
    Pinned,
    /// At `dir` and its ancestors, never entering a ceiling directory.
    Upward(&'a [PathBuf]),
}

enum RunError {
    Io(io::Error),
    TooLarge,
}

async fn read_bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, RunError> {
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut reader = reader.take(take_limit);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.map_err(RunError::Io)?;
    if bytes.len() > limit {
        return Err(RunError::TooLarge);
    }
    Ok(bytes)
}

fn first_line(message: &str) -> String {
    let line = message.lines().next().unwrap_or(message).trim();
    let mut bounded: String = line.chars().take(240).collect();
    if bounded.len() < line.len() {
        bounded.push('…');
    }
    bounded
}

/// Git's refusal of a repository owned by another user.
pub(crate) fn is_dubious_ownership(stderr: &str) -> bool {
    stderr.contains("detected dubious ownership")
}

/// One entry of `git worktree list --porcelain -z`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Parse `git worktree list --porcelain -z`. The first entry is the main
/// worktree. Attribute lines end in NUL; an empty line ends a record.
pub(crate) fn parse_worktree_list(output: &[u8]) -> Result<Vec<WorktreeEntry>, String> {
    use std::os::unix::ffi::OsStrExt;

    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for token in output.split(|byte| *byte == 0) {
        if token.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        let (key, value) = match token.iter().position(|byte| *byte == b' ') {
            Some(index) => (&token[..index], Some(&token[index + 1..])),
            None => (token, None),
        };
        if key == b"worktree" {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let path = value.ok_or("worktree entry without a path")?;
            current = Some(WorktreeEntry {
                path: PathBuf::from(OsStr::from_bytes(path)),
                ..WorktreeEntry::default()
            });
            continue;
        }
        let entry = current
            .as_mut()
            .ok_or("worktree attribute before a worktree line")?;
        match key {
            b"HEAD" => entry.head = value.map(|value| String::from_utf8_lossy(value).into_owned()),
            b"branch" => {
                entry.branch = value.map(|value| String::from_utf8_lossy(value).into_owned());
            }
            b"bare" => entry.bare = true,
            b"detached" => entry.detached = true,
            b"locked" => entry.locked = true,
            b"prunable" => entry.prunable = true,
            // Unknown attributes from newer Git versions are ignored; the
            // safety-relevant ones above are all recognized.
            _ => {}
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    Ok(entries)
}

/// Parsed `git status --porcelain=v2 --branch -z`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatusSummary {
    pub oid: Option<String>,
    /// `None` when `HEAD` is detached.
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub changed: u64,
    pub untracked: u64,
    /// Ignored paths relative to the work tree (directories end in `/`).
    pub ignored: Vec<PathBuf>,
}

/// Parse `git status --porcelain=v2 --branch -z`.
pub(crate) fn parse_status(output: &[u8]) -> Result<StatusSummary, String> {
    use std::os::unix::ffi::OsStrExt;

    let mut summary = StatusSummary::default();
    let mut tokens = output.split(|byte| *byte == 0);
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        if let Some(header) = token.strip_prefix(b"# ") {
            let text = String::from_utf8_lossy(header);
            let (key, value) = text.split_once(' ').unwrap_or((&text, ""));
            match key {
                "branch.oid" => summary.oid = Some(value.to_owned()),
                "branch.head" if value == "(detached)" => summary.detached = true,
                "branch.head" => summary.branch = Some(value.to_owned()),
                "branch.upstream" => summary.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    let mut counts = value.split(' ');
                    summary.ahead = counts
                        .next()
                        .and_then(|ahead| ahead.strip_prefix('+'))
                        .and_then(|ahead| ahead.parse().ok());
                    summary.behind = counts
                        .next()
                        .and_then(|behind| behind.strip_prefix('-'))
                        .and_then(|behind| behind.parse().ok());
                    if summary.ahead.is_none() || summary.behind.is_none() {
                        return Err(format!("unrecognized branch.ab header '{value}'"));
                    }
                }
                _ => {}
            }
            continue;
        }
        match token.first() {
            Some(b'1' | b'u') => summary.changed += 1,
            Some(b'2') => {
                summary.changed += 1;
                // A rename or copy record is followed by its original path.
                tokens
                    .next()
                    .ok_or("rename record without an original path")?;
            }
            Some(b'?') => summary.untracked += 1,
            Some(b'!') => {
                let path = token.get(2..).ok_or("ignored record without a path")?;
                summary.ignored.push(PathBuf::from(OsStr::from_bytes(path)));
            }
            _ => {
                return Err(format!(
                    "unrecognized status record '{}'",
                    String::from_utf8_lossy(&token[..token.len().min(40)])
                ));
            }
        }
    }
    Ok(summary)
}

fn os(value: &str) -> &OsStr {
    OsStr::new(value)
}

/// `git worktree list --porcelain -z` for the repository at `dir`.
pub(crate) async fn list_worktrees(
    runner: &GitRunner,
    dir: &Path,
) -> Result<Vec<WorktreeEntry>, GitError> {
    let output = runner
        .run(
            dir,
            &[os("worktree"), os("list"), os("--porcelain"), os("-z")],
            MAX_WORKTREE_LIST_BYTES,
            &[0],
        )
        .await?;
    parse_worktree_list(&output.stdout).map_err(GitError::Failed)
}

async fn status(
    runner: &GitRunner,
    dir: &Path,
    untracked: &str,
    ignored: bool,
) -> Result<StatusSummary, GitError> {
    let untracked = format!("--untracked-files={untracked}");
    let mut args = vec![
        os("status"),
        os("--porcelain=v2"),
        os("--branch"),
        OsStr::new(untracked.as_str()),
        // A repository's `.gitmodules` may set `ignore=all`; report dirty
        // submodules regardless.
        os("--ignore-submodules=none"),
        os("-z"),
    ];
    if ignored {
        args.push(os("--ignored=matching"));
    }
    let output = runner
        .run(dir, &args, MAX_STATUS_STDOUT_BYTES, &[0])
        .await?;
    parse_status(&output.stdout).map_err(GitError::Failed)
}

/// Exit code 0 means yes, 1 means no.
async fn yes_no(runner: &GitRunner, dir: &Path, args: &[&OsStr]) -> Result<bool, GitError> {
    let output = runner
        .run(dir, args, MAX_SMALL_STDOUT_BYTES, &[0, 1])
        .await?;
    Ok(output.code == Some(0))
}

/// The work tree Git itself reports for `start`, searching upward but never
/// entering one of `ceilings` (`GIT_CEILING_DIRECTORIES`), so gitfiles,
/// linked worktrees, `safe.directory` and ceilings are honoured. `Ok(None)`
/// means no repository encloses `start`: a stale or broken `.git` that Git
/// does not accept is ignored rather than treated as a repository. Every
/// other failure (timeout, dubious ownership, Git missing) is an error.
pub(crate) async fn discover_work_tree(
    runner: &GitRunner,
    start: &Path,
    ceilings: &[PathBuf],
) -> Result<Option<PathBuf>, GitError> {
    // Git resolves symlinks in the cwd and in every ceiling, so the ceiling
    // checks below compare canonical paths only.
    let start = canonical(start);
    let ceilings = canonical_ceilings(ceilings);
    let ceilings = ceilings.as_slice();
    let mut from = start.clone();
    loop {
        let message = match runner
            .run_with(
                &from,
                &[os("rev-parse"), os("--show-toplevel")],
                MAX_SMALL_STDOUT_BYTES,
                &[0],
                Discovery::Upward(ceilings),
            )
            .await
        {
            Ok(output) => return parse_toplevel(&output.stdout).map(Some),
            Err(GitError::Failed(message)) if is_not_a_repository(&message) => message,
            Err(error) => return Err(error),
        };
        if message.starts_with("fatal: not a git repository (or any") {
            // Git searched every allowed ancestor.
            return Ok(None);
        }
        // Git stops at the first `.git` file it cannot read. A repository
        // above that file still encloses `start` (its tracked files must stay
        // protected), so the search resumes above it.
        let Some(broken) = nearest_git_file(&from, ceilings) else {
            return Err(GitError::Failed(message));
        };
        match broken.parent() {
            Some(parent) if !hits_ceiling(&start, parent, ceilings) => {
                from = parent.to_path_buf();
            }
            _ => return Ok(None),
        }
    }
}

/// The work tree whose `.git` sits in `dir` itself, or `None` when Git does
/// not accept that `.git` (an empty directory, garbage, or a gitfile that
/// points nowhere). Every other failure is an error.
pub(crate) async fn work_tree_at(
    runner: &GitRunner,
    dir: &Path,
) -> Result<Option<PathBuf>, GitError> {
    match runner
        .run(
            dir,
            &[os("rev-parse"), os("--show-toplevel")],
            MAX_SMALL_STDOUT_BYTES,
            &[0],
        )
        .await
    {
        Ok(output) => parse_toplevel(&output.stdout).map(Some),
        Err(GitError::Failed(message)) if is_not_a_repository(&message) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Git's answer when no usable repository was found.
fn is_not_a_repository(message: &str) -> bool {
    message.starts_with("fatal: not a git repository")
        || message.starts_with("fatal: invalid gitfile format")
}

fn parse_toplevel(stdout: &[u8]) -> Result<PathBuf, GitError> {
    use std::os::unix::ffi::OsStrExt;
    let line = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    let path = PathBuf::from(OsStr::from_bytes(line));
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(GitError::Failed(
            "git reported no absolute work tree".to_owned(),
        ))
    }
}

/// `path` with symlinks resolved, as Git sees it; the literal path when it
/// cannot be resolved (it does not exist).
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Ceiling directories with symlinks resolved, as Git applies them.
pub(crate) fn canonical_ceilings(ceilings: &[PathBuf]) -> Vec<PathBuf> {
    ceilings.iter().map(|ceiling| canonical(ceiling)).collect()
}

/// `dir` is (or is above) a ceiling that applies to `start`. All three are
/// canonical paths.
fn hits_ceiling(start: &Path, dir: &Path, ceilings: &[PathBuf]) -> bool {
    ceilings
        .iter()
        .any(|ceiling| start.starts_with(ceiling) && ceiling.starts_with(dir))
}

/// The nearest directory from `from` upward, below every ceiling, whose
/// `.git` exists and does not resolve to a directory (a gitfile). `from` and
/// `ceilings` are canonical paths.
fn nearest_git_file(from: &Path, ceilings: &[PathBuf]) -> Option<PathBuf> {
    from.ancestors()
        .take_while(|dir| !hits_ceiling(from, dir, ceilings))
        .find(|dir| {
            let git = dir.join(".git");
            std::fs::symlink_metadata(&git).is_ok()
                && !std::fs::metadata(&git).is_ok_and(|metadata| metadata.is_dir())
        })
        .map(Path::to_path_buf)
}

/// Read-only state of one linked worktree plus its ignored paths (absolute).
pub(crate) async fn probe_worktree(
    runner: &GitRunner,
    worktree: &Path,
) -> Result<(WorktreeGitFacts, Vec<PathBuf>), GitError> {
    let status = status(runner, worktree, "all", true).await?;
    let listing = runner
        .run(
            worktree,
            &[os("ls-files"), os("-v"), os("-z")],
            MAX_STATUS_STDOUT_BYTES,
            &[0],
        )
        .await?;
    let mut facts = WorktreeGitFacts {
        changed: status.changed,
        untracked: status.untracked,
        hidden_from_status: count_hidden_from_status(&listing.stdout, worktree),
        detached: status.detached,
        // An upstream without `branch.ab` no longer exists.
        has_upstream: status.upstream.is_some() && status.ahead.is_some(),
        upstream: status.upstream.clone(),
        ..WorktreeGitFacts::default()
    };
    let mut remote = None;
    if let Some(branch) = status.branch.as_deref().filter(|_| facts.has_upstream) {
        let reference = format!("refs/heads/{branch}");
        let output = runner
            .run(
                worktree,
                &[
                    os("for-each-ref"),
                    os("--format=%(upstream:remotename)"),
                    OsStr::new(reference.as_str()),
                ],
                MAX_SMALL_STDOUT_BYTES,
                &[0],
            )
            .await?;
        let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if name.is_empty() || name == "." {
            facts.upstream_is_local = true;
        } else {
            remote = Some(name);
            facts.pushed = Some(
                yes_no(
                    runner,
                    worktree,
                    &[
                        os("merge-base"),
                        os("--is-ancestor"),
                        os("HEAD"),
                        os("@{upstream}"),
                    ],
                )
                .await?,
            );
        }
    }
    let remote = remote.unwrap_or_else(|| "origin".to_owned());
    if let Some(default_ref) = default_branch_ref(runner, worktree, &remote).await? {
        facts.merged = Some(
            yes_no(
                runner,
                worktree,
                &[
                    os("merge-base"),
                    os("--is-ancestor"),
                    os("HEAD"),
                    OsStr::new(default_ref.as_str()),
                ],
            )
            .await?,
        );
        facts.default_branch = Some(
            default_ref
                .strip_prefix("refs/remotes/")
                .unwrap_or(&default_ref)
                .to_owned(),
        );
    }
    let ignored = status
        .ignored
        .iter()
        .map(|path| worktree.join(path))
        .collect();
    Ok((facts, ignored))
}

/// Index entries `git status` never reports: assume-unchanged (lowercase
/// tag) always, skip-worktree (`S`) when the file exists on disk (a sparse
/// checkout's absent files hold no edits). Edits to them are lost when the
/// worktree is removed.
fn count_hidden_from_status(ls_files_v: &[u8], worktree: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;

    let hidden = ls_files_v
        .split(|byte| *byte == 0)
        .filter(|record| match record {
            [tag, b' ', path @ ..] if tag.is_ascii_lowercase() => !path.is_empty(),
            [b'S', b' ', path @ ..] if !path.is_empty() => {
                std::fs::symlink_metadata(worktree.join(OsStr::from_bytes(path))).is_ok()
            }
            _ => false,
        })
        .count();
    u64::try_from(hidden).unwrap_or(u64::MAX)
}

/// Conventional default branch names tried when `<remote>/HEAD` is not set
/// (some clones have no `origin/HEAD` and use `mainline`).
const DEFAULT_BRANCH_NAMES: [&str; 3] = ["main", "mainline", "master"];

/// The remote-tracking default branch: `<remote>/HEAD` when set, else the
/// only existing one of `<remote>/{main,mainline,master}` (none or several
/// -> unknown). No fetch runs, so a stale ref can only block.
async fn default_branch_ref(
    runner: &GitRunner,
    dir: &Path,
    remote: &str,
) -> Result<Option<String>, GitError> {
    let head = format!("refs/remotes/{remote}/HEAD");
    let output = runner
        .run(
            dir,
            &[os("symbolic-ref"), os("--quiet"), OsStr::new(head.as_str())],
            MAX_SMALL_STDOUT_BYTES,
            &[0, 1],
        )
        .await?;
    if output.code == Some(0) {
        let target = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if target.starts_with("refs/remotes/") {
            return Ok(Some(target));
        }
    }
    let mut found = None;
    for name in DEFAULT_BRANCH_NAMES {
        let reference = format!("refs/remotes/{remote}/{name}");
        let exists = yes_no(
            runner,
            dir,
            &[
                os("rev-parse"),
                os("--verify"),
                os("--quiet"),
                OsStr::new(reference.as_str()),
            ],
        )
        .await?;
        if exists {
            if found.is_some() {
                return Ok(None);
            }
            found = Some(reference);
        }
    }
    Ok(found)
}

/// Read-only state of a nested workspace package repository.
pub(crate) async fn probe_package(
    runner: &GitRunner,
    package: &Path,
) -> Result<PackageGitFacts, GitError> {
    let status = status(runner, package, "normal", false).await?;
    let stash = yes_no(
        runner,
        package,
        &[
            os("rev-parse"),
            os("--verify"),
            os("--quiet"),
            os("refs/stash"),
        ],
    )
    .await?;
    Ok(PackageGitFacts {
        changed: status.changed,
        untracked: status.untracked,
        detached: status.detached,
        has_upstream: status.upstream.is_some(),
        upstream_gone: status.upstream.is_some() && status.ahead.is_none(),
        ahead: status.ahead,
        stash,
    })
}

/// `git ls-files` and `git check-ignore` for generated output inside a
/// work tree. More than a bounded amount of tracked output still means
/// "contains tracked files". `ls-files` reads the path literally;
/// `check-ignore` rejects pathspec magic, which fails closed.
pub(crate) async fn probe_generated(
    runner: &GitRunner,
    work_tree: &Path,
    candidate: &Path,
) -> Result<GeneratedGitFacts, GitError> {
    let relative = candidate
        .strip_prefix(work_tree)
        .map_err(|_| GitError::Failed("candidate is outside its work tree".to_owned()))?;
    let tracked = match runner
        .run(
            work_tree,
            &[
                os("--literal-pathspecs"),
                os("ls-files"),
                os("-z"),
                os("--"),
                relative.as_os_str(),
            ],
            MAX_SMALL_STDOUT_BYTES,
            &[0],
        )
        .await
    {
        Ok(output) => !output.stdout.is_empty(),
        Err(GitError::OutputTooLarge) => true,
        Err(error) => return Err(error),
    };
    if tracked {
        return Ok(GeneratedGitFacts {
            tracked,
            ignored: false,
        });
    }
    let ignored = yes_no(
        runner,
        work_tree,
        &[os("check-ignore"), os("-q"), os("--"), relative.as_os_str()],
    )
    .await?;
    Ok(GeneratedGitFacts { tracked, ignored })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_list_records_and_flags() {
        let output = b"worktree /repo\0HEAD 1111\0branch refs/heads/main\0\0\
worktree /repo/.worktrees/a b\0HEAD 2222\0detached\0locked reason with spaces\0\0\
worktree /gone\0HEAD 3333\0branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0";
        let entries = parse_worktree_list(output).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, PathBuf::from("/repo"));
        assert_eq!(entries[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(entries[1].path, PathBuf::from("/repo/.worktrees/a b"));
        assert!(entries[1].detached && entries[1].locked && !entries[1].prunable);
        assert!(entries[2].prunable);
        assert!(parse_worktree_list(b"HEAD 1111\0").is_err());
    }

    #[test]
    fn parses_status_headers_and_records() {
        let output = b"# branch.oid abc\0# branch.head feature\0# branch.upstream origin/feature\0\
# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa aaa src/lib.rs\0\
2 R. N... 100644 100644 100644 bbb bbb R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 c c c conflict.rs\0\
? notes.md\0! target/\0! .yard/yard.sqlite3\0";
        let status = parse_status(output).unwrap();
        assert_eq!(status.branch.as_deref(), Some("feature"));
        assert_eq!(status.upstream.as_deref(), Some("origin/feature"));
        assert_eq!((status.ahead, status.behind), (Some(2), Some(1)));
        assert_eq!(status.changed, 3);
        assert_eq!(status.untracked, 1);
        assert_eq!(
            status.ignored,
            [
                PathBuf::from("target/"),
                PathBuf::from(".yard/yard.sqlite3")
            ]
        );
        let detached = parse_status(b"# branch.oid abc\0# branch.head (detached)\0").unwrap();
        assert!(detached.detached && detached.branch.is_none() && detached.upstream.is_none());
        assert!(parse_status(b"X weird\0").is_err());
    }

    #[test]
    fn recognizes_dubious_ownership_refusals() {
        assert!(is_dubious_ownership(
            "fatal: detected dubious ownership in repository at '/x'\n"
        ));
        assert!(!is_dubious_ownership("fatal: not a git repository"));
        assert_eq!(
            GitCheck::<()>::from(GitError::DubiousOwnership),
            GitCheck::DubiousOwnership
        );
    }

    #[tokio::test]
    async fn a_hung_git_is_killed_at_its_timeout() {
        let temp = tempfile::TempDir::new().unwrap();
        let fake = temp.path().join("git");
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
        let runner = GitRunner::new(
            fake.into_os_string(),
            Duration::from_millis(200),
            Instant::now() + Duration::from_secs(60),
        );
        let started = std::time::Instant::now();
        let error = runner
            .run(temp.path(), &[os("status")], 1024, &[0])
            .await
            .unwrap_err();
        assert_eq!(error, GitError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(10));
        let expired = GitRunner::new(
            OsString::from("git"),
            Duration::from_secs(1),
            Instant::now(),
        );
        assert_eq!(
            expired
                .run(temp.path(), &[os("status")], 1024, &[0])
                .await
                .unwrap_err(),
            GitError::DeadlineExceeded
        );
    }
    fn fake_git(dir: &Path, body: &str) -> OsString {
        let fake = dir.join("fake-git");
        std::fs::write(&fake, format!("#!/bin/sh\n{body}")).unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
        fake.into_os_string()
    }

    fn runner(binary: OsString, command_timeout: Duration) -> GitRunner {
        GitRunner::new(
            binary,
            command_timeout,
            Instant::now() + Duration::from_secs(60),
        )
    }

    #[tokio::test]
    async fn a_dubious_ownership_refusal_is_its_own_error() {
        let temp = tempfile::TempDir::new().unwrap();
        let fake = fake_git(
            temp.path(),
            "echo \"fatal: detected dubious ownership in repository at '/x'\" >&2\nexit 128\n",
        );
        let error = runner(fake, Duration::from_secs(10))
            .run(temp.path(), &[os("status")], 1024, &[0])
            .await
            .unwrap_err();
        assert_eq!(error, GitError::DubiousOwnership);
    }

    #[tokio::test]
    async fn git_runs_with_hardening_flags_and_environment() {
        let temp = tempfile::TempDir::new().unwrap();
        let record = temp.path().join("record");
        let fake = fake_git(
            temp.path(),
            &format!(
                "printf '%s\\n' \"$@\" > '{0}.args'\nenv > '{0}.env'\n",
                record.display()
            ),
        );
        let work = temp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        runner(fake, Duration::from_secs(10))
            .run(&work, &[os("status"), os("-z")], 1024, &[0])
            .await
            .unwrap();
        let args = std::fs::read_to_string(temp.path().join("record.args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-C",
                work.to_str().unwrap(),
                "status",
                "-z",
            ]
        );
        let env = std::fs::read_to_string(temp.path().join("record.env")).unwrap();
        let env: Vec<&str> = env.lines().collect();
        for expected in [
            "GIT_CONFIG_NOSYSTEM=1",
            "GIT_OPTIONAL_LOCKS=0",
            "GIT_TERMINAL_PROMPT=0",
            "LC_ALL=C",
            &format!("GIT_CEILING_DIRECTORIES={}", temp.path().display()),
        ] {
            assert!(env.contains(&expected), "{expected} missing: {env:?}");
        }
    }

    #[tokio::test]
    async fn output_beyond_the_limit_is_refused() {
        let temp = tempfile::TempDir::new().unwrap();
        let fake = fake_git(
            temp.path(),
            "i=0\nwhile [ $i -lt 64 ]; do echo 0123456789012345678901234567890123456789; i=$((i+1)); done\n",
        );
        let runner = runner(fake, Duration::from_secs(10));
        assert_eq!(
            runner
                .run(temp.path(), &[os("status")], 1024, &[0])
                .await
                .unwrap_err(),
            GitError::OutputTooLarge
        );
        assert!(
            runner
                .run(temp.path(), &[os("status")], 8 * 1024, &[0])
                .await
                .is_ok(),
            "control: the same output fits a larger limit"
        );
    }

    #[tokio::test]
    async fn a_timed_out_git_process_is_killed() {
        let temp = tempfile::TempDir::new().unwrap();
        let pid_file = temp.path().join("pid");
        let fake = fake_git(
            temp.path(),
            &format!(
                "echo $$ > '{0}.tmp' && mv '{0}.tmp' '{0}'\nexec sleep 30\n",
                pid_file.display()
            ),
        );
        let error = runner(fake, Duration::from_secs(1))
            .run(temp.path(), &[os("status")], 1024, &[0])
            .await
            .unwrap_err();
        assert_eq!(error, GitError::Timeout);
        // Under load the child may be killed before it records its PID; a
        // surviving child records it late and is then found alive.
        let mut pid = None;
        for _ in 0..40 {
            if let Ok(recorded) = std::fs::read_to_string(&pid_file) {
                pid = Some(recorded.trim().to_owned());
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let Some(pid) = pid else {
            return;
        };
        // Sandboxes that forbid inspecting other processes cannot answer this; the
        // timeout itself is already asserted above, so skip only the liveness check.
        let alive = || {
            let output = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid])
                .output()
                .ok()?;
            let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            Some(!state.is_empty() && !state.starts_with('Z'))
        };
        if alive().is_none() {
            return;
        }
        let mut waited = 0;
        while alive().unwrap_or(false) && waited < 100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            waited += 1;
        }
        let still_alive = alive().unwrap_or(false);
        if still_alive {
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid])
                .status();
        }
        assert!(!still_alive, "git process {pid} outlived its timeout");
    }

    #[tokio::test]
    async fn discovery_skips_stale_dot_git_and_never_enters_a_ceiling() {
        let temp = tempfile::TempDir::new().unwrap();
        let base = std::fs::canonicalize(temp.path()).unwrap();
        let ceilings = [base.parent().unwrap().to_path_buf()];
        let git = runner("git".into(), Duration::from_secs(10))
            .with_env(vec![("GIT_CONFIG_GLOBAL".into(), "/dev/null".into())]);
        let repo = base.join("repo");
        let start = repo.join("stale/start");
        std::fs::create_dir_all(&start).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(init.success());
        let stale = repo.join("stale/.git");
        for (kind, plant) in [
            ("empty directory", None),
            ("garbage file", Some("garbage\n")),
            (
                "gitfile pointing nowhere",
                Some("gitdir: /nonexistent/yard\n"),
            ),
        ] {
            match plant {
                None => std::fs::create_dir_all(&stale).unwrap(),
                Some(contents) => std::fs::write(&stale, contents).unwrap(),
            }
            assert_eq!(
                discover_work_tree(&git, &start, &ceilings).await,
                Ok(Some(repo.clone())),
                "{kind}: Git looks past it to the real repository"
            );
            assert_eq!(
                work_tree_at(&git, &repo.join("stale")).await,
                Ok(None),
                "{kind}: not a repository"
            );
            assert_eq!(
                discover_work_tree(&git, &start, std::slice::from_ref(&repo)).await,
                Ok(None),
                "{kind}: the ceiling stops discovery"
            );
            if stale.is_dir() {
                std::fs::remove_dir(&stale).unwrap();
            } else {
                std::fs::remove_file(&stale).unwrap();
            }
        }
        assert_eq!(work_tree_at(&git, &repo).await, Ok(Some(repo.clone())));
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            discover_work_tree(&git, &outside, &ceilings).await,
            Ok(None)
        );

        let refusing = fake_git(
            &base,
            "echo 'fatal: detected dubious ownership in repository at /x' >&2\nexit 128\n",
        );
        assert_eq!(
            discover_work_tree(
                &runner(refusing, Duration::from_secs(10)),
                &start,
                &ceilings
            )
            .await,
            Err(GitError::DubiousOwnership)
        );
    }
}

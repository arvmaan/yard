use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};
use yard_domain::{
    ProjectRepository, RepositoryDiff, RepositoryDiffHunk, RepositoryDiffLine,
    RepositoryDiffLineKind, RepositoryFile, RepositoryFileContent, RepositoryFileMode,
    RepositoryFileState, RepositoryFiles,
};

use crate::project_service::{ProjectService, ProjectServiceError};

const GIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_GIT_LIST_BYTES: usize = 4 * 1024 * 1024;
const MAX_GIT_DIFF_BYTES: usize = 2 * 1024 * 1024;
const MAX_GIT_ERROR_BYTES: usize = 64 * 1024;
const MAX_CONTENT_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_PATH_BYTES: usize = 4_096;

#[derive(Clone)]
pub struct RepositoryFilesService {
    projects: ProjectService,
    #[cfg(test)]
    git_processes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl RepositoryFilesService {
    #[must_use]
    pub fn new(projects: ProjectService) -> Self {
        Self {
            projects,
            #[cfg(test)]
            git_processes: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// List bounded repository metadata without reading file content.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryFilesServiceError`] when repository ownership,
    /// identity, Git, or returned path data cannot be verified.
    pub async fn list(
        &self,
        project_id: &str,
        repository_id: &str,
        mode: RepositoryFileMode,
    ) -> Result<RepositoryFiles, RepositoryFilesServiceError> {
        let repository = self.repository(project_id, repository_id).await?;
        match mode {
            RepositoryFileMode::Browse => self.list_browse(&repository).await,
            RepositoryFileMode::Review => self.list_review(&repository).await,
        }
    }

    /// Read one bounded repository-relative text file.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryFilesServiceError`] when repository ownership,
    /// identity, or path containment cannot be verified.
    pub async fn content(
        &self,
        project_id: &str,
        repository_id: &str,
        path: &str,
    ) -> Result<RepositoryFileContent, RepositoryFilesServiceError> {
        let repository = self.selected_repository(project_id, repository_id).await?;
        let resolved = safe_repository_path(&repository, path, true).await?;
        let mut response = RepositoryFileContent {
            repository_id: repository.id,
            root_path: repository.root_path,
            path: path.to_owned(),
            content: None,
            binary: false,
            truncated: false,
            unavailable: false,
        };
        match bounded_file(&resolved).await? {
            Some(BoundedFile::Text { content, truncated }) => {
                response.content = Some(content);
                response.truncated = truncated;
            }
            Some(BoundedFile::Binary) => response.binary = true,
            None => response.unavailable = true,
        }
        Ok(response)
    }

    /// Read one selected repository diff as structured hunks.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryFilesServiceError`] when repository ownership,
    /// identity, path containment, Git execution, or diff parsing fails.
    pub async fn diff(
        &self,
        project_id: &str,
        repository_id: &str,
        path: &str,
        previous_path: Option<&str>,
    ) -> Result<RepositoryDiff, RepositoryFilesServiceError> {
        let repository = self.selected_repository(project_id, repository_id).await?;
        let resolved = safe_repository_path(&repository, path, true).await?;
        if let Some(previous_path) = previous_path {
            safe_repository_path(&repository, previous_path, true).await?;
        }
        let mut response = RepositoryDiff {
            repository_id: repository.id.clone(),
            root_path: repository.root_path.clone(),
            path: path.to_owned(),
            previous_path: None,
            hunks: Vec::new(),
            binary: false,
            truncated: false,
            unavailable: false,
        };
        let mut args = diff_prefix();
        args.extend([
            OsString::from("-M"),
            OsString::from("HEAD"),
            OsString::from("--"),
            OsString::from(path),
        ]);
        if let Some(previous_path) = previous_path {
            args.push(OsString::from(previous_path));
        }
        let output = self.run_git(&repository, args, MAX_GIT_DIFF_BYTES).await?;
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| RepositoryFilesServiceError::InvalidGitOutput)?;
        if let Some(previous_path) = previous_path {
            let expected_from = format!("rename from {previous_path}");
            let expected_to = format!("rename to {path}");
            if !text.lines().any(|line| line == expected_from)
                || !text.lines().any(|line| line == expected_to)
            {
                return Err(RepositoryFilesServiceError::InvalidPath);
            }
        }
        response.truncated = output.truncated;
        response.binary = text.contains("\nBinary files ") || text.contains("\nGIT binary patch\n");
        response.previous_path = text
            .lines()
            .find_map(|line| line.strip_prefix("rename from "))
            .map(str::to_owned);
        if !response.binary {
            response.hunks = parse_diff(text)?;
        }
        if response.hunks.is_empty() && !response.binary && response.previous_path.is_none() {
            match bounded_file(&resolved).await? {
                Some(BoundedFile::Text { content, truncated }) => {
                    response.hunks = added_file_hunk(&content);
                    response.truncated |= truncated;
                }
                Some(BoundedFile::Binary) => response.binary = true,
                None if output.success => response.unavailable = true,
                None => return Err(RepositoryFilesServiceError::GitFailed(output.error())),
            }
        }
        Ok(response)
    }

    async fn repository(
        &self,
        project_id: &str,
        repository_id: &str,
    ) -> Result<ProjectRepository, RepositoryFilesServiceError> {
        let repository = self
            .projects
            .get_repository(project_id, repository_id)
            .await?;
        Ok(repository)
    }

    async fn selected_repository(
        &self,
        project_id: &str,
        repository_id: &str,
    ) -> Result<ProjectRepository, RepositoryFilesServiceError> {
        let repository = self.repository(project_id, repository_id).await?;
        crate::project_service::validate_repository_identity_for_read(&repository)
            .await
            .map_err(|_| RepositoryFilesServiceError::RepositoryIdentityChanged)?;
        Ok(repository)
    }

    async fn list_browse(
        &self,
        repository: &ProjectRepository,
    ) -> Result<RepositoryFiles, RepositoryFilesServiceError> {
        let tracked = self
            .run_git(
                repository,
                vec![
                    OsString::from("ls-files"),
                    OsString::from("-z"),
                    OsString::from("--cached"),
                ],
                MAX_GIT_LIST_BYTES,
            )
            .await?;
        let untracked = self
            .run_git(
                repository,
                vec![
                    OsString::from("ls-files"),
                    OsString::from("-z"),
                    OsString::from("--others"),
                    OsString::from("--exclude-standard"),
                ],
                MAX_GIT_LIST_BYTES,
            )
            .await?;
        if (!tracked.success && !tracked.truncated) || (!untracked.success && !untracked.truncated)
        {
            return Err(RepositoryFilesServiceError::GitFailed(if tracked.success {
                untracked.error()
            } else {
                tracked.error()
            }));
        }
        let mut files = BTreeMap::new();
        for path in parse_nul_paths(&tracked.stdout, tracked.truncated)? {
            files.insert(
                path.clone(),
                browse_file(path, RepositoryFileState::Tracked),
            );
        }
        for path in parse_nul_paths(&untracked.stdout, untracked.truncated)? {
            files.insert(
                path.clone(),
                browse_file(path, RepositoryFileState::Untracked),
            );
        }
        Ok(repository_files(
            repository,
            RepositoryFileMode::Browse,
            files.into_values().collect(),
            tracked.truncated || untracked.truncated,
        ))
    }

    async fn list_review(
        &self,
        repository: &ProjectRepository,
    ) -> Result<RepositoryFiles, RepositoryFilesServiceError> {
        let output = self.status(repository, None).await?;
        let files = parse_status(&output.stdout, output.truncated)?;
        Ok(repository_files(
            repository,
            RepositoryFileMode::Review,
            files,
            output.truncated,
        ))
    }

    async fn status(
        &self,
        repository: &ProjectRepository,
        path: Option<&str>,
    ) -> Result<GitOutput, RepositoryFilesServiceError> {
        let mut args = vec![
            OsString::from("status"),
            OsString::from("--porcelain=v2"),
            OsString::from("-z"),
            OsString::from("--untracked-files=all"),
        ];
        if let Some(path) = path {
            args.extend([OsString::from("--"), OsString::from(path)]);
        }
        let output = self.run_git(repository, args, MAX_GIT_LIST_BYTES).await?;
        if !output.success && !output.truncated {
            return Err(RepositoryFilesServiceError::GitFailed(output.error()));
        }
        Ok(output)
    }

    async fn run_git(
        &self,
        repository: &ProjectRepository,
        args: Vec<OsString>,
        output_limit: usize,
    ) -> Result<GitOutput, RepositoryFilesServiceError> {
        #[cfg(test)]
        self.git_processes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut command = Command::new("git");
        command
            .arg("--no-optional-locks")
            .arg("--literal-pathspecs")
            .args(["-c", "color.ui=false"])
            .args(["-c", "core.fsmonitor=false"])
            .args(["-c", "core.quotePath=false"])
            .args(["-c", "diff.external="])
            .args(["-c", "diff.trustExitCode=false"])
            .args(args)
            .current_dir(&repository.root_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_EXTERNAL_DIFF")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| RepositoryFilesServiceError::GitUnavailable(error.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RepositoryFilesServiceError::GitUnavailable("stdout missing".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RepositoryFilesServiceError::GitUnavailable("stderr missing".into()))?;
        let stdout_task = tokio::spawn(read_bounded(stdout, output_limit));
        let stderr_task = tokio::spawn(read_bounded(stderr, MAX_GIT_ERROR_BYTES));
        let Ok(status) = timeout(GIT_TIMEOUT, child.wait()).await else {
            child.kill().await?;
            let _ = child.wait().await;
            return Err(RepositoryFilesServiceError::GitTimeout);
        };
        let status = status?;
        let stdout = stdout_task
            .await
            .map_err(|error| RepositoryFilesServiceError::GitUnavailable(error.to_string()))??;
        let stderr = stderr_task
            .await
            .map_err(|error| RepositoryFilesServiceError::GitUnavailable(error.to_string()))??;
        Ok(GitOutput {
            success: status.success(),
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            truncated: stdout.truncated || stderr.truncated,
        })
    }

    #[cfg(test)]
    fn git_process_count(&self) -> usize {
        self.git_processes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn browse_file(path: String, state: RepositoryFileState) -> RepositoryFile {
    RepositoryFile {
        path,
        previous_path: None,
        state,
        staged: false,
        unstaged: false,
        additions: None,
        deletions: None,
    }
}

fn repository_files(
    repository: &ProjectRepository,
    mode: RepositoryFileMode,
    mut files: Vec<RepositoryFile>,
    process_truncated: bool,
) -> RepositoryFiles {
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let count_truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    RepositoryFiles {
        repository_id: repository.id.clone(),
        root_path: repository.root_path.clone(),
        mode,
        files,
        truncated: process_truncated || count_truncated,
    }
}

fn diff_prefix() -> Vec<OsString> {
    [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--unified=3",
    ]
    .into_iter()
    .map(Into::into)
    .collect()
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> io::Result<BoundedBytes> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(BoundedBytes {
                bytes,
                truncated: false,
            });
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        if read > remaining {
            return Ok(BoundedBytes {
                bytes,
                truncated: true,
            });
        }
    }
}

struct GitOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

impl GitOutput {
    fn error(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

async fn safe_repository_path(
    repository: &ProjectRepository,
    path: &str,
    allow_missing_file: bool,
) -> Result<PathBuf, RepositoryFilesServiceError> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err(RepositoryFilesServiceError::InvalidPath);
    }
    let relative = Path::new(path);
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(RepositoryFilesServiceError::InvalidPath);
    }
    let root = fs::canonicalize(&repository.root_path).await?;
    let mut current = root.clone();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                current = fs::canonicalize(&current).await?;
                if !current.starts_with(&root) {
                    return Err(RepositoryFilesServiceError::PathEscape);
                }
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && allow_missing_file
                    && index + 1 == components.len() =>
            {
                return Ok(current);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(RepositoryFilesServiceError::InvalidPath);
            }
            Err(error) => return Err(error.into()),
        }
    }
    let canonical = fs::canonicalize(&current).await?;
    if !canonical.starts_with(&root) {
        return Err(RepositoryFilesServiceError::PathEscape);
    }
    Ok(canonical)
}

enum BoundedFile {
    Text { content: String, truncated: bool },
    Binary,
}

async fn bounded_file(path: &Path) -> Result<Option<BoundedFile>, RepositoryFilesServiceError> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Err(RepositoryFilesServiceError::InvalidPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let file = fs::File::open(path).await?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_CONTENT_BYTES)
            .min(MAX_CONTENT_BYTES),
    );
    file.take(u64::try_from(MAX_CONTENT_BYTES).expect("content limit fits u64") + 1)
        .read_to_end(&mut bytes)
        .await?;
    let truncated = bytes.len() > MAX_CONTENT_BYTES;
    bytes.truncate(MAX_CONTENT_BYTES);
    if bytes.contains(&0) {
        return Ok(Some(BoundedFile::Binary));
    }
    if truncated {
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_none() {
                bytes.truncate(error.valid_up_to());
            }
        }
    }
    Ok(Some(match String::from_utf8(bytes) {
        Ok(content) => BoundedFile::Text { content, truncated },
        Err(_) => BoundedFile::Binary,
    }))
}

fn added_file_hunk(content: &str) -> Vec<RepositoryDiffHunk> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<_> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let new_lines = u64::try_from(lines.len()).unwrap_or(u64::MAX);
    vec![RepositoryDiffHunk {
        old_start: 0,
        old_lines: 0,
        new_start: 1,
        new_lines,
        lines: lines
            .into_iter()
            .enumerate()
            .map(|(index, content)| RepositoryDiffLine {
                kind: RepositoryDiffLineKind::Addition,
                old_line: None,
                new_line: Some(u64::try_from(index + 1).unwrap_or(u64::MAX)),
                content: content.strip_suffix('\r').unwrap_or(content).to_owned(),
            })
            .collect(),
    }]
}

fn parse_nul_paths(
    output: &[u8],
    truncated: bool,
) -> Result<Vec<String>, RepositoryFilesServiceError> {
    let text =
        std::str::from_utf8(output).map_err(|_| RepositoryFilesServiceError::InvalidGitOutput)?;
    let mut paths: Vec<_> = text.split('\0').collect();
    if paths.last() == Some(&"") || truncated {
        paths.pop();
    }
    paths
        .into_iter()
        .map(|path| {
            if path.is_empty() {
                Err(RepositoryFilesServiceError::InvalidGitOutput)
            } else {
                Ok(path.to_owned())
            }
        })
        .collect()
}

fn parse_status(
    output: &[u8],
    truncated: bool,
) -> Result<Vec<RepositoryFile>, RepositoryFilesServiceError> {
    let text =
        std::str::from_utf8(output).map_err(|_| RepositoryFilesServiceError::InvalidGitOutput)?;
    let mut records: Vec<_> = text.split('\0').collect();
    if records.last() == Some(&"") || truncated {
        records.pop();
    }
    let mut files = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        let mut file = match record.as_bytes().first().copied() {
            Some(b'?') => browse_file(
                record
                    .strip_prefix("? ")
                    .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?
                    .to_owned(),
                RepositoryFileState::Untracked,
            ),
            Some(b'1') => {
                let fields: Vec<_> = record.splitn(9, ' ').collect();
                status_file(fields.get(1).copied(), fields.get(8).copied(), None)?
            }
            Some(b'2') => {
                let fields: Vec<_> = record.splitn(10, ' ').collect();
                index += 1;
                status_file(
                    fields.get(1).copied(),
                    fields.get(9).copied(),
                    records.get(index).copied(),
                )?
            }
            Some(b'u') => {
                let fields: Vec<_> = record.splitn(11, ' ').collect();
                let mut file = status_file(fields.get(1).copied(), fields.get(10).copied(), None)?;
                file.state = RepositoryFileState::Conflicted;
                file
            }
            _ => return Err(RepositoryFilesServiceError::InvalidGitOutput),
        };
        if file.state == RepositoryFileState::Untracked {
            file.unstaged = true;
        }
        files.push(file);
        index += 1;
    }
    Ok(files)
}

fn status_file(
    xy: Option<&str>,
    path: Option<&str>,
    previous_path: Option<&str>,
) -> Result<RepositoryFile, RepositoryFilesServiceError> {
    let xy = xy.ok_or(RepositoryFilesServiceError::InvalidGitOutput)?;
    let mut chars = xy.chars();
    let staged_code = chars
        .next()
        .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?;
    let unstaged_code = chars
        .next()
        .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?;
    if chars.next().is_some() {
        return Err(RepositoryFilesServiceError::InvalidGitOutput);
    }
    let state = repository_file_state(staged_code, unstaged_code);
    Ok(RepositoryFile {
        path: path
            .filter(|path| !path.is_empty())
            .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?
            .to_owned(),
        previous_path: previous_path.map(str::to_owned),
        state,
        staged: staged_code != '.',
        unstaged: unstaged_code != '.',
        additions: None,
        deletions: None,
    })
}

fn repository_file_state(staged: char, unstaged: char) -> RepositoryFileState {
    let codes = [staged, unstaged];
    if codes.contains(&'U') || matches!((staged, unstaged), ('A' | 'D', 'A' | 'D')) {
        RepositoryFileState::Conflicted
    } else if codes.contains(&'R') || codes.contains(&'C') {
        RepositoryFileState::Renamed
    } else if codes.contains(&'D') {
        RepositoryFileState::Deleted
    } else if codes.contains(&'A') {
        RepositoryFileState::Added
    } else if codes.contains(&'T') {
        RepositoryFileState::TypeChanged
    } else {
        RepositoryFileState::Modified
    }
}

fn parse_diff(text: &str) -> Result<Vec<RepositoryDiffHunk>, RepositoryFilesServiceError> {
    let mut hunks = Vec::new();
    let mut current: Option<RepositoryDiffHunk> = None;
    let mut old_line = 0;
    let mut new_line = 0;
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.starts_with("@@ ") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            let (old_start, old_lines, new_start, new_lines) = parse_hunk_header(line)?;
            old_line = old_start;
            new_line = new_start;
            current = Some(RepositoryDiffHunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        let (kind, old, new, content) = match line.as_bytes().first().copied() {
            Some(b' ') => {
                let old = old_line;
                let new = new_line;
                old_line += 1;
                new_line += 1;
                (
                    RepositoryDiffLineKind::Context,
                    Some(old),
                    Some(new),
                    &line[1..],
                )
            }
            Some(b'+') => {
                let new = new_line;
                new_line += 1;
                (
                    RepositoryDiffLineKind::Addition,
                    None,
                    Some(new),
                    &line[1..],
                )
            }
            Some(b'-') => {
                let old = old_line;
                old_line += 1;
                (
                    RepositoryDiffLineKind::Deletion,
                    Some(old),
                    None,
                    &line[1..],
                )
            }
            Some(b'\\') => (
                RepositoryDiffLineKind::NoNewline,
                None,
                None,
                line.strip_prefix("\\ ").unwrap_or(&line[1..]),
            ),
            None => continue,
            _ => return Err(RepositoryFilesServiceError::InvalidGitOutput),
        };
        hunk.lines.push(RepositoryDiffLine {
            kind,
            old_line: old,
            new_line: new,
            content: content.to_owned(),
        });
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    Ok(hunks)
}

fn parse_hunk_header(header: &str) -> Result<(u64, u64, u64, u64), RepositoryFilesServiceError> {
    let end = header[3..]
        .find(" @@")
        .map(|index| index + 3)
        .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?;
    let mut ranges = header[3..end].split(' ');
    let (old_start, old_lines) = parse_diff_range(
        ranges
            .next()
            .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?,
        '-',
    )?;
    let (new_start, new_lines) = parse_diff_range(
        ranges
            .next()
            .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?,
        '+',
    )?;
    Ok((old_start, old_lines, new_start, new_lines))
}

fn parse_diff_range(range: &str, prefix: char) -> Result<(u64, u64), RepositoryFilesServiceError> {
    let range = range
        .strip_prefix(prefix)
        .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?;
    let mut values = range.split(',');
    let start = values
        .next()
        .ok_or(RepositoryFilesServiceError::InvalidGitOutput)?
        .parse()
        .map_err(|_| RepositoryFilesServiceError::InvalidGitOutput)?;
    let lines = values
        .next()
        .map(str::parse)
        .transpose()
        .map_err(|_| RepositoryFilesServiceError::InvalidGitOutput)?
        .unwrap_or(1);
    if values.next().is_some() {
        return Err(RepositoryFilesServiceError::InvalidGitOutput);
    }
    Ok((start, lines))
}

#[derive(Debug, Error)]
pub enum RepositoryFilesServiceError {
    #[error(transparent)]
    Project(#[from] ProjectServiceError),
    #[error("stored repository identity changed or is unavailable")]
    RepositoryIdentityChanged,
    #[error("repository path must be a contained relative file path")]
    InvalidPath,
    #[error("repository path escapes the linked checkout")]
    PathEscape,
    #[error("Git is unavailable: {0}")]
    GitUnavailable(String),
    #[error("Git command timed out")]
    GitTimeout,
    #[error("Git command failed: {0}")]
    GitFailed(String),
    #[error("Git returned invalid repository file data")]
    InvalidGitOutput,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::Arc,
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, CreateProject, ObservedStatus, Project, ProjectRepository,
        ProjectRuntimeBinding, RepositoryFileMode, RepositoryFileState, RuntimeInventory,
        RuntimeObservationState, RuntimeProcessState, RuntimeSessions, SetProjectRepository,
        WorkerRuntimeBinding,
    };
    use yard_store::{SqliteProjectStore, YardStore};

    use super::{
        RepositoryFilesService, RepositoryFilesServiceError, parse_diff, parse_status,
        repository_file_state,
    };
    use crate::{
        allocation_service::{RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest},
        inventory_service::{InventoryServiceError, InventorySource},
        project_service::{ProjectService, ProjectServiceError},
    };

    struct UnusedInventory;

    #[async_trait]
    impl InventorySource for UnusedInventory {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            unreachable!("repository file tests do not inspect runtime inventory")
        }

        async fn inventory(
            &self,
            _session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            unreachable!("repository file tests do not inspect runtime inventory")
        }
    }

    struct UnusedRuntime;

    #[async_trait]
    impl RuntimeControl for UnusedRuntime {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            unreachable!("repository file tests do not provision workers")
        }
    }

    struct Fixture {
        temp: TempDir,
        repository_path: PathBuf,
        repository: ProjectRepository,
        project: Project,
        service: RepositoryFilesService,
        store: Arc<SqliteProjectStore>,
    }

    async fn fixture(committed: bool, suffix: &str) -> Fixture {
        let temp = TempDir::new().expect("temporary directory");
        let store = Arc::new(
            SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
                .await
                .expect("store"),
        );
        let project = create_project(&store, suffix).await;
        let repository_path = temp.path().join("repository");
        init_repository(&repository_path);
        if committed {
            for path in [
                "tracked.txt",
                "staged.txt",
                "unstaged.txt",
                "both.txt",
                "deleted.txt",
                "old.txt",
            ] {
                fs::write(repository_path.join(path), format!("base {path}\n")).expect("base file");
            }
            fs::write(repository_path.join("binary.bin"), [0_u8, 1, 2]).expect("binary file");
            git(&repository_path, &["add", "."]);
            git_commit(&repository_path, "base");
        }
        let projects = ProjectService::new(
            Arc::new(UnusedInventory),
            Arc::new(UnusedRuntime),
            store.clone(),
        );
        let repository = projects
            .link_repository(
                &project.id,
                "repository-1",
                SetProjectRepository {
                    root_path: repository_path.to_string_lossy().into_owned(),
                },
            )
            .await
            .expect("linked repository");
        Fixture {
            temp,
            repository_path,
            repository,
            project,
            service: RepositoryFilesService::new(projects),
            store,
        }
    }

    async fn create_project(store: &Arc<SqliteProjectStore>, suffix: &str) -> Project {
        store
            .create_project(
                CreateProject {
                    name: format!("Repository files {suffix}"),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: format!("workspace-{suffix}"),
                    },
                    orchestrator_observed_worker_id: format!("terminal-{suffix}"),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                WorkerRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "default".to_owned(),
                    workspace_id: format!("workspace-{suffix}"),
                    terminal_id: format!("terminal-{suffix}"),
                    tab_id: Some(format!("tab-{suffix}")),
                    pane_id: format!("pane-{suffix}"),
                    provider_session: None,
                    owns_tab: false,
                    observation_state: RuntimeObservationState::Observed,
                    process_state: RuntimeProcessState::Running,
                    status: ObservedStatus::Idle,
                    state_change_sequence: 1,
                    revision: 1,
                    version: 1,
                    last_observed_at_unix_ms: 1,
                },
            )
            .await
            .expect("project")
    }

    fn init_repository(path: &Path) {
        fs::create_dir_all(path).expect("repository directory");
        git(path, &["init", "--quiet"]);
    }

    fn git(path: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(path)
                .args(args)
                .status()
                .expect("git")
                .success(),
            "git {args:?}"
        );
    }

    fn git_commit(path: &Path, message: &str) {
        assert!(
            Command::new("git")
                .current_dir(path)
                .args([
                    "-c",
                    "user.name=Yard Tests",
                    "-c",
                    "user.email=yard@example.invalid",
                    "commit",
                    "--quiet",
                    "-m",
                    message,
                ])
                .status()
                .expect("git commit")
                .success()
        );
    }

    #[test]
    fn parses_structured_hunks_and_line_numbers() {
        let hunks = parse_diff(
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,3 @@\n one\n-old\n+new\n+tail\n",
        )
        .expect("diff parses");
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_start, hunks[0].new_start), (1, 1));
        assert_eq!(hunks[0].lines[1].old_line, Some(2));
        assert_eq!(hunks[0].lines[2].new_line, Some(2));
        assert_eq!(hunks[0].lines[3].new_line, Some(3));
    }

    #[test]
    fn parses_porcelain_states_and_renames() {
        let files = parse_status(
            b"1 M. N... 100644 100644 100644 aaaaaaa bbbbbbb staged.txt\0\
              1 .M N... 100644 100644 100644 aaaaaaa bbbbbbb unstaged.txt\0\
              2 R. N... 100644 100644 100644 aaaaaaa bbbbbbb R100 new.txt\0old.txt\0\
              ? untracked.txt\0",
            false,
        )
        .expect("status parses");
        assert_eq!(files.len(), 4);
        assert!(files[0].staged);
        assert!(files[1].unstaged);
        assert_eq!(files[2].state, RepositoryFileState::Renamed);
        assert_eq!(files[2].previous_path.as_deref(), Some("old.txt"));
        assert_eq!(files[3].state, RepositoryFileState::Untracked);
    }

    #[test]
    fn classifies_conflicts_before_ordinary_changes() {
        assert_eq!(
            repository_file_state('U', 'U'),
            RepositoryFileState::Conflicted
        );
        assert_eq!(repository_file_state('A', '.'), RepositoryFileState::Added);
        assert_eq!(
            repository_file_state('.', 'D'),
            RepositoryFileState::Deleted
        );
    }

    #[tokio::test]
    async fn lists_browse_and_review_metadata_with_bounded_processes() {
        let fixture = fixture(true, "lists").await;
        fs::write(fixture.repository_path.join("untracked.txt"), "new\n").expect("untracked");
        fs::write(fixture.repository_path.join("staged.txt"), "staged\n").expect("staged");
        git(&fixture.repository_path, &["add", "staged.txt"]);
        fs::write(fixture.repository_path.join("unstaged.txt"), "unstaged\n").expect("unstaged");
        fs::write(fixture.repository_path.join("both.txt"), "staged part\n").expect("both staged");
        git(&fixture.repository_path, &["add", "both.txt"]);
        fs::write(fixture.repository_path.join("both.txt"), "unstaged part\n")
            .expect("both unstaged");
        fs::remove_file(fixture.repository_path.join("deleted.txt")).expect("delete");
        git(&fixture.repository_path, &["mv", "old.txt", "new.txt"]);

        let before = fixture.service.git_process_count();
        let browse = fixture
            .service
            .list(
                &fixture.project.id,
                &fixture.repository.id,
                RepositoryFileMode::Browse,
            )
            .await
            .expect("browse");
        assert_eq!(fixture.service.git_process_count() - before, 2);
        assert_eq!(browse.root_path, fixture.repository.root_path);
        assert!(browse.files.iter().any(|file| {
            file.path == "untracked.txt" && file.state == RepositoryFileState::Untracked
        }));

        let before = fixture.service.git_process_count();
        let review = fixture
            .service
            .list(
                &fixture.project.id,
                &fixture.repository.id,
                RepositoryFileMode::Review,
            )
            .await
            .expect("review");
        assert_eq!(fixture.service.git_process_count() - before, 1);
        let file = |path: &str| {
            review
                .files
                .iter()
                .find(|file| file.path == path)
                .expect("review file")
        };
        assert!(file("staged.txt").staged && !file("staged.txt").unstaged);
        assert!(!file("unstaged.txt").staged && file("unstaged.txt").unstaged);
        assert!(file("both.txt").staged && file("both.txt").unstaged);
        assert_eq!(file("deleted.txt").state, RepositoryFileState::Deleted);
        assert_eq!(file("new.txt").previous_path.as_deref(), Some("old.txt"));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn reads_file_states_binary_oversize_rename_and_unborn() {
        let committed = fixture(true, "reads").await;
        fs::write(committed.repository_path.join("staged.txt"), "staged\n").expect("staged");
        git(&committed.repository_path, &["add", "staged.txt"]);
        fs::write(committed.repository_path.join("unstaged.txt"), "unstaged\n").expect("unstaged");
        fs::write(committed.repository_path.join("both.txt"), "first\n").expect("both");
        git(&committed.repository_path, &["add", "both.txt"]);
        fs::write(committed.repository_path.join("both.txt"), "second\n").expect("both second");
        fs::remove_file(committed.repository_path.join("deleted.txt")).expect("deleted");
        git(&committed.repository_path, &["mv", "old.txt", "new.txt"]);
        fs::write(
            committed.repository_path.join("new.txt"),
            "base old.txt\nchanged\n",
        )
        .expect("changed rename");
        fs::write(
            committed.repository_path.join("untracked.txt"),
            "untracked\n",
        )
        .expect("untracked");
        fs::write(committed.repository_path.join("binary.bin"), [0_u8, 9, 8]).expect("binary");
        fs::write(
            committed.repository_path.join("oversized.txt"),
            "x".repeat(super::MAX_CONTENT_BYTES + 1),
        )
        .expect("oversized");

        for path in [
            "staged.txt",
            "unstaged.txt",
            "both.txt",
            "deleted.txt",
            "new.txt",
            "untracked.txt",
        ] {
            let before = committed.service.git_process_count();
            let diff = committed
                .service
                .diff(
                    &committed.project.id,
                    &committed.repository.id,
                    path,
                    (path == "new.txt").then_some("old.txt"),
                )
                .await
                .expect("diff");
            assert_eq!(
                committed.service.git_process_count() - before,
                1,
                "identity revalidation uses two Git checks and diff uses one"
            );
            assert!(!diff.hunks.is_empty(), "{path}");
            if path == "new.txt" {
                assert_eq!(diff.previous_path.as_deref(), Some("old.txt"));
            }
        }
        let binary = committed
            .service
            .diff(
                &committed.project.id,
                &committed.repository.id,
                "binary.bin",
                None,
            )
            .await
            .expect("binary diff");
        assert!(binary.binary);
        let deleted = committed
            .service
            .content(
                &committed.project.id,
                &committed.repository.id,
                "deleted.txt",
            )
            .await
            .expect("deleted content");
        assert!(deleted.unavailable);
        let oversized = committed
            .service
            .content(
                &committed.project.id,
                &committed.repository.id,
                "oversized.txt",
            )
            .await
            .expect("oversized content");
        assert!(oversized.truncated);
        assert_eq!(
            oversized.content.expect("text").len(),
            super::MAX_CONTENT_BYTES
        );
        fs::write(
            committed.repository_path.join("unicode.txt"),
            format!("{}é", "x".repeat(super::MAX_CONTENT_BYTES - 1)),
        )
        .expect("unicode oversized");
        let unicode = committed
            .service
            .content(
                &committed.project.id,
                &committed.repository.id,
                "unicode.txt",
            )
            .await
            .expect("unicode content");
        assert!(unicode.truncated);
        assert!(!unicode.binary);

        let unborn = fixture(false, "unborn").await;
        fs::write(unborn.repository_path.join("new.txt"), "one\ntwo\n").expect("unborn file");
        git(&unborn.repository_path, &["add", "new.txt"]);
        fs::write(unborn.repository_path.join("new.txt"), "one\ntwo\nthree\n")
            .expect("unborn unstaged");
        let diff = unborn
            .service
            .diff(&unborn.project.id, &unborn.repository.id, "new.txt", None)
            .await
            .expect("unborn diff");
        assert_eq!(diff.hunks[0].new_lines, 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_path_and_symlink_escape_and_identity_change() {
        let fixture = fixture(true, "security").await;
        assert!(matches!(
            fixture
                .service
                .content(&fixture.project.id, &fixture.repository.id, "../outside")
                .await,
            Err(RepositoryFilesServiceError::InvalidPath)
        ));
        let outside = fixture.temp.path().join("outside.txt");
        fs::write(&outside, "outside").expect("outside");
        std::os::unix::fs::symlink(&outside, fixture.repository_path.join("escape"))
            .expect("symlink");
        assert!(matches!(
            fixture
                .service
                .content(&fixture.project.id, &fixture.repository.id, "escape")
                .await,
            Err(RepositoryFilesServiceError::PathEscape)
        ));

        let moved = fixture.temp.path().join("moved");
        fs::rename(&fixture.repository_path, &moved).expect("move repository");
        let other = fixture.temp.path().join("other");
        init_repository(&other);
        fs::write(other.join("replacement.txt"), "replacement\n").expect("replacement file");
        git(&other, &["add", "."]);
        git_commit(&other, "replacement");
        assert!(
            Command::new("git")
                .current_dir(&other)
                .args(["worktree", "add", "--detach", "--quiet"])
                .arg(&fixture.repository_path)
                .arg("HEAD")
                .status()
                .expect("replacement worktree")
                .success()
        );
        let changed = fixture
            .service
            .content(&fixture.project.id, &fixture.repository.id, "tracked.txt")
            .await;
        assert!(
            matches!(
                changed,
                Err(RepositoryFilesServiceError::RepositoryIdentityChanged)
            ),
            "{changed:?}"
        );
    }

    #[test]
    fn caps_file_lists_and_reports_truncation() {
        let repository = ProjectRepository {
            id: "repository".to_owned(),
            project_id: "project".to_owned(),
            root_path: "/repository".to_owned(),
            git_common_dir: "/repository/.git".to_owned(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let files = (0..=super::MAX_FILES)
            .map(|index| {
                super::browse_file(format!("{index:05}.txt"), RepositoryFileState::Tracked)
            })
            .collect();
        let response =
            super::repository_files(&repository, RepositoryFileMode::Browse, files, false);
        assert_eq!(response.files.len(), super::MAX_FILES);
        assert!(response.truncated);
    }

    #[tokio::test]
    async fn rejects_cross_project_repository_ids() {
        let fixture = fixture(true, "scope").await;
        let other = create_project(&fixture.store, "other").await;
        assert!(matches!(
            fixture
                .service
                .list(
                    &other.id,
                    &fixture.repository.id,
                    RepositoryFileMode::Browse
                )
                .await,
            Err(RepositoryFilesServiceError::Project(
                ProjectServiceError::Store(
                    yard_store::ProjectStoreError::ProjectRepositoryNotFound
                )
            ))
        ));
    }

    #[tokio::test]
    async fn lists_two_hundred_changes_under_budget() {
        let fixture = fixture(true, "performance").await;
        for index in 0..200 {
            fs::write(
                fixture.repository_path.join(format!("perf-{index:03}.txt")),
                "base\n",
            )
            .expect("performance file");
        }
        git(&fixture.repository_path, &["add", "."]);
        git_commit(&fixture.repository_path, "performance files");
        for index in 0..200 {
            fs::write(
                fixture.repository_path.join(format!("perf-{index:03}.txt")),
                "changed\n",
            )
            .expect("changed performance file");
        }
        let started = Instant::now();
        let files = fixture
            .service
            .list(
                &fixture.project.id,
                &fixture.repository.id,
                RepositoryFileMode::Review,
            )
            .await
            .expect("review list");
        assert_eq!(files.files.len(), 200);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "200-file review list took {:?}",
            started.elapsed()
        );
    }
}

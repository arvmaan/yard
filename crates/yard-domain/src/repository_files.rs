use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryFileMode {
    Browse,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryFileState {
    Tracked,
    Untracked,
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub state: RepositoryFileState,
    pub staged: bool,
    pub unstaged: bool,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryFiles {
    pub repository_id: String,
    pub root_path: String,
    pub mode: RepositoryFileMode,
    pub files: Vec<RepositoryFile>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryFileContent {
    pub repository_id: String,
    pub root_path: String,
    pub path: String,
    pub content: Option<String>,
    pub binary: bool,
    pub truncated: bool,
    pub unavailable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryDiffLineKind {
    Context,
    Addition,
    Deletion,
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDiffLine {
    pub kind: RepositoryDiffLineKind,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDiffHunk {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<RepositoryDiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDiff {
    pub repository_id: String,
    pub root_path: String,
    pub path: String,
    pub previous_path: Option<String>,
    pub hunks: Vec<RepositoryDiffHunk>,
    pub binary: bool,
    pub truncated: bool,
    pub unavailable: bool,
}

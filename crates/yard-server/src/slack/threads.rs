//! One DM thread per project, remembered across restarts.
//!
//! Thread timestamps are stored in a small JSON file next to the database
//! (`slack-threads.json`, mode 0600, written atomically), not in SQLite: a
//! schema migration here would collide with the in-flight `PR2c` migrations
//! (0032/0033) and would make the database unreadable by an older Yard
//! binary just for enabling notifications. Entries are keyed by Slack team
//! and DM channel, so a sandbox and a production install never share a
//! thread. The file holds no secrets; losing it only starts new threads.

use std::{
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tracing::warn;

const MAX_THREADS: usize = 512;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ThreadEntry {
    team_id: String,
    channel_id: String,
    key: String,
    ts: String,
    created_at_unix_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ThreadFile {
    version: u32,
    threads: Vec<ThreadEntry>,
}

#[derive(Debug, Default)]
pub struct ThreadStore {
    path: Option<PathBuf>,
    threads: Vec<ThreadEntry>,
}

impl ThreadStore {
    /// Load `path`; a missing, unreadable or foreign file starts empty.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let threads = match read_file(&path) {
            Ok(threads) => threads,
            Err(reason) => {
                if path.exists() {
                    warn!(path = %path.display(), %reason, "Ignoring the Slack thread file");
                }
                Vec::new()
            }
        };
        Self {
            path: Some(path),
            threads,
        }
    }

    #[must_use]
    pub fn get(&self, team_id: &str, channel_id: &str, key: &str) -> Option<&str> {
        self.threads
            .iter()
            .find(|entry| {
                entry.team_id == team_id && entry.channel_id == channel_id && entry.key == key
            })
            .map(|entry| entry.ts.as_str())
    }

    pub fn put(&mut self, team_id: &str, channel_id: &str, key: &str, ts: &str, now_unix_ms: u64) {
        self.threads.retain(|entry| {
            !(entry.team_id == team_id && entry.channel_id == channel_id && entry.key == key)
        });
        self.threads.push(ThreadEntry {
            team_id: team_id.to_owned(),
            channel_id: channel_id.to_owned(),
            key: key.to_owned(),
            ts: ts.to_owned(),
            created_at_unix_ms: now_unix_ms,
        });
        if self.threads.len() > MAX_THREADS {
            self.threads.sort_by_key(|entry| entry.created_at_unix_ms);
            let excess = self.threads.len() - MAX_THREADS;
            self.threads.drain(..excess);
        }
        self.persist();
    }

    pub fn forget(&mut self, team_id: &str, channel_id: &str, key: &str) {
        let before = self.threads.len();
        self.threads.retain(|entry| {
            !(entry.team_id == team_id && entry.channel_id == channel_id && entry.key == key)
        });
        if self.threads.len() != before {
            self.persist();
        }
    }

    fn persist(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Err(error) = write_file(path, &self.threads) {
            warn!(
                path = %path.display(),
                %error,
                "Could not save Slack threads; new threads may start after a restart"
            );
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<ThreadEntry>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.kind().to_string())?;
    if !metadata.file_type().is_file() {
        return Err("not a regular file".to_owned());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err("too large".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| error.kind().to_string())?;
    let file: ThreadFile =
        serde_json::from_slice(&bytes).map_err(|_| "not valid JSON".to_owned())?;
    if file.version != FILE_VERSION {
        return Err(format!("unsupported version {}", file.version));
    }
    Ok(file.threads)
}

fn write_file(path: &Path, threads: &[ThreadEntry]) -> std::io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err(std::io::Error::other(
            "the thread file is not a regular file",
        ));
    }
    let bytes = serde_json::to_vec_pretty(&ThreadFile {
        version: FILE_VERSION,
        threads: threads.to_vec(),
    })
    .map_err(std::io::Error::other)?;
    let file_name = path.file_name().map_or_else(
        || "slack-threads.json".into(),
        |name| name.to_string_lossy(),
    );
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Where the thread file lives for a database.
#[must_use]
pub fn thread_file_path(database_path: &Path) -> PathBuf {
    database_path.with_file_name("slack-threads.json")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::{ThreadStore, thread_file_path};

    #[test]
    fn threads_survive_reload_per_team_and_channel() {
        let temp = TempDir::new().unwrap();
        let path = thread_file_path(&temp.path().join("yard.sqlite3"));
        let mut store = ThreadStore::load(path.clone());
        store.put("T1", "D1", "project:p-1", "1700000000.000100", 1);
        store.put("T1", "D1", "yard", "1700000000.000200", 2);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let reloaded = ThreadStore::load(path.clone());
        assert_eq!(
            reloaded.get("T1", "D1", "project:p-1"),
            Some("1700000000.000100")
        );
        assert_eq!(reloaded.get("T2", "D1", "project:p-1"), None);
        assert_eq!(reloaded.get("T1", "D9", "project:p-1"), None);

        let mut reloaded = reloaded;
        reloaded.forget("T1", "D1", "project:p-1");
        assert_eq!(ThreadStore::load(path).get("T1", "D1", "project:p-1"), None);
    }

    #[test]
    fn corrupt_or_symlinked_files_start_empty() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("slack-threads.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(
            ThreadStore::load(path.clone())
                .get("T", "D", "yard")
                .is_none()
        );

        let target = temp.path().join("elsewhere.json");
        std::fs::write(&target, br#"{"version":1,"threads":[]}"#).unwrap();
        let link = temp.path().join("linked.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut store = ThreadStore::load(link.clone());
        store.put("T", "D", "yard", "1.2", 1);
        // The symlink target is never overwritten.
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            r#"{"version":1,"threads":[]}"#
        );
    }
}

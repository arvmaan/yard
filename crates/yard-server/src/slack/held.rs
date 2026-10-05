//! What the quiet policy keeps across restarts, in small JSON files beside
//! the database (no migration, same rules as `slack-threads.json`: mode
//! 0600, written atomically, a missing or foreign file starts empty):
//! - `slack-held.json`: notifications held for the next digest.
//! - `slack-preferences.json`: the owner's mute.
//!
//! Neither holds secrets or terminal content; a held event carries only the
//! titles a notification card would show ([`trimmed`]: the objective's
//! first line, redacted and truncated), which also keeps the file small.

use std::{
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::warn;

use super::{
    detector::{AttentionEvent, Subject},
    message::held_title,
};

pub const HELD_FILE: &str = "slack-held.json";
pub const PREFERENCES_FILE: &str = "slack-preferences.json";
const FILE_VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Held notifications kept at most; the oldest go first (and are counted).
pub const MAX_HELD: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldEntry {
    pub event: AttentionEvent,
    /// Debounce key (worker + kind, or the command).
    pub key: String,
    pub held_at_unix_ms: u64,
    /// Held outside working hours or while muted: it goes into the "While
    /// you were away" digest at the start of the next working window.
    pub away: bool,
}

/// `event` as it may be held: each objective reduced to the title a card
/// would show, so no full task text is written to disk.
#[must_use]
pub fn trimmed(mut event: AttentionEvent) -> AttentionEvent {
    if let Subject::Worker { objective, .. } | Subject::Command { objective, .. } =
        &mut event.subject
    {
        *objective = objective.as_deref().and_then(held_title);
    }
    event
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldState {
    pub entries: Vec<HeldEntry>,
    /// Held entries dropped for space since the last digest.
    #[serde(default)]
    pub dropped: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    /// Unix ms; everything is held until then.
    #[serde(default)]
    pub muted_until_unix_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Versioned<T> {
    version: u32,
    #[serde(flatten)]
    body: T,
}

/// A JSON document at an optional path (`None`: in memory only).
#[derive(Debug, Default)]
pub struct JsonFile<T> {
    path: Option<PathBuf>,
    pub value: T,
}

impl<T: Serialize + DeserializeOwned + Default> JsonFile<T> {
    #[must_use]
    pub fn load(path: Option<PathBuf>) -> Self {
        let value = path.as_deref().map_or_else(T::default, |path| {
            read(path).unwrap_or_else(|reason| {
                if path.exists() {
                    warn!(path = %path.display(), %reason, "Ignoring a Slack state file");
                }
                T::default()
            })
        });
        Self { path, value }
    }

    /// Write the current value (failures are logged; the value stays in
    /// memory).
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Err(error) = write(path, &self.value) {
            warn!(path = %path.display(), %error, "Could not save a Slack state file");
        }
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.kind().to_string())?;
    if !metadata.file_type().is_file() {
        return Err("not a regular file".to_owned());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err("too large".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| error.kind().to_string())?;
    let file: Versioned<T> =
        serde_json::from_slice(&bytes).map_err(|_| "not valid JSON".to_owned())?;
    if file.version != FILE_VERSION {
        return Err(format!("unsupported version {}", file.version));
    }
    Ok(file.body)
}

fn write<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err(std::io::Error::other("not a regular file"));
    }
    let bytes = serde_json::to_vec_pretty(&Versioned {
        version: FILE_VERSION,
        body: value,
    })
    .map_err(std::io::Error::other)?;
    let name = path
        .file_name()
        .map_or_else(|| "slack-state.json".into(), |name| name.to_string_lossy());
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::now_v7()));
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

/// `slack-held.json` / `slack-preferences.json` beside the thread file.
#[must_use]
pub fn sibling(thread_file: &Path, name: &str) -> PathBuf {
    thread_file.with_file_name(name)
}

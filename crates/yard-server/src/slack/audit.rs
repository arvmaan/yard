//! Append-only record of every inbound Slack action and its outcome.
//!
//! One JSON line per event in `slack-audit.jsonl` beside the database
//! (0600, opened with `O_NOFOLLOW`, rotated to `.1` past 1 MiB). It holds
//! the Slack user id, what was asked (`button` / `reply` / `message` /
//! `dropped`), the target agent and the outcome — never message text,
//! prompt text or tokens. No migration: an older Yard binary can still open
//! the database.
//!
//! Drops (events from anyone but the owner, stale or malformed ones) go to
//! their own `slack-audit-dropped.jsonl` with the same rotation, so traffic
//! from other workspace members can never rotate the owner's action records
//! away.

use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::Serialize;
use tracing::{info, warn};

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditRecord {
    pub at_unix_ms: u64,
    pub slack_user: String,
    /// `button`, `reply`, `message`, `dropped`.
    pub kind: &'static str,
    /// `allow`, `deny`, `choice`, `free_text`, `command`, or a drop reason.
    pub action: String,
    /// `assignment:<project>/<assignment>`, `orchestrator:<project>`, …
    pub target: Option<String>,
    /// `sent`, `refused:<reason>`, `dropped`, …
    pub outcome: String,
}

#[derive(Debug, Default)]
pub struct AuditLog {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

#[must_use]
pub fn audit_file_path(database_path: &Path) -> PathBuf {
    database_path.with_file_name("slack-audit.jsonl")
}

impl AuditLog {
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Log and append one record. A write failure is logged, never fatal.
    pub fn record(&self, record: &AuditRecord) {
        info!(
            slack_user = %record.slack_user,
            kind = record.kind,
            action = %record.action,
            target = record.target.as_deref().unwrap_or("-"),
            outcome = %record.outcome,
            "Slack inbound"
        );
        let Some(path) = &self.path else {
            return;
        };
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = if record.kind == "dropped" {
            dropped_path(path)
        } else {
            path.clone()
        };
        if let Err(error) = append(&path, record) {
            warn!(%error, "Could not append to the Slack audit file");
        }
    }
}

/// `slack-audit.jsonl` → `slack-audit-dropped.jsonl`.
#[must_use]
pub fn dropped_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(|| "slack-audit".into(), |stem| stem.to_string_lossy());
    path.with_file_name(format!("{stem}-dropped.jsonl"))
}

fn append(path: &Path, record: &AuditRecord) -> std::io::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::other("refusing a symlinked audit file"));
        }
        if metadata.len() > MAX_BYTES {
            std::fs::rename(path, path.with_extension("jsonl.1"))?;
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let mut line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    file.write_all(&line)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::{AuditLog, AuditRecord};

    fn record(outcome: &str) -> AuditRecord {
        AuditRecord {
            at_unix_ms: 1,
            slack_user: "U01OWNER".to_owned(),
            kind: "button",
            action: "allow".to_owned(),
            target: Some("assignment:p/a".to_owned()),
            outcome: outcome.to_owned(),
        }
    }

    #[test]
    fn appends_private_json_lines_and_refuses_symlinks() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("slack-audit.jsonl");
        let log = AuditLog::new(Some(path.clone()));
        log.record(&record("sent"));
        log.record(&record("refused:prompt_changed"));
        let text = std::fs::read_to_string(&path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["slack_user"], "U01OWNER");
        assert_eq!(first["outcome"], "sent");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let target = temp.path().join("elsewhere");
        std::fs::write(&target, "").unwrap();
        let link = temp.path().join("link.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        AuditLog::new(Some(link)).record(&record("sent"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
    }

    #[test]
    fn dropped_events_never_rotate_the_owner_records_away() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("slack-audit.jsonl");
        let log = AuditLog::new(Some(path.clone()));
        log.record(&record("sent"));
        let mut foreign = record("dropped");
        foreign.kind = "dropped";
        foreign.slack_user = "U0STRANGER".to_owned();
        foreign.action = "not_the_owner_dm".to_owned();
        // Well past two rotations' worth of foreign drops.
        let line = serde_json::to_vec(&foreign).unwrap().len() as u64 + 1;
        for _ in 0..=(2 * super::MAX_BYTES / line + 2) {
            log.record(&foreign);
        }
        let owner = std::fs::read_to_string(&path).unwrap();
        assert_eq!(owner.lines().count(), 1, "owner record kept");
        assert!(owner.contains("\"outcome\":\"sent\""));
        let dropped = super::dropped_path(&path);
        assert!(dropped.ends_with("slack-audit-dropped.jsonl"));
        assert!(
            std::fs::read_to_string(&dropped)
                .unwrap()
                .contains("U0STRANGER")
        );
        let mode = std::fs::metadata(&dropped).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

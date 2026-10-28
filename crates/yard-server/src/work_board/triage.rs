//! The user's Keep / Skip decisions from "Review older work", kept in a
//! small JSON file beside Yard's other state files (no schema migration).
//! Keep pins a card to the main board; Skip marks it reviewed until it has
//! new activity. Done and Archive go through the existing guarded routes
//! and dialogs, so they are not stored here.

use std::{
    collections::HashMap,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};

use super::board::Decision;

/// The file beside the knowledge directory: `<managed root>/work-board-triage.json`.
pub(crate) const TRIAGE_FILE: &str = "work-board-triage.json";
/// Decisions kept (the oldest are dropped past this).
pub(crate) const MAX_DECISIONS: usize = 1_000;
/// Largest file read.
const MAX_FILE_BYTES: u64 = 512 * 1024;
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Choice {
    Keep,
    Skip,
    /// Forget the decision (unpin, or offer it for review again).
    Clear,
}

#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    version: u32,
    decisions: Vec<SavedDecision>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedDecision {
    item_id: String,
    keep: bool,
    at_unix_ms: u64,
}

/// Decisions in memory, written through to the file (when there is one).
pub(crate) struct TriageStore {
    file: Option<PathBuf>,
    decisions: Mutex<HashMap<String, Decision>>,
}

/// An item id the board makes (`a:`, `w:`, `o:`, `l:` + an id): bounded,
/// printable, no whitespace.
pub(crate) fn valid_item_id(id: &str) -> bool {
    let Some((kind, rest)) = id.split_once(':') else {
        return false;
    };
    matches!(kind, "a" | "w" | "o" | "l")
        && !rest.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

impl TriageStore {
    /// Load `file` (missing, unreadable, oversized or foreign content: start
    /// empty; the file is rewritten on the next decision).
    pub(crate) fn load(file: Option<PathBuf>) -> Self {
        let decisions = file.as_deref().and_then(read_file).unwrap_or_default();
        Self {
            file,
            decisions: Mutex::new(decisions),
        }
    }

    pub(crate) fn snapshot(&self) -> HashMap<String, Decision> {
        self.decisions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Record a choice and write the file.
    ///
    /// # Errors
    ///
    /// The write error (the in-memory decision is kept).
    pub(crate) fn decide(&self, item_id: &str, choice: Choice, now_ms: u64) -> std::io::Result<()> {
        let bytes = {
            let mut decisions = self
                .decisions
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match choice {
                Choice::Clear => {
                    decisions.remove(item_id);
                }
                Choice::Keep | Choice::Skip => {
                    decisions.insert(
                        item_id.to_owned(),
                        Decision {
                            keep: choice == Choice::Keep,
                            at_ms: now_ms,
                        },
                    );
                }
            }
            if decisions.len() > MAX_DECISIONS {
                let mut by_age = decisions
                    .iter()
                    .map(|(id, decision)| (decision.at_ms, id.clone()))
                    .collect::<Vec<_>>();
                by_age.sort();
                for (_, id) in by_age.into_iter().take(decisions.len() - MAX_DECISIONS) {
                    decisions.remove(&id);
                }
            }
            let mut saved = decisions
                .iter()
                .map(|(id, decision)| SavedDecision {
                    item_id: id.clone(),
                    keep: decision.keep,
                    at_unix_ms: decision.at_ms,
                })
                .collect::<Vec<_>>();
            saved.sort_by(|left, right| left.item_id.cmp(&right.item_id));
            serde_json::to_vec_pretty(&Saved {
                version: FILE_VERSION,
                decisions: saved,
            })
            .map_err(std::io::Error::other)?
        };
        let Some(path) = self.file.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        crate::slack::held::write_bytes(path, &bytes)
    }
}

fn read_file(path: &Path) -> Option<HashMap<String, Decision>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let saved: Saved = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if saved.version != FILE_VERSION {
        return None;
    }
    Some(
        saved
            .decisions
            .into_iter()
            .filter(|decision| valid_item_id(&decision.item_id))
            .take(MAX_DECISIONS)
            .map(|decision| {
                (
                    decision.item_id,
                    Decision {
                        keep: decision.keep,
                        at_ms: decision.at_unix_ms,
                    },
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_survive_a_restart_and_clear_removes_them() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(TRIAGE_FILE);
        let store = TriageStore::load(Some(file.clone()));
        store.decide("w:one", Choice::Keep, 10).unwrap();
        store.decide("l:s:w1:p2", Choice::Skip, 20).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let again = TriageStore::load(Some(file.clone()));
        let decisions = again.snapshot();
        assert_eq!(
            decisions["w:one"],
            Decision {
                keep: true,
                at_ms: 10
            }
        );
        assert_eq!(
            decisions["l:s:w1:p2"],
            Decision {
                keep: false,
                at_ms: 20
            }
        );
        again.decide("w:one", Choice::Clear, 30).unwrap();
        assert!(
            !TriageStore::load(Some(file))
                .snapshot()
                .contains_key("w:one")
        );
    }

    #[test]
    fn foreign_or_broken_files_start_empty() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(TRIAGE_FILE);
        std::fs::write(&file, b"{not json").unwrap();
        assert!(TriageStore::load(Some(file.clone())).snapshot().is_empty());
        std::fs::write(
            &file,
            br#"{"version":1,"decisions":[{"item_id":"x:bad id","keep":true,"at_unix_ms":1}]}"#,
        )
        .unwrap();
        assert!(TriageStore::load(Some(file)).snapshot().is_empty());
    }

    #[test]
    fn only_board_item_ids_are_accepted() {
        assert!(valid_item_id("a:01a1-2"));
        assert!(valid_item_id("l:yard-orchestrator:w1:pN"));
        for bad in [
            "",
            "a:",
            "x:1",
            "a:has space",
            "w:\"q\"",
            &format!("w:{}", "x".repeat(300)),
        ] {
            assert!(!valid_item_id(bad), "{bad}");
        }
    }

    #[test]
    fn the_oldest_decisions_are_dropped_past_the_cap() {
        let store = TriageStore::load(None);
        for index in 0..=MAX_DECISIONS {
            store
                .decide(&format!("w:{index}"), Choice::Skip, index as u64)
                .unwrap();
        }
        let decisions = store.snapshot();
        assert_eq!(decisions.len(), MAX_DECISIONS);
        assert!(!decisions.contains_key("w:0"));
    }
}

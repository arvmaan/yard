//! The dedicated Slack Superintendent: a Yard workstream node (shared
//! `yard-coordination` session, own pane and provider session) that only
//! answers the owner's Slack conversations, so Slack chat never queues
//! behind or disrupts the main Superintendent's work.
//!
//! Started lazily on the first plain Slack question, re-used afterwards,
//! archived and started again when its worker is gone. It is matched by
//! [`SLACK_SUPERINTENDENT_ACTOR`] (`created_by`), never by name, so an
//! owner's node with the same name is never adopted. Its bootstrap prompt
//! is [`SLACK_SUPERINTENDENT_BRIEF`]; its full answers are written to
//! `slack-answers/<key>.md` in its backend-owned folder ([`read_answer`]).

use std::{fs, io::Read, os::fd::OwnedFd, path::Path, sync::Arc, time::Duration};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use uuid::Uuid;
use yard_domain::{
    ArchiveCoordinationNode, CanvasPlacement, CoordinationNode, CoordinationNodeKind,
    CreateCoordinationNode, ProvisionCoordinationNode, RuntimeObservationState,
    RuntimeProcessState, YardOrchestrator,
};
use yard_store::YardStore;

use crate::coordination_node_service::{CoordinationNodeService, CoordinationNodeServiceError};

/// `created_by` (and every command's actor) of the node.
pub const SLACK_SUPERINTENDENT_ACTOR: &str = "slack:superintendent";
pub const SLACK_SUPERINTENDENT_NAME: &str = "Slack Superintendent";
/// How Slack names it.
pub const SLACK_SUPERINTENDENT_TO: &str = "the Slack Superintendent";
pub const SLACK_SUPERINTENDENT_ROLE: &str = "Slack chat assistant";
/// The bootstrap objective (instead of the workstream objective).
pub const SLACK_SUPERINTENDENT_BRIEF: &str = "You are Yard's Slack Superintendent: the Yard owner's conversational assistant in Slack. Each owner message is a question or an instruction. Answer questions from Yard state, read with the same tools and HTTP API the Superintendent uses (projects, workers, assignments, coordination nodes, status). When the owner asks you to do something, do it through Yard's normal guarded routes, the same ones the Superintendent uses: assign work, prompt a project's orchestrator, report status, or hand it to the main Superintendent. Never type into terminals directly and never edit Yard's files or database. Before anything destructive (archiving, deleting, stopping or discarding work) ask the owner to confirm in the thread, and act only after they say yes. Reply conversationally and briefly, saying what you did or started. Each Slack thread is its own conversation: use only the context Yard sends with each message. Write each reply to the file Yard names, then emit the status line.";
/// Folder (relative to the node's cwd) for answer files.
pub const ANSWER_DIR: &str = "slack-answers";
/// An answer file larger than this is not read.
pub const MAX_ANSWER_BYTES: u64 = 64 * 1024;
/// Answer files kept; older ones are removed.
pub const KEEP_ANSWER_FILES: usize = 32;
/// How long a freshly provisioned worker may take to become reachable.
pub const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(500);
/// Where the node first appears on the canvas.
const PLACEMENT: CanvasPlacement = CanvasPlacement {
    x: -480.0,
    y: -320.0,
    width: 300.0,
    height: 180.0,
};

/// The answer file for `key`, relative to the agent's cwd.
#[must_use]
pub fn answer_file(key: &str) -> String {
    format!("{ANSWER_DIR}/{key}.md")
}

/// A fresh answer key (a Yard-generated UUID).
#[must_use]
pub fn new_answer_key() -> String {
    Uuid::now_v7().to_string()
}

/// Only Yard-generated keys name a file: hex digits and dashes.
fn valid_key(key: &str) -> bool {
    (1..=64).contains(&key.len()) && key.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The answer folder, opened without following a symlink (as the last
/// component); every file operation below is relative to this handle, so
/// swapping the folder or a file for a symlink later changes nothing.
fn open_answer_dir(cwd: &Path) -> Option<OwnedFd> {
    rustix::fs::open(
        cwd.join(ANSWER_DIR),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()
}

/// A regular file `name` directly in `dir` (lstat, no symlinks).
fn regular_at(dir: &OwnedFd, name: &str) -> Option<rustix::fs::Stat> {
    rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .filter(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile)
}

/// The answer the agent wrote for `key`: a regular file (no symlinks) of
/// at most [`MAX_ANSWER_BYTES`] of UTF-8 in `cwd/slack-answers/`, trimmed;
/// `None` for anything else. `cwd` must already be validated.
#[must_use]
pub fn read_answer(cwd: &Path, key: &str) -> Option<String> {
    if !valid_key(key) {
        return None;
    }
    let dir = open_answer_dir(cwd)?;
    // NOFOLLOW: a symlink is refused at open; NONBLOCK: a FIFO never hangs
    // the read (and is refused below as not a regular file).
    let file = rustix::fs::openat(
        &dir,
        format!("{key}.md"),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let file = fs::File::from(file);
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_ANSWER_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_ANSWER_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ANSWER_BYTES {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// After the answer was relayed: remove its file, then keep only the
/// newest [`KEEP_ANSWER_FILES`] regular `.md` files (all relative to the
/// opened folder; a symlink is never followed, only ever left alone).
pub fn forget_answer(cwd: &Path, key: &str) {
    let Some(dir) = open_answer_dir(cwd) else {
        return;
    };
    if valid_key(key) {
        let name = format!("{key}.md");
        if regular_at(&dir, &name).is_some() {
            let _ = rustix::fs::unlinkat(&dir, name.as_str(), AtFlags::empty());
        }
    }
    let Ok(listing) = rustix::fs::Dir::read_from(&dir) else {
        return;
    };
    let mut files = listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str().ok()?.to_owned();
            let stat = name
                .strip_suffix(".md")
                .filter(|stem| !stem.is_empty())
                .and_then(|_| regular_at(&dir, &name))?;
            Some(((stat.st_mtime, stat.st_mtime_nsec), name))
        })
        .collect::<Vec<_>>();
    if files.len() <= KEEP_ANSWER_FILES {
        return;
    }
    files.sort();
    let excess = files.len() - KEEP_ANSWER_FILES;
    for (_, name) in files.into_iter().take(excess) {
        let _ = rustix::fs::unlinkat(&dir, name.as_str(), AtFlags::empty());
    }
}

/// The worker profile to start it with: the main Superintendent's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRef {
    pub id: String,
    pub version: u64,
}

/// Finds, re-uses, recovers or starts the node. One at a time, so two
/// first questions create one node.
pub struct SlackSuperintendent {
    nodes: CoordinationNodeService,
    store: Arc<dyn YardStore>,
    operation: tokio::sync::Mutex<()>,
    ready_timeout: Duration,
}

/// What a health check found.
enum Health {
    Healthy,
    /// Positively gone: archive and start a new one.
    Dead,
    /// Could not tell (a read failed): leave it alone this turn.
    Unknown(String),
}

/// Whether the bound agent process still runs in an observed pane.
fn running(node: &CoordinationNode) -> bool {
    node.worker
        .as_ref()
        .and_then(|worker| worker.runtime.as_ref())
        .is_some_and(|runtime| {
            runtime.process_state != RuntimeProcessState::Exited
                && runtime.observation_state != RuntimeObservationState::Missing
        })
}

impl SlackSuperintendent {
    #[must_use]
    pub fn new(nodes: CoordinationNodeService, store: Arc<dyn YardStore>) -> Self {
        Self {
            nodes,
            store,
            operation: tokio::sync::Mutex::new(()),
            ready_timeout: READY_TIMEOUT,
        }
    }

    /// A short readiness wait (tests).
    #[must_use]
    pub fn with_ready_timeout(mut self, ready_timeout: Duration) -> Self {
        self.ready_timeout = ready_timeout;
        self
    }

    /// The main Superintendent's worker profile, at its current version.
    ///
    /// # Errors
    ///
    /// A reason for the owner when there is none to copy.
    pub async fn profile(&self) -> Result<ProfileRef, String> {
        let orchestrator = self
            .store
            .get_yard_orchestrator()
            .await
            .map_err(|error| format!("Yard could not read the Superintendent ({error})"))?;
        self.profile_of(orchestrator).await
    }

    /// `orchestrator`'s worker profile, at the profile's current version.
    pub(crate) async fn profile_of(
        &self,
        orchestrator: YardOrchestrator,
    ) -> Result<ProfileRef, String> {
        let profile_id = orchestrator
            .worker
            .and_then(|worker| worker.profile_id)
            .ok_or_else(|| "the Superintendent has no worker profile to copy".to_owned())?;
        let profile = self
            .store
            .get_worker_profile(&profile_id)
            .await
            .map_err(|error| {
                format!("Yard could not read the Superintendent's profile ({error})")
            })?;
        Ok(ProfileRef {
            id: profile.id,
            version: profile.version,
        })
    }

    /// The node's id once its worker is reachable, starting it if needed.
    ///
    /// # Errors
    ///
    /// A short reason (shown to the owner) when it can't start.
    pub async fn ensure(&self) -> Result<String, String> {
        let _operation = self.operation.lock().await;
        if let Some(node) = self.reusable().await? {
            return Ok(node);
        }
        let profile = self.profile().await?;
        self.start(&profile).await
    }

    /// A working node instead of `stale`, which Slack found unusable (gone,
    /// not running, or its terminal showing a lost session): while it is
    /// still the newest node it is archived (its worker ended) and a new one
    /// started; when another message already replaced it, that one is
    /// used.
    ///
    /// # Errors
    ///
    /// As [`Self::ensure`].
    pub async fn replace(&self, stale: &str) -> Result<String, String> {
        let _operation = self.operation.lock().await;
        match self.find().await? {
            Some(node) if node.id == stale => {
                let current = self.nodes.get(&node.id).await.unwrap_or(node);
                let archive = ArchiveCoordinationNode {
                    command_id: Uuid::now_v7().to_string(),
                    actor: SLACK_SUPERINTENDENT_ACTOR.to_owned(),
                    expected_node_version: current.version,
                    expected_worker_version: None,
                };
                self.nodes
                    .archive(&current.id, archive)
                    .await
                    .map_err(|error| format!("its old worker could not be retired ({error})"))?;
            }
            _ => {
                if let Some(node) = self.reusable().await? {
                    return Ok(node);
                }
            }
        }
        let profile = self.profile().await?;
        self.start(&profile).await
    }

    /// [`Self::ensure`] with a given profile (tests).
    ///
    /// # Errors
    ///
    /// As [`Self::ensure`].
    pub async fn ensure_with(&self, profile: &ProfileRef) -> Result<String, String> {
        let _operation = self.operation.lock().await;
        if let Some(node) = self.reusable().await? {
            return Ok(node);
        }
        self.start(profile).await
    }

    /// The newest active node this module created.
    async fn find(&self) -> Result<Option<CoordinationNode>, String> {
        let nodes = self
            .nodes
            .list()
            .await
            .map_err(|error| format!("Yard could not list workstreams ({error})"))?;
        Ok(nodes
            .nodes
            .into_iter()
            .filter(|node| {
                node.created_by == SLACK_SUPERINTENDENT_ACTOR
                    && node.kind == CoordinationNodeKind::Workstream
                    && node.attached_project_ids.is_empty()
            })
            .max_by_key(|node| node.created_at_unix_ms))
    }

    /// Whether `node`'s worker answers. Only positive evidence counts as
    /// [`Health::Dead`]: no worker, an exited or missing process, or a
    /// binding a fresh inventory still contradicts after one re-read (a
    /// newly reported provider session is reconciled by the first check).
    /// A failed read (store, Herdr) is [`Health::Unknown`].
    async fn health(&self, node: &CoordinationNode) -> Health {
        let mut node = node.clone();
        for attempt in 0..2 {
            if node.worker.is_none() || !running(&node) {
                return Health::Dead;
            }
            match self.nodes.validate_node_binding(&node).await {
                Ok(_) => return Health::Healthy,
                Err(
                    CoordinationNodeServiceError::NodeNotProvisioned
                    | CoordinationNodeServiceError::RuntimeBindingMissing,
                ) => return Health::Dead,
                Err(
                    error @ (CoordinationNodeServiceError::RuntimeBindingStale
                    | CoordinationNodeServiceError::NodeChanged),
                ) => {
                    if attempt == 1 {
                        return match error {
                            CoordinationNodeServiceError::RuntimeBindingStale => Health::Dead,
                            _ => Health::Unknown(error.to_string()),
                        };
                    }
                    // Re-read: the node changed, or its binding was just
                    // reconciled; check the current one once more.
                    match self.nodes.get(&node.id).await {
                        Ok(current) => node = current,
                        Err(error) => return Health::Unknown(error.to_string()),
                    }
                }
                Err(error) => return Health::Unknown(error.to_string()),
            }
        }
        Health::Unknown("its state kept changing".to_owned())
    }

    /// A healthy node to re-use; a node whose worker is gone is archived
    /// (once) so a new one is started. When its state can't be read, an
    /// error: this turn falls back, the node is left alone.
    async fn reusable(&self) -> Result<Option<String>, String> {
        let Some(node) = self.find().await? else {
            return Ok(None);
        };
        match self.health(&node).await {
            Health::Healthy => return Ok(Some(node.id)),
            Health::Unknown(reason) => {
                return Err(format!("Yard could not check its worker ({reason})"));
            }
            Health::Dead => {}
        }
        // Archive the version just checked (a concurrent change fails it).
        let current = self.nodes.get(&node.id).await.unwrap_or(node);
        let archive = ArchiveCoordinationNode {
            command_id: Uuid::now_v7().to_string(),
            actor: SLACK_SUPERINTENDENT_ACTOR.to_owned(),
            expected_node_version: current.version,
            expected_worker_version: None,
        };
        self.nodes
            .archive(&current.id, archive)
            .await
            .map_err(|error| format!("its old worker could not be retired ({error})"))?;
        Ok(None)
    }

    async fn start(&self, profile: &ProfileRef) -> Result<String, String> {
        let created = self
            .nodes
            .create_slack_superintendent(CreateCoordinationNode {
                command_id: Uuid::now_v7().to_string(),
                actor: SLACK_SUPERINTENDENT_ACTOR.to_owned(),
                name: SLACK_SUPERINTENDENT_NAME.to_owned(),
                kind: CoordinationNodeKind::Workstream,
                placement: PLACEMENT,
                attached_project_ids: Vec::new(),
            })
            .await
            .map_err(|error| format!("Yard could not create it ({error})"))?;
        let node_id = created.node.id;
        self.nodes
            .provision(
                &node_id,
                ProvisionCoordinationNode {
                    command_id: Uuid::now_v7().to_string(),
                    actor: SLACK_SUPERINTENDENT_ACTOR.to_owned(),
                    profile_id: profile.id.clone(),
                    expected_profile_version: profile.version,
                    expected_node_version: created.node.version,
                },
            )
            .await
            .map_err(|error| format!("its worker did not start ({error})"))?;
        let deadline = tokio::time::Instant::now() + self.ready_timeout;
        loop {
            if let Ok(node) = self.nodes.get(&node_id).await
                && matches!(self.health(&node).await, Health::Healthy)
            {
                return Ok(node_id);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("its worker did not become reachable in time".to_owned());
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{KEEP_ANSWER_FILES, MAX_ANSWER_BYTES, answer_file, forget_answer, read_answer};

    const KEY: &str = "0190aaaa-bbbb-7ccc-8ddd-eeeeffff0000";

    fn dir() -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().unwrap();
        fs::create_dir(temp.path().join("slack-answers")).unwrap();
        temp
    }

    #[test]
    fn a_regular_utf8_answer_file_is_read_and_forgotten() {
        let temp = dir();
        assert_eq!(answer_file(KEY), format!("slack-answers/{KEY}.md"));
        fs::write(temp.path().join(answer_file(KEY)), "  Two tasks left.\n").unwrap();
        assert_eq!(
            read_answer(temp.path(), KEY).as_deref(),
            Some("Two tasks left.")
        );
        forget_answer(temp.path(), KEY);
        assert!(!temp.path().join(answer_file(KEY)).exists());
        assert_eq!(read_answer(temp.path(), KEY), None);
    }

    #[test]
    fn symlinks_oversize_non_utf8_and_odd_keys_are_never_read() {
        let temp = dir();
        let outside = tempfile::TempDir::new().unwrap();
        let secret = outside.path().join("secret.md");
        fs::write(&secret, "outside").unwrap();
        std::os::unix::fs::symlink(&secret, temp.path().join(answer_file(KEY))).unwrap();
        assert_eq!(read_answer(temp.path(), KEY), None, "symlinked file");
        forget_answer(temp.path(), KEY);
        assert!(secret.exists(), "a symlink target is never removed");

        let big = "0190aaaa-bbbb-7ccc-8ddd-eeeeffff0001";
        let size = usize::try_from(MAX_ANSWER_BYTES).unwrap() + 1;
        fs::write(temp.path().join(answer_file(big)), "a".repeat(size)).unwrap();
        assert_eq!(read_answer(temp.path(), big), None, "oversize");

        let binary = "0190aaaa-bbbb-7ccc-8ddd-eeeeffff0002";
        fs::write(temp.path().join(answer_file(binary)), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_answer(temp.path(), binary), None, "not UTF-8");

        fs::write(outside.path().join("x.md"), "outside").unwrap();
        assert_eq!(read_answer(temp.path(), "../x"), None, "not a Yard key");

        // A symlinked answer folder is not followed either.
        let linked = tempfile::TempDir::new().unwrap();
        fs::create_dir(outside.path().join("answers")).unwrap();
        fs::write(
            outside.path().join("answers").join(format!("{KEY}.md")),
            "x",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("answers"),
            linked.path().join("slack-answers"),
        )
        .unwrap();
        assert_eq!(read_answer(linked.path(), KEY), None, "symlinked folder");
    }

    #[test]
    fn only_the_newest_answer_files_are_kept() {
        let temp = dir();
        for index in 0..KEEP_ANSWER_FILES + 5 {
            let key = format!("0190aaaa-bbbb-7ccc-8ddd-{index:012}");
            fs::write(temp.path().join(answer_file(&key)), "answer").unwrap();
        }
        forget_answer(temp.path(), KEY);
        let left = fs::read_dir(temp.path().join("slack-answers"))
            .unwrap()
            .count();
        assert_eq!(left, KEEP_ANSWER_FILES);
    }
}

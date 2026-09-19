use std::{
    env, fmt,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf},
    process::{ExitCode, Stdio},
    sync::Arc,
    time::Duration,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, UnixListener, UnixStream},
    process::{Child, Command},
    sync::watch,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};
use uuid::Uuid;
use yard_server::config::{ServerConfig, database_path_from_env};

// Lifecycle commands trust only a same-UID peer that proves the random secret
// over this private socket. Persisted PIDs are diagnostic and never authorize a
// signal; only a retained Child handle may be terminated during failed startup.
const CONTROL_PROTOCOL: u32 = 1;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
const LAUNCH_LOCK_TIMEOUT: Duration = Duration::from_secs(40);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const BROWSER_TIMEOUT: Duration = Duration::from_secs(5);
const CHILD_TERMINATION_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_CONTROL_FRAME: u64 = 16 * 1024;
const MAX_METADATA_SIZE: u64 = 16 * 1024;
const MAX_LOG_TAIL_BYTES: u64 = 64 * 1024;
const STOPPED_EXIT_CODE: u8 = 1;

#[derive(Debug, Error)]
pub(crate) enum LifecycleError {
    #[error("{operation} '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid Yard configuration: {0}")]
    Configuration(String),
    #[error("unsafe Yard runtime path '{}': {reason}", path.display())]
    UnsafeRuntimePath { path: PathBuf, reason: String },
    #[error("timed out waiting for the Yard launch lock at '{}'", .0.display())]
    LaunchLockTimeout(PathBuf),
    #[error("another Yard process owns the lifecycle instance lock at '{}'", .0.display())]
    InstanceOwned(PathBuf),
    #[error("managed control state is in use but could not be verified: {0}")]
    ControlConflict(String),
    #[error("managed control protocol failed: {0}")]
    ControlProtocol(String),
    #[error("Yard failed to start: {0}")]
    Startup(String),
    #[error(
        "Yard did not stop within {} seconds; no PID-based signal was sent",
        STOP_TIMEOUT.as_secs()
    )]
    StopTimeout,
    #[error("yard stop refuses a foreground instance; stop it in its owning terminal")]
    ForegroundStopRefused,
    #[error("Yard is already running in {0} mode")]
    AlreadyRunning(InstanceMode),
    #[error("managed instance id is invalid: {0}")]
    InvalidInstanceId(String),
    #[error("could not generate a control secret: {0}")]
    Random(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LifecycleError {
    pub(crate) const fn exit_code(&self) -> u8 {
        match self {
            Self::Configuration(_)
            | Self::UnsafeRuntimePath { .. }
            | Self::InvalidInstanceId(_) => 2,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimePaths {
    directories: Vec<PathBuf>,
    root: PathBuf,
    control: PathBuf,
    metadata: PathBuf,
    launch_lock: PathBuf,
    instance_lock: PathBuf,
    log: PathBuf,
}

impl RuntimePaths {
    fn discover(database_path: &Path) -> Result<Self, LifecycleError> {
        let database_id = database_identity(database_path)?;
        let (mut directories, namespace) = if let Some(path) = env::var_os("YARD_RUNTIME_DIR") {
            let base = require_absolute(PathBuf::from(path), "YARD_RUNTIME_DIR")?;
            (vec![base.clone()], base)
        } else if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
            let xdg = require_absolute(PathBuf::from(path), "XDG_RUNTIME_DIR")?;
            let namespace = xdg.join("yard");
            (vec![xdg, namespace.clone()], namespace)
        } else {
            let namespace =
                PathBuf::from("/tmp").join(format!("yard-{}", rustix::process::geteuid().as_raw()));
            (vec![namespace.clone()], namespace)
        };
        let root = namespace.join(database_id);
        directories.push(root.clone());
        let paths = Self::new(directories, root);
        paths.validate_socket_length()?;
        Ok(paths)
    }

    #[cfg(test)]
    fn from_root(root: PathBuf) -> Self {
        Self::new(vec![root.clone()], root)
    }

    fn new(directories: Vec<PathBuf>, root: PathBuf) -> Self {
        Self {
            control: root.join("control.sock"),
            metadata: root.join("instance.json"),
            launch_lock: root.join("launch.lock"),
            instance_lock: root.join("instance.lock"),
            log: root.join("yard.log"),
            directories,
            root,
        }
    }

    fn prepare(&self) -> Result<(), LifecycleError> {
        for directory in &self.directories {
            ensure_private_directory(directory, true)?;
        }
        Ok(())
    }

    fn validate_existing(&self) -> Result<bool, LifecycleError> {
        for directory in &self.directories {
            if !ensure_private_directory(directory, false)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn lock_launch(&self, create: bool) -> Result<Option<File>, LifecycleError> {
        let Some(file) = open_private_file(&self.launch_lock, create, false)? else {
            return Ok(None);
        };
        let deadline = Instant::now() + LAUNCH_LOCK_TIMEOUT;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Some(file)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(LifecycleError::LaunchLockTimeout(self.launch_lock.clone()));
                    }
                    sleep(POLL_INTERVAL).await;
                }
                Err(source) => {
                    return Err(Self::path_error(
                        "lock lifecycle launch state",
                        &self.launch_lock,
                        source,
                    ));
                }
            }
        }
    }

    fn acquire_instance(&self) -> Result<File, LifecycleError> {
        match self.instance_lock_state(true)? {
            InstanceLockState::Free(file) => Ok(file),
            InstanceLockState::Held => {
                Err(LifecycleError::InstanceOwned(self.instance_lock.clone()))
            }
            InstanceLockState::Missing => unreachable!("create=true always opens the lock"),
        }
    }

    fn instance_lock_state(&self, create: bool) -> Result<InstanceLockState, LifecycleError> {
        let Some(file) = open_private_file(&self.instance_lock, create, false)? else {
            return Ok(InstanceLockState::Missing);
        };
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(InstanceLockState::Free(file)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(InstanceLockState::Held),
            Err(source) => Err(Self::path_error(
                "inspect lifecycle instance lock",
                &self.instance_lock,
                source,
            )),
        }
    }

    pub(crate) fn open_log(&self) -> Result<File, LifecycleError> {
        let file = open_private_file(&self.log, true, true)?
            .ok_or_else(|| Self::path_error("open managed log", &self.log, not_found()))?;
        file.set_len(0)
            .map_err(|source| Self::path_error("truncate managed log", &self.log, source))?;
        Ok(file)
    }

    fn validate_socket_length(&self) -> Result<(), LifecycleError> {
        use std::os::unix::ffi::OsStrExt;

        #[cfg(target_os = "macos")]
        const MAX_SOCKET_PATH: usize = 103;
        #[cfg(not(target_os = "macos"))]
        const MAX_SOCKET_PATH: usize = 107;

        let length = self.control.as_os_str().as_bytes().len();
        if length <= MAX_SOCKET_PATH {
            Ok(())
        } else {
            Err(LifecycleError::UnsafeRuntimePath {
                path: self.control.clone(),
                reason: format!(
                    "Unix socket path is {length} bytes; supported maximum is {MAX_SOCKET_PATH}"
                ),
            })
        }
    }

    fn path_error(operation: &'static str, path: &Path, source: io::Error) -> LifecycleError {
        LifecycleError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

enum InstanceLockState {
    Free(File),
    Held,
    Missing,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstanceMode {
    Managed,
    Foreground,
}

impl fmt::Display for InstanceMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Managed => formatter.write_str("managed"),
            Self::Foreground => formatter.write_str("foreground"),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct InstanceMetadata {
    protocol: u32,
    instance_id: String,
    control_secret: String,
    config_fingerprint: String,
    mode: InstanceMode,
    pid: u32,
    address: SocketAddr,
    url: String,
    started_at: String,
}

impl fmt::Debug for InstanceMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstanceMetadata")
            .field("protocol", &self.protocol)
            .field("instance_id", &self.instance_id)
            .field("control_secret", &"[redacted]")
            .field("config_fingerprint", &self.config_fingerprint)
            .field("mode", &self.mode)
            .field("pid", &self.pid)
            .field("address", &self.address)
            .field("url", &self.url)
            .field("started_at", &self.started_at)
            .finish()
    }
}

impl InstanceMetadata {
    pub(crate) fn new(
        instance_id: String,
        config: &ServerConfig,
        mode: InstanceMode,
        address: SocketAddr,
        url: String,
        started_at: String,
    ) -> Result<Self, LifecycleError> {
        Ok(Self {
            protocol: CONTROL_PROTOCOL,
            instance_id,
            control_secret: random_secret()?,
            config_fingerprint: config_fingerprint(config),
            mode,
            pid: std::process::id(),
            address,
            url,
            started_at,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum InstanceState {
    Running,
    Stopping,
}

#[derive(Debug)]
enum ManagedState {
    Stopped {
        stale: bool,
    },
    Active {
        metadata: InstanceMetadata,
        state: InstanceState,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct ControlRequest {
    protocol: u32,
    instance_id: String,
    control_secret: String,
    command: ControlCommand,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ControlCommand {
    Status,
    Stop,
}

#[derive(Deserialize, Serialize)]
struct ControlResponse {
    protocol: u32,
    instance_id: String,
    control_secret: String,
    mode: InstanceMode,
    state: InstanceState,
    pid: u32,
    address: SocketAddr,
    url: String,
    error: Option<String>,
}

impl fmt::Debug for ControlResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlResponse")
            .field("protocol", &self.protocol)
            .field("instance_id", &self.instance_id)
            .field("control_secret", &"[redacted]")
            .field("mode", &self.mode)
            .field("state", &self.state)
            .field("pid", &self.pid)
            .field("address", &self.address)
            .field("url", &self.url)
            .field("error", &self.error)
            .finish()
    }
}

#[derive(Debug)]
enum ControlRequestError {
    Unavailable(io::Error),
    Timeout,
    Protocol(String),
}

pub(crate) struct RuntimeClaim {
    paths: RuntimePaths,
    instance_lock: File,
    launch_lock: Option<File>,
}

impl RuntimeClaim {
    pub(crate) fn paths(&self) -> &RuntimePaths {
        &self.paths
    }

    pub(crate) fn published(&mut self) {
        self.launch_lock.take();
    }

    pub(crate) fn retain_instance_lock_until_process_exit(self) {
        let Self {
            paths: _,
            instance_lock,
            launch_lock,
        } = self;
        debug_assert!(launch_lock.is_none());
        // The OS closes this descriptor when the process exits. Leaking only
        // the descriptor keeps ownership authoritative through Tokio runtime
        // teardown and any non-cancellable spawn_blocking work.
        std::mem::forget(instance_lock);
    }
}

pub(crate) async fn claim_runtime(
    database_path: &Path,
    mode: InstanceMode,
) -> Result<RuntimeClaim, LifecycleError> {
    let paths = RuntimePaths::discover(database_path)?;
    paths.prepare()?;
    let launch_lock = if mode == InstanceMode::Foreground {
        let lock = paths
            .lock_launch(true)
            .await?
            .expect("create=true returns a launch lock");
        match inspect(&paths, true).await? {
            ManagedState::Stopped { .. } => {}
            ManagedState::Active { metadata, .. } => {
                return Err(LifecycleError::AlreadyRunning(metadata.mode));
            }
        }
        Some(lock)
    } else {
        None
    };
    let instance_lock = paths.acquire_instance()?;
    Ok(RuntimeClaim {
        paths,
        instance_lock,
        launch_lock,
    })
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn start(no_open: bool) -> Result<ExitCode, LifecycleError> {
    let config = ServerConfig::from_env()
        .map_err(|error| LifecycleError::Configuration(error.to_string()))?;
    let paths = RuntimePaths::discover(&config.database_path)?;
    paths.prepare()?;
    let launch_lock = paths
        .lock_launch(true)
        .await?
        .expect("create=true returns a launch lock");

    match inspect(&paths, true).await? {
        ManagedState::Active {
            metadata,
            state: InstanceState::Running,
        } => {
            print_instance("Yard is already running", &metadata, &paths.log);
            if metadata.config_fingerprint != config_fingerprint(&config) {
                if config.bind.port() != 0 && config.bind != metadata.address {
                    eprintln!(
                        "warning: requested bind {} was not applied; the existing instance remains at {}",
                        config.bind, metadata.address
                    );
                } else {
                    eprintln!(
                        "warning: requested configuration was not applied; the existing instance remains active"
                    );
                }
            } else if config.bind.port() != 0 && config.bind != metadata.address {
                eprintln!(
                    "warning: requested bind {} was not applied; the existing instance remains at {}",
                    config.bind, metadata.address
                );
            }
            drop(launch_lock);
            maybe_open_browser(no_open, &metadata.url).await;
            return Ok(ExitCode::SUCCESS);
        }
        ManagedState::Active {
            state: InstanceState::Stopping,
            ..
        } => {
            return Err(LifecycleError::Startup(
                "the existing instance is still stopping; retry shortly".to_owned(),
            ));
        }
        ManagedState::Stopped { .. } => {}
    }

    let (_signal_task, mut interrupted) = startup_signal_monitor()?;
    tokio::task::yield_now().await;
    let instance_id = Uuid::now_v7().to_string();
    let mut child = spawn_daemon(&paths, &instance_id)?;
    let expected_pid = child.id();
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    loop {
        if let Some(status) = child.try_wait().map_err(|source| {
            RuntimePaths::path_error("inspect background process", &paths.log, source)
        })? {
            let _ = inspect(&paths, true).await;
            return Err(LifecycleError::Startup(startup_failure(&paths, status)));
        }

        match inspect(&paths, false).await {
            Ok(ManagedState::Active {
                metadata,
                state: InstanceState::Running,
            }) if metadata.instance_id == instance_id
                && metadata.mode == InstanceMode::Managed
                && expected_pid.is_none_or(|pid| pid == metadata.pid)
                && health_ready(metadata.address).await =>
            {
                print_instance("Yard started", &metadata, &paths.log);
                drop(launch_lock);
                maybe_open_browser(no_open, &metadata.url).await;
                return Ok(ExitCode::SUCCESS);
            }
            Ok(ManagedState::Active { metadata, .. }) if metadata.instance_id != instance_id => {
                terminate_child(&mut child).await?;
                return Err(LifecycleError::Startup(
                    "a different instance claimed the control socket".to_owned(),
                ));
            }
            Ok(ManagedState::Active { metadata, .. })
                if expected_pid.is_some_and(|pid| pid != metadata.pid) =>
            {
                terminate_child(&mut child).await?;
                return Err(LifecycleError::Startup(
                    "the background process reported an unexpected identity".to_owned(),
                ));
            }
            Ok(_)
            | Err(LifecycleError::ControlConflict(_) | LifecycleError::ControlProtocol(_)) => {}
            Err(error) => {
                terminate_child(&mut child).await?;
                return Err(error);
            }
        }

        if Instant::now() >= deadline {
            terminate_child(&mut child).await?;
            let _ = inspect(&paths, true).await;
            return Err(LifecycleError::Startup(format!(
                "startup timed out after {} seconds; see {}",
                STARTUP_TIMEOUT.as_secs(),
                paths.log.display()
            )));
        }
        tokio::select! {
            changed = interrupted.changed() => {
                if changed.is_ok() && *interrupted.borrow() {
                    terminate_child(&mut child).await?;
                    return Err(LifecycleError::Startup(
                        "startup was interrupted; the spawned child was terminated".to_owned(),
                    ));
                }
            }
            () = sleep(POLL_INTERVAL) => {}
        }
    }
}

pub(crate) async fn status() -> Result<ExitCode, LifecycleError> {
    let database_path = database_path_from_env()
        .map_err(|error| LifecycleError::Configuration(error.to_string()))?;
    let paths = RuntimePaths::discover(&database_path)?;
    if !paths.validate_existing()? {
        println!("Yard is not running");
        return Ok(ExitCode::from(STOPPED_EXIT_CODE));
    }
    let _launch_lock = paths.lock_launch(false).await?;

    match inspect(&paths, false).await? {
        ManagedState::Stopped { stale } => {
            if stale {
                println!("Yard is not running (stale lifecycle state)");
            } else {
                println!("Yard is not running");
            }
            Ok(ExitCode::from(STOPPED_EXIT_CODE))
        }
        ManagedState::Active { metadata, state } => {
            let heading = if state == InstanceState::Running {
                "Yard is running"
            } else {
                "Yard is stopping"
            };
            print_instance(heading, &metadata, &paths.log);
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub(crate) async fn stop() -> Result<ExitCode, LifecycleError> {
    let database_path = database_path_from_env()
        .map_err(|error| LifecycleError::Configuration(error.to_string()))?;
    let paths = RuntimePaths::discover(&database_path)?;
    if !paths.validate_existing()? {
        println!("Yard is already stopped");
        return Ok(ExitCode::SUCCESS);
    }
    let _launch_lock = paths
        .lock_launch(true)
        .await?
        .expect("create=true returns a launch lock");

    let metadata = match inspect(&paths, true).await? {
        ManagedState::Stopped { .. } => {
            println!("Yard is already stopped");
            return Ok(ExitCode::SUCCESS);
        }
        ManagedState::Active { metadata, state } => {
            if metadata.mode == InstanceMode::Foreground {
                return Err(LifecycleError::ForegroundStopRefused);
            }
            if state == InstanceState::Running {
                request(&paths, &metadata, ControlCommand::Stop)
                    .await
                    .map_err(control_error)?;
            }
            metadata
        }
    };

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        match inspect(&paths, true).await {
            Ok(ManagedState::Stopped { .. }) => {
                println!("Yard stopped");
                return Ok(ExitCode::SUCCESS);
            }
            Ok(ManagedState::Active {
                metadata: current, ..
            }) if current.instance_id != metadata.instance_id => {
                return Err(LifecycleError::ControlConflict(
                    "another instance replaced the process being stopped".to_owned(),
                ));
            }
            Ok(ManagedState::Active { .. })
            | Err(LifecycleError::ControlProtocol(_) | LifecycleError::ControlConflict(_)) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(LifecycleError::StopTimeout);
        }
        sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
pub(crate) async fn request_managed_stop(paths: &RuntimePaths) -> Result<(), LifecycleError> {
    let ManagedState::Active { metadata, state } = inspect(paths, false).await? else {
        return Err(LifecycleError::ControlConflict(
            "the test server has not published lifecycle metadata".to_owned(),
        ));
    };
    if metadata.mode != InstanceMode::Managed {
        return Err(LifecycleError::ForegroundStopRefused);
    }
    if state == InstanceState::Running {
        request(paths, &metadata, ControlCommand::Stop)
            .await
            .map_err(control_error)?;
    }
    Ok(())
}

fn spawn_daemon(paths: &RuntimePaths, instance_id: &str) -> Result<Child, LifecycleError> {
    let stdout = paths.open_log()?;
    let stderr = stdout
        .try_clone()
        .map_err(|source| RuntimePaths::path_error("clone managed log", &paths.log, source))?;
    let executable = env::current_exe().map_err(|source| {
        RuntimePaths::path_error("locate Yard executable", &paths.root, source)
    })?;
    let mut command = Command::new(executable);
    command
        .arg("__managed-run")
        .arg("--instance-id")
        .arg(instance_id)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(false);
    command.as_std_mut().process_group(0);
    command
        .spawn()
        .map_err(|source| RuntimePaths::path_error("start background process", &paths.log, source))
}

fn startup_signal_monitor() -> Result<(JoinHandle<()>, watch::Receiver<bool>), LifecycleError> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|source| LifecycleError::Io {
        operation: "install startup SIGTERM handler",
        path: PathBuf::from("<process>"),
        source,
    })?;
    let (sender, receiver) = watch::channel(false);
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        let _ = sender.send(true);
    });
    Ok((task, receiver))
}

async fn terminate_child(child: &mut Child) -> Result<(), LifecycleError> {
    match child.start_kill() {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
        Err(source) => {
            return Err(LifecycleError::Startup(format!(
                "could not terminate the spawned child: {source}"
            )));
        }
    }
    timeout(CHILD_TERMINATION_TIMEOUT, child.wait())
        .await
        .map_err(|_| {
            LifecycleError::Startup(format!(
                "spawned child did not exit within {} seconds",
                CHILD_TERMINATION_TIMEOUT.as_secs()
            ))
        })?
        .map(|_| ())
        .map_err(|error| LifecycleError::Startup(format!("could not reap spawned child: {error}")))
}

fn startup_failure(paths: &RuntimePaths, status: std::process::ExitStatus) -> String {
    let tail = read_log_tail(&paths.log).unwrap_or_default();
    if tail.is_empty() {
        format!(
            "background process exited with {status}; see {}",
            paths.log.display()
        )
    } else {
        format!(
            "background process exited with {status}; see {}\n{}",
            paths.log.display(),
            tail
        )
    }
}

fn read_log_tail(path: &Path) -> io::Result<String> {
    let Some(mut file) = open_private_file(path, false, false)
        .map_err(|error| io::Error::other(error.to_string()))?
    else {
        return Ok(String::new());
    };
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length - start).unwrap_or(0));
    file.take(MAX_LOG_TAIL_BYTES).read_to_end(&mut bytes)?;
    if start > 0
        && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
    {
        bytes.drain(..=newline);
    }
    let contents = String::from_utf8_lossy(&bytes);
    let lines = contents.lines().rev().take(12).collect::<Vec<_>>();
    Ok(lines.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn print_instance(heading: &str, metadata: &InstanceMetadata, log: &Path) {
    println!("{heading}");
    println!("  Mode: {}", metadata.mode);
    println!("  URL: {}", metadata.url);
    println!("  PID: {}", metadata.pid);
    if metadata.mode == InstanceMode::Managed {
        println!("  Log: {}", log.display());
    } else {
        println!("  Log: attached terminal");
    }
}

async fn maybe_open_browser(no_open: bool, url: &str) {
    if no_open {
        return;
    }
    if let Err(error) = open_browser(url).await {
        eprintln!("warning: could not open the default browser: {error}");
        eprintln!("Open {url}");
    }
}

async fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err("browser launch is supported on Linux and macOS".to_owned());

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let mut command = Command::new(launcher);
        command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not run {launcher}: {error}"))?;
        let status = if let Ok(result) = timeout(BROWSER_TIMEOUT, child.wait()).await {
            result.map_err(|error| format!("could not wait for {launcher}: {error}"))?
        } else {
            return match timeout(CHILD_TERMINATION_TIMEOUT, child.kill()).await {
                Ok(Ok(())) => Err(format!(
                    "{launcher} did not exit within {} seconds and was terminated",
                    BROWSER_TIMEOUT.as_secs()
                )),
                Ok(Err(error)) => Err(format!(
                    "{launcher} did not exit within {} seconds and termination failed: {error}",
                    BROWSER_TIMEOUT.as_secs()
                )),
                Err(_) => Err(format!(
                    "{launcher} did not exit within {} seconds; termination could not be confirmed within {} seconds",
                    BROWSER_TIMEOUT.as_secs(),
                    CHILD_TERMINATION_TIMEOUT.as_secs()
                )),
            };
        };
        if status.success() {
            Ok(())
        } else {
            Err(format!("{launcher} exited with {status}"))
        }
    }
}

async fn health_ready(address: SocketAddr) -> bool {
    timeout(CONTROL_TIMEOUT, async {
        let mut stream = TcpStream::connect(address).await?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: yard\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = vec![0_u8; 4096];
        let length = stream.read(&mut response).await?;
        let response = String::from_utf8_lossy(&response[..length]);
        Ok::<_, io::Error>(
            response.starts_with("HTTP/1.1 200") && response.contains(r#""status":"ok""#),
        )
    })
    .await
    .is_ok_and(Result::unwrap_or_default)
}

async fn inspect(
    paths: &RuntimePaths,
    recover_stale: bool,
) -> Result<ManagedState, LifecycleError> {
    let Some(bytes) = read_private_file(&paths.metadata, MAX_METADATA_SIZE)? else {
        return inspect_without_metadata(paths, recover_stale, "metadata is missing").await;
    };
    let metadata = match serde_json::from_slice::<InstanceMetadata>(&bytes) {
        Ok(metadata) if metadata.protocol == CONTROL_PROTOCOL => metadata,
        Ok(_) | Err(_) => {
            return inspect_without_metadata(paths, recover_stale, "metadata is invalid").await;
        }
    };

    validate_socket_path(paths)?;
    match request(paths, &metadata, ControlCommand::Status).await {
        Ok(response) => Ok(ManagedState::Active {
            metadata,
            state: response.state,
        }),
        Err(ControlRequestError::Unavailable(error)) if is_stale_socket_error(&error) => {
            stale_state(paths, recover_stale, true)
        }
        Err(error) => Err(control_error(error)),
    }
}

async fn inspect_without_metadata(
    paths: &RuntimePaths,
    recover_stale: bool,
    reason: &str,
) -> Result<ManagedState, LifecycleError> {
    let socket_exists = validate_socket_path(paths)?;
    if !socket_exists {
        let stale = paths.metadata.exists();
        return stale_state(paths, recover_stale, stale);
    }

    match timeout(CONTROL_TIMEOUT, UnixStream::connect(&paths.control)).await {
        Ok(Ok(stream)) => {
            verify_peer_uid(&stream).map_err(control_error)?;
            Err(LifecycleError::ControlConflict(format!(
                "{reason}, but {} accepts same-UID connections",
                paths.control.display()
            )))
        }
        Ok(Err(error)) if is_stale_socket_error(&error) => stale_state(paths, recover_stale, true),
        Ok(Err(error)) => Err(LifecycleError::ControlConflict(format!(
            "{reason}, and {} could not be verified: {error}",
            paths.control.display()
        ))),
        Err(_) => Err(LifecycleError::ControlConflict(format!(
            "{reason}, and {} did not respond",
            paths.control.display()
        ))),
    }
}

fn stale_state(
    paths: &RuntimePaths,
    recover_stale: bool,
    stale: bool,
) -> Result<ManagedState, LifecycleError> {
    match paths.instance_lock_state(recover_stale)? {
        InstanceLockState::Held => Err(LifecycleError::ControlConflict(
            "the instance lock is held while control is unavailable; no state was removed"
                .to_owned(),
        )),
        InstanceLockState::Free(_guard) => {
            if recover_stale && stale {
                remove_stale_state(paths)?;
            }
            Ok(ManagedState::Stopped { stale })
        }
        InstanceLockState::Missing => Ok(ManagedState::Stopped { stale }),
    }
}

async fn request(
    paths: &RuntimePaths,
    metadata: &InstanceMetadata,
    command: ControlCommand,
) -> Result<ControlResponse, ControlRequestError> {
    let operation = async {
        let mut stream = UnixStream::connect(&paths.control)
            .await
            .map_err(ControlRequestError::Unavailable)?;
        verify_peer_uid(&stream)?;
        let request = ControlRequest {
            protocol: CONTROL_PROTOCOL,
            instance_id: metadata.instance_id.clone(),
            control_secret: metadata.control_secret.clone(),
            command,
        };
        let mut bytes = serde_json::to_vec(&request)
            .map_err(|error| ControlRequestError::Protocol(error.to_string()))?;
        bytes.push(b'\n');
        stream
            .write_all(&bytes)
            .await
            .map_err(ControlRequestError::Unavailable)?;
        let mut line = String::new();
        BufReader::new(stream)
            .take(MAX_CONTROL_FRAME)
            .read_line(&mut line)
            .await
            .map_err(ControlRequestError::Unavailable)?;
        if line.is_empty() || line.len() as u64 >= MAX_CONTROL_FRAME || !line.ends_with('\n') {
            return Err(ControlRequestError::Protocol(
                "control channel returned an empty or oversized frame".to_owned(),
            ));
        }
        let response: ControlResponse = serde_json::from_str(&line)
            .map_err(|error| ControlRequestError::Protocol(error.to_string()))?;
        if response.protocol != CONTROL_PROTOCOL
            || response.instance_id != metadata.instance_id
            || response.control_secret != metadata.control_secret
            || response.mode != metadata.mode
            || response.pid != metadata.pid
            || response.address != metadata.address
            || response.url != metadata.url
        {
            return Err(ControlRequestError::Protocol(
                "control response did not match the lifecycle metadata".to_owned(),
            ));
        }
        if let Some(error) = response.error.as_deref() {
            return Err(ControlRequestError::Protocol(error.to_owned()));
        }
        Ok(response)
    };

    timeout(CONTROL_TIMEOUT, operation)
        .await
        .map_err(|_| ControlRequestError::Timeout)?
}

fn verify_peer_uid(stream: &UnixStream) -> Result<(), ControlRequestError> {
    let peer = stream
        .peer_cred()
        .map_err(ControlRequestError::Unavailable)?;
    if peer.uid() == rustix::process::geteuid().as_raw() {
        Ok(())
    } else {
        Err(ControlRequestError::Protocol(
            "control peer has a different effective UID".to_owned(),
        ))
    }
}

fn control_error(error: ControlRequestError) -> LifecycleError {
    match error {
        ControlRequestError::Unavailable(error) => {
            LifecycleError::ControlProtocol(error.to_string())
        }
        ControlRequestError::Timeout => {
            LifecycleError::ControlProtocol("control request timed out".to_owned())
        }
        ControlRequestError::Protocol(message) => LifecycleError::ControlProtocol(message),
    }
}

fn is_stale_socket_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

fn remove_stale_state(paths: &RuntimePaths) -> Result<(), LifecycleError> {
    remove_owned_regular_file(&paths.metadata)?;
    remove_owned_socket(&paths.control)
}

fn remove_owned_regular_file(path: &Path) -> Result<(), LifecycleError> {
    let Some(quarantined) = quarantine_owned_path(path, PrivatePathKind::RegularFile)? else {
        return Ok(());
    };
    remove_quarantined_path(path, &quarantined, "remove stale lifecycle metadata")
}

fn remove_owned_socket(path: &Path) -> Result<(), LifecycleError> {
    let Some(quarantined) = quarantine_owned_path(path, PrivatePathKind::Socket)? else {
        return Ok(());
    };
    remove_quarantined_path(path, &quarantined, "remove stale lifecycle socket")
}

fn quarantine_owned_path(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<Option<PathBuf>, LifecycleError> {
    let parent = path
        .parent()
        .ok_or_else(|| LifecycleError::UnsafeRuntimePath {
            path: path.to_path_buf(),
            reason: "lifecycle path has no parent directory".to_owned(),
        })?;
    let quarantined = parent.join(format!(".cleanup-{}", Uuid::now_v7()));
    match fs::rename(path, &quarantined) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LifecycleError::Io {
                operation: "quarantine lifecycle path before cleanup",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let validation = fs::symlink_metadata(&quarantined)
        .map_err(|source| LifecycleError::Io {
            operation: "inspect quarantined lifecycle path",
            path: quarantined.clone(),
            source,
        })
        .and_then(|metadata| validate_private_metadata(path, &metadata, kind));
    if let Err(error) = validation {
        restore_quarantined_path(path, &quarantined)?;
        return Err(error);
    }
    Ok(Some(quarantined))
}

fn restore_quarantined_path(path: &Path, quarantined: &Path) -> Result<(), LifecycleError> {
    fs::rename(quarantined, path).map_err(|source| LifecycleError::Io {
        operation: "restore rejected lifecycle path",
        path: path.to_path_buf(),
        source,
    })
}

fn remove_quarantined_path(
    original: &Path,
    quarantined: &Path,
    operation: &'static str,
) -> Result<(), LifecycleError> {
    fs::remove_file(quarantined).map_err(|source| LifecycleError::Io {
        operation,
        path: original.to_path_buf(),
        source,
    })
}

fn remove_metadata_for_instance(path: &Path, instance_id: &str) -> Result<bool, LifecycleError> {
    let Some(quarantined) = quarantine_owned_path(path, PrivatePathKind::RegularFile)? else {
        return Ok(false);
    };
    let ownership = read_private_file(&quarantined, MAX_METADATA_SIZE)
        .and_then(|bytes| {
            bytes.ok_or_else(|| RuntimePaths::path_error("read owned metadata", path, not_found()))
        })
        .and_then(|bytes| {
            serde_json::from_slice::<InstanceMetadata>(&bytes).map_err(LifecycleError::from)
        })
        .map(|metadata| metadata.instance_id == instance_id);

    match ownership {
        Ok(true) => {
            remove_quarantined_path(path, &quarantined, "remove owned lifecycle metadata")?;
            Ok(true)
        }
        Ok(false) => {
            restore_quarantined_path(path, &quarantined)?;
            Ok(false)
        }
        Err(error) => {
            restore_quarantined_path(path, &quarantined)?;
            Err(error)
        }
    }
}

pub(crate) fn parse_instance_id(instance_id: &str) -> Result<(), LifecycleError> {
    Uuid::parse_str(instance_id)
        .map(|_| ())
        .map_err(|_| LifecycleError::InvalidInstanceId(instance_id.to_owned()))
}

pub(crate) fn new_instance_id() -> String {
    Uuid::now_v7().to_string()
}

pub(crate) fn bind_control(paths: &RuntimePaths) -> Result<UnixListener, LifecycleError> {
    match fs::symlink_metadata(&paths.control) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(LifecycleError::ControlConflict(format!(
                "{} already exists",
                paths.control.display()
            )));
        }
        Err(source) => {
            return Err(RuntimePaths::path_error(
                "inspect managed control socket",
                &paths.control,
                source,
            ));
        }
    }
    let listener = UnixListener::bind(&paths.control).map_err(|source| {
        RuntimePaths::path_error("bind managed control socket", &paths.control, source)
    })?;
    fs::set_permissions(&paths.control, fs::Permissions::from_mode(0o600)).map_err(|source| {
        RuntimePaths::path_error("secure managed control socket", &paths.control, source)
    })?;
    validate_socket_path(paths)?;
    Ok(listener)
}

pub(crate) fn write_metadata(
    paths: &RuntimePaths,
    metadata: &InstanceMetadata,
) -> Result<(), LifecycleError> {
    let temporary = paths.root.join(format!(".instance-{}.tmp", Uuid::now_v7()));
    let result = (|| {
        let mut file = open_private_file(&temporary, true, false)?.ok_or_else(|| {
            RuntimePaths::path_error("create lifecycle metadata", &temporary, not_found())
        })?;
        serde_json::to_writer(&mut file, metadata)?;
        file.write_all(b"\n").map_err(|source| {
            RuntimePaths::path_error("write lifecycle metadata", &temporary, source)
        })?;
        file.sync_all().map_err(|source| {
            RuntimePaths::path_error("sync lifecycle metadata", &temporary, source)
        })?;
        fs::rename(&temporary, &paths.metadata).map_err(|source| {
            RuntimePaths::path_error("publish lifecycle metadata", &paths.metadata, source)
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn spawn_control(
    listener: UnixListener,
    metadata: InstanceMetadata,
    shutdown: watch::Sender<bool>,
) -> JoinHandle<()> {
    let metadata = Arc::new(metadata);
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _address)) => {
                    let metadata = Arc::clone(&metadata);
                    let shutdown = shutdown.clone();
                    tokio::spawn(async move {
                        match timeout(
                            CONTROL_TIMEOUT,
                            handle_control(stream, &metadata, &shutdown),
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                tracing::warn!(target: "yard_server", %error, "lifecycle control request failed");
                            }
                            Err(_) => {
                                tracing::warn!(
                                    target: "yard_server",
                                    timeout_seconds = CONTROL_TIMEOUT.as_secs(),
                                    "lifecycle control request timed out"
                                );
                            }
                        }
                    });
                }
                Err(error) => {
                    tracing::warn!(target: "yard_server", %error, "lifecycle control listener failed");
                    let _ = shutdown.send(true);
                    break;
                }
            }
        }
    })
}

async fn handle_control(
    stream: UnixStream,
    metadata: &InstanceMetadata,
    shutdown: &watch::Sender<bool>,
) -> Result<(), LifecycleError> {
    verify_peer_uid(&stream).map_err(control_error)?;
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    BufReader::new(reader)
        .take(MAX_CONTROL_FRAME)
        .read_line(&mut line)
        .await
        .map_err(|source| LifecycleError::ControlProtocol(source.to_string()))?;
    if line.is_empty() || line.len() as u64 >= MAX_CONTROL_FRAME || !line.ends_with('\n') {
        return Err(LifecycleError::ControlProtocol(
            "control request frame was empty or oversized".to_owned(),
        ));
    }
    let request: ControlRequest = serde_json::from_str(&line)?;
    let mut error = None;
    let mut stop_requested = false;
    let authenticated = request.protocol == CONTROL_PROTOCOL
        && request.instance_id == metadata.instance_id
        && request.control_secret == metadata.control_secret;
    if request.protocol != CONTROL_PROTOCOL {
        error = Some("unsupported control protocol".to_owned());
    } else if !authenticated {
        error = Some("instance authentication failed".to_owned());
    } else if matches!(request.command, ControlCommand::Stop) {
        if metadata.mode == InstanceMode::Foreground {
            error =
                Some("foreground instances must be stopped in their owning terminal".to_owned());
        } else {
            stop_requested = true;
        }
    }
    let state = if stop_requested || *shutdown.borrow() {
        InstanceState::Stopping
    } else {
        InstanceState::Running
    };
    let response = ControlResponse {
        protocol: CONTROL_PROTOCOL,
        instance_id: metadata.instance_id.clone(),
        control_secret: if authenticated {
            metadata.control_secret.clone()
        } else {
            String::new()
        },
        mode: metadata.mode,
        state,
        pid: metadata.pid,
        address: metadata.address,
        url: metadata.url.clone(),
        error,
    };
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|source| LifecycleError::ControlProtocol(source.to_string()))?;
    writer
        .shutdown()
        .await
        .map_err(|source| LifecycleError::ControlProtocol(source.to_string()))?;
    if stop_requested {
        let _ = shutdown.send(true);
    }
    Ok(())
}

pub(crate) struct RuntimeOwnerGuard {
    paths: RuntimePaths,
    instance_id: String,
}

impl RuntimeOwnerGuard {
    pub(crate) fn new(paths: RuntimePaths, instance_id: String) -> Self {
        Self { paths, instance_id }
    }
}

impl Drop for RuntimeOwnerGuard {
    fn drop(&mut self) {
        if remove_metadata_for_instance(&self.paths.metadata, &self.instance_id).unwrap_or(false) {
            let _ = remove_owned_socket(&self.paths.control);
        }
    }
}

fn database_identity(database_path: &Path) -> Result<String, LifecycleError> {
    use std::os::unix::ffi::OsStrExt;

    let absolute = if database_path.is_absolute() {
        database_path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|source| LifecycleError::Io {
                operation: "resolve database identity",
                path: database_path.to_path_buf(),
                source,
            })?
            .join(database_path)
    };
    let normalized = normalize_lexically(&absolute);
    let digest = Sha256::digest(normalized.as_os_str().as_bytes());
    let mut identity = String::with_capacity(32);
    for byte in &digest[..16] {
        let _ = write!(identity, "{byte:02x}");
    }
    Ok(identity)
}

fn config_fingerprint(config: &ServerConfig) -> String {
    use std::os::unix::ffi::OsStrExt;

    let mut digest = Sha256::new();
    for value in [
        config.bind.to_string().as_bytes(),
        config.herdr_binary.as_os_str().as_bytes(),
        config.database_path.as_os_str().as_bytes(),
        config.artifact_path.as_os_str().as_bytes(),
        config.orchestrator_cwd.as_os_str().as_bytes(),
        config.coordination_path.as_os_str().as_bytes(),
        config.knowledge_path.as_os_str().as_bytes(),
    ] {
        digest.update(value.len().to_le_bytes());
        digest.update(value);
    }
    format!("{:x}", digest.finalize())
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Normal(_) | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn require_absolute(path: PathBuf, variable: &str) -> Result<PathBuf, LifecycleError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(LifecycleError::UnsafeRuntimePath {
            path,
            reason: format!("{variable} must be an absolute path"),
        })
    }
}

fn ensure_private_directory(path: &Path, create: bool) -> Result<bool, LifecycleError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_private_metadata(path, &metadata, PrivatePathKind::Directory)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|source| LifecycleError::Io {
                    operation: "create private runtime directory",
                    path: path.to_path_buf(),
                    source,
                })?;
            let metadata = fs::symlink_metadata(path).map_err(|source| LifecycleError::Io {
                operation: "inspect private runtime directory",
                path: path.to_path_buf(),
                source,
            })?;
            validate_private_metadata(path, &metadata, PrivatePathKind::Directory)?;
            Ok(true)
        }
        Err(source) => Err(LifecycleError::Io {
            operation: "inspect private runtime directory",
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[derive(Clone, Copy)]
enum PrivatePathKind {
    Directory,
    RegularFile,
    Socket,
}

fn validate_private_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    kind: PrivatePathKind,
) -> Result<(), LifecycleError> {
    let correct_type = match kind {
        PrivatePathKind::Directory => metadata.file_type().is_dir(),
        PrivatePathKind::RegularFile => metadata.file_type().is_file(),
        PrivatePathKind::Socket => metadata.file_type().is_socket(),
    };
    let expected_mode = match kind {
        PrivatePathKind::Directory => 0o700,
        PrivatePathKind::RegularFile | PrivatePathKind::Socket => 0o600,
    };
    let mode = metadata.mode() & 0o777;
    let current_uid = rustix::process::geteuid().as_raw();
    if metadata.file_type().is_symlink() || !correct_type {
        return Err(LifecycleError::UnsafeRuntimePath {
            path: path.to_path_buf(),
            reason: "wrong file type or symbolic link".to_owned(),
        });
    }
    if metadata.uid() != current_uid {
        return Err(LifecycleError::UnsafeRuntimePath {
            path: path.to_path_buf(),
            reason: format!(
                "owned by UID {}, expected effective UID {current_uid}",
                metadata.uid()
            ),
        });
    }
    if mode != expected_mode {
        return Err(LifecycleError::UnsafeRuntimePath {
            path: path.to_path_buf(),
            reason: format!("mode is {mode:04o}, expected {expected_mode:04o}"),
        });
    }
    Ok(())
}

fn open_private_file(
    path: &Path,
    create: bool,
    append: bool,
) -> Result<Option<File>, LifecycleError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .append(append)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(source) => {
            return Err(LifecycleError::Io {
                operation: "open private lifecycle file",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let metadata = file.metadata().map_err(|source| LifecycleError::Io {
        operation: "inspect private lifecycle file",
        path: path.to_path_buf(),
        source,
    })?;
    validate_private_metadata(path, &metadata, PrivatePathKind::RegularFile)?;
    Ok(Some(file))
}

fn read_private_file(path: &Path, maximum: u64) -> Result<Option<Vec<u8>>, LifecycleError> {
    let Some(file) = open_private_file(path, false, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| LifecycleError::Io {
            operation: "read private lifecycle file",
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > maximum {
        return Err(LifecycleError::UnsafeRuntimePath {
            path: path.to_path_buf(),
            reason: format!("file exceeds the {maximum}-byte limit"),
        });
    }
    Ok(Some(bytes))
}

fn validate_socket_path(paths: &RuntimePaths) -> Result<bool, LifecycleError> {
    match fs::symlink_metadata(&paths.control) {
        Ok(metadata) => {
            validate_private_metadata(&paths.control, &metadata, PrivatePathKind::Socket)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(RuntimePaths::path_error(
            "inspect lifecycle control socket",
            &paths.control,
            source,
        )),
    }
}

fn random_secret() -> Result<String, LifecycleError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| LifecycleError::Random(error.to_string()))?;
    let mut secret = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(secret, "{byte:02x}");
    }
    Ok(secret)
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "path was not created")
}

#[cfg(test)]
mod tests {
    use std::{
        net::SocketAddr,
        os::unix::{
            fs::{MetadataExt, PermissionsExt},
            net::UnixListener as StdUnixListener,
        },
    };

    use tempfile::tempdir;

    use super::{
        ControlCommand, ControlRequestError, InstanceMetadata, InstanceMode, InstanceState,
        LifecycleError, ManagedState, RuntimePaths, database_identity, inspect, request,
        spawn_control, write_metadata,
    };

    fn metadata(instance_id: &str, mode: InstanceMode) -> InstanceMetadata {
        InstanceMetadata {
            protocol: super::CONTROL_PROTOCOL,
            instance_id: instance_id.to_owned(),
            control_secret: "a".repeat(64),
            config_fingerprint: "b".repeat(64),
            mode,
            pid: 42,
            address: "127.0.0.1:4317".parse::<SocketAddr>().expect("address"),
            url: "http://127.0.0.1:4317/".to_owned(),
            started_at: "2026-08-20T00:00:00Z".to_owned(),
        }
    }

    #[tokio::test]
    async fn control_channel_authenticates_and_requests_managed_shutdown() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let listener = super::bind_control(&paths).expect("bind");
        let metadata = metadata(
            "018f0000-0000-7000-8000-000000000001",
            InstanceMode::Managed,
        );
        write_metadata(&paths, &metadata).expect("metadata");
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let task = spawn_control(listener, metadata.clone(), shutdown);

        let status = request(&paths, &metadata, ControlCommand::Status)
            .await
            .expect("status");
        assert_eq!(status.state, InstanceState::Running);
        let stopped = request(&paths, &metadata, ControlCommand::Stop)
            .await
            .expect("stop");
        assert_eq!(stopped.state, InstanceState::Stopping);
        assert!(*receiver.borrow());
        task.abort();
    }

    #[tokio::test]
    async fn wrong_identity_or_secret_cannot_request_shutdown() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let listener = super::bind_control(&paths).expect("bind");
        let actual = metadata(
            "018f0000-0000-7000-8000-000000000001",
            InstanceMode::Managed,
        );
        write_metadata(&paths, &actual).expect("metadata");
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let task = spawn_control(listener, actual, shutdown);
        let mut unrelated = metadata(
            "018f0000-0000-7000-8000-000000000002",
            InstanceMode::Managed,
        );
        unrelated.control_secret = "b".repeat(64);

        let mut stream = tokio::net::UnixStream::connect(&paths.control)
            .await
            .expect("connect");
        let mut bytes = serde_json::to_vec(&super::ControlRequest {
            protocol: super::CONTROL_PROTOCOL,
            instance_id: unrelated.instance_id.clone(),
            control_secret: unrelated.control_secret.clone(),
            command: ControlCommand::Stop,
        })
        .expect("serialize");
        bytes.push(b'\n');
        tokio::io::AsyncWriteExt::write_all(&mut stream, &bytes)
            .await
            .expect("write");
        let mut line = String::new();
        tokio::io::AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(stream), &mut line)
            .await
            .expect("read");
        let response: super::ControlResponse = serde_json::from_str(&line).expect("response");
        assert!(response.control_secret.is_empty());

        let error = request(&paths, &unrelated, ControlCommand::Stop)
            .await
            .expect_err("identity mismatch");
        assert!(matches!(error, ControlRequestError::Protocol(_)));
        assert!(!*receiver.borrow());
        task.abort();
    }

    #[tokio::test]
    async fn partial_control_client_does_not_block_other_requests() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let listener = super::bind_control(&paths).expect("bind");
        let metadata = metadata(
            "018f0000-0000-7000-8000-000000000001",
            InstanceMode::Managed,
        );
        write_metadata(&paths, &metadata).expect("metadata");
        let (shutdown, _receiver) = tokio::sync::watch::channel(false);
        let task = spawn_control(listener, metadata.clone(), shutdown);
        let mut stalled = tokio::net::UnixStream::connect(&paths.control)
            .await
            .expect("stalled connection");
        tokio::io::AsyncWriteExt::write_all(&mut stalled, b"{")
            .await
            .expect("partial frame");

        let status = request(&paths, &metadata, ControlCommand::Status)
            .await
            .expect("concurrent status");
        assert_eq!(status.state, InstanceState::Running);
        task.abort();
    }

    #[tokio::test]
    async fn foreground_control_refuses_shutdown() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let listener = super::bind_control(&paths).expect("bind");
        let metadata = metadata(
            "018f0000-0000-7000-8000-000000000001",
            InstanceMode::Foreground,
        );
        write_metadata(&paths, &metadata).expect("metadata");
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let task = spawn_control(listener, metadata.clone(), shutdown);

        let error = request(&paths, &metadata, ControlCommand::Stop)
            .await
            .expect_err("foreground stop");
        assert!(matches!(error, ControlRequestError::Protocol(_)));
        assert!(!*receiver.borrow());
        task.abort();
    }

    #[tokio::test]
    async fn stale_socket_and_metadata_are_recovered_without_unlinking_locks() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let launch = paths
            .lock_launch(true)
            .await
            .expect("launch lock")
            .expect("lock file");
        let lock_inode = launch.metadata().expect("lock metadata").ino();
        drop(launch);
        let listener = StdUnixListener::bind(&paths.control).expect("bind stale socket");
        std::fs::set_permissions(&paths.control, std::fs::Permissions::from_mode(0o600))
            .expect("socket mode");
        drop(listener);
        write_metadata(
            &paths,
            &metadata(
                "018f0000-0000-7000-8000-000000000001",
                InstanceMode::Managed,
            ),
        )
        .expect("metadata");

        assert!(matches!(
            inspect(&paths, true).await.expect("inspect"),
            ManagedState::Stopped { stale: true }
        ));
        assert!(!paths.control.exists());
        assert!(!paths.metadata.exists());
        assert_eq!(
            std::fs::metadata(&paths.launch_lock)
                .expect("persistent launch lock")
                .ino(),
            lock_inode
        );
    }

    #[tokio::test]
    async fn held_instance_lock_prevents_stale_cleanup() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let _instance = paths.acquire_instance().expect("instance lock");
        write_metadata(
            &paths,
            &metadata(
                "018f0000-0000-7000-8000-000000000001",
                InstanceMode::Managed,
            ),
        )
        .expect("metadata");

        let error = inspect(&paths, true)
            .await
            .expect_err("held lock must fail closed");
        assert!(matches!(error, LifecycleError::ControlConflict(_)));
        assert!(paths.metadata.exists());
    }

    #[tokio::test]
    async fn active_unidentified_socket_is_preserved_as_a_conflict() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let _listener = StdUnixListener::bind(&paths.control).expect("bind unrelated socket");
        std::fs::set_permissions(&paths.control, std::fs::Permissions::from_mode(0o600))
            .expect("socket mode");

        let error = inspect(&paths, true)
            .await
            .expect_err("must reject conflict");
        assert!(matches!(error, LifecycleError::ControlConflict(_)));
        assert!(paths.control.exists());
    }

    #[test]
    fn runtime_paths_are_private_and_insecure_existing_paths_fail_closed() {
        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        write_metadata(
            &paths,
            &metadata(
                "018f0000-0000-7000-8000-000000000001",
                InstanceMode::Managed,
            ),
        )
        .expect("metadata");
        assert_eq!(
            std::fs::metadata(&paths.root)
                .expect("runtime metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&paths.metadata)
                .expect("instance metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        std::fs::set_permissions(&paths.root, std::fs::Permissions::from_mode(0o755))
            .expect("loosen runtime");
        assert!(matches!(
            paths.prepare().expect_err("insecure mode"),
            LifecycleError::UnsafeRuntimePath { .. }
        ));
    }

    #[test]
    fn stale_cleanup_does_not_follow_or_remove_symlink_targets() {
        use std::os::unix::fs::symlink;

        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let target = temporary.path().join("foreign");
        std::fs::write(&target, b"keep").expect("target");
        symlink(&target, &paths.metadata).expect("symlink");

        assert!(matches!(
            super::remove_owned_regular_file(&paths.metadata)
                .expect_err("symlink must fail closed"),
            LifecycleError::UnsafeRuntimePath { .. }
        ));
        assert!(paths.metadata.is_symlink());
        assert_eq!(std::fs::read(&target).expect("target remains"), b"keep");
    }

    #[test]
    fn database_identity_is_stable_and_path_scoped() {
        let first = database_identity(Path::new("/tmp/yard-a.sqlite3")).expect("first");
        let equivalent =
            database_identity(Path::new("/tmp/./folder/../yard-a.sqlite3")).expect("equivalent");
        let second = database_identity(Path::new("/tmp/yard-b.sqlite3")).expect("second");
        assert_eq!(first, equivalent);
        assert_ne!(first, second);
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn oversized_control_socket_path_fails_before_bind() {
        let root = PathBuf::from("/tmp").join("x".repeat(200));
        let paths = RuntimePaths::new(vec![root.clone()], root);
        assert!(matches!(
            paths
                .validate_socket_length()
                .expect_err("oversized socket path"),
            LifecycleError::UnsafeRuntimePath { .. }
        ));
    }

    #[test]
    fn secret_is_redacted_from_debug_output() {
        let metadata = metadata(
            "018f0000-0000-7000-8000-000000000001",
            InstanceMode::Managed,
        );
        let output = format!("{metadata:?}");
        assert!(output.contains("[redacted]"));
        assert!(!output.contains(&metadata.control_secret));
    }

    #[test]
    fn startup_failure_tail_reads_only_a_bounded_suffix() {
        use std::io::Write as _;

        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let mut log = paths.open_log().expect("log");
        log.write_all(&vec![
            b'x';
            usize::try_from(super::MAX_LOG_TAIL_BYTES)
                .expect("tail size")
                + 1024
        ])
        .expect("large prefix");
        log.write_all(b"\nfinal startup error\n").expect("tail");

        let tail = super::read_log_tail(&paths.log).expect("read tail");
        assert_eq!(tail, "final startup error");
        assert!(tail.len() < usize::try_from(super::MAX_LOG_TAIL_BYTES).expect("tail size"));
    }

    #[test]
    fn managed_log_is_truncated_for_each_launch() {
        use std::io::Write as _;

        let temporary = tempdir().expect("tempdir");
        let paths = RuntimePaths::from_root(temporary.path().join("run"));
        paths.prepare().expect("prepare");
        let mut first = paths.open_log().expect("first log");
        first.write_all(b"previous launch\n").expect("write log");
        drop(first);

        let second = paths.open_log().expect("second log");
        assert_eq!(second.metadata().expect("log metadata").len(), 0);
    }

    #[test]
    fn launch_lock_wait_covers_startup_and_shutdown_budgets() {
        assert!(super::LAUNCH_LOCK_TIMEOUT > super::STARTUP_TIMEOUT);
        assert!(super::LAUNCH_LOCK_TIMEOUT > super::STOP_TIMEOUT);
    }

    use std::path::Path;
    use std::path::PathBuf;
}

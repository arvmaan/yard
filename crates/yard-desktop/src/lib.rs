use serde::Deserialize;
use std::{
    ffi::OsStr,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use url::{Host, Url};

pub const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceMode {
    Managed,
    Foreground,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopping,
    Stopped,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceStatus {
    pub schema_version: u32,
    pub state: ServiceState,
    pub mode: Option<ServiceMode>,
    pub url: Option<String>,
    pub pid: Option<u32>,
    pub log_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitDecision {
    PreventLastWindowExit,
    AllowExit,
}

#[must_use]
pub const fn exit_decision(
    last_window_destroyed: bool,
    programmatic_code: Option<i32>,
) -> ExitDecision {
    if last_window_destroyed && programmatic_code.is_none() {
        ExitDecision::PreventLastWindowExit
    } else {
        ExitDecision::AllowExit
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Attach,
    Start,
    Wait,
}

/// Select the desktop action represented by a status document.
///
/// # Errors
///
/// Returns an error when an active service is foreground-owned.
pub fn service_action(status: &ServiceStatus) -> Result<ServiceAction, String> {
    if status.schema_version != 1 {
        return Err(format!(
            "yard service status schema {} is not supported",
            status.schema_version
        ));
    }
    match (status.state, status.mode) {
        (ServiceState::Running, Some(ServiceMode::Managed)) => Ok(ServiceAction::Attach),
        (ServiceState::Running, Some(ServiceMode::Foreground)) => Err(
            "a foreground Yard service owns this database; use its browser UI or stop it in its owning terminal before opening Yard.app"
                .to_owned(),
        ),
        (ServiceState::Running, None) => {
            Err("running yard service status omitted its owner mode".to_owned())
        }
        (ServiceState::Stopped, _) => Ok(ServiceAction::Start),
        (ServiceState::Stopping, _) => Ok(ServiceAction::Wait),
    }
}

#[must_use]
pub fn can_stop_service(status: &ServiceStatus) -> bool {
    matches!(
        (status.state, status.mode),
        (
            ServiceState::Running | ServiceState::Stopping,
            Some(ServiceMode::Managed)
        )
    )
}

impl ServiceStatus {
    /// Convert verified running status into a desktop navigation target.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported schema versions, non-running states,
    /// non-managed owners, missing URLs, or URLs outside loopback HTTP.
    pub fn ready(self) -> Result<ReadyService, String> {
        if self.schema_version != 1 {
            return Err(format!(
                "yard service status schema {} is not supported",
                self.schema_version
            ));
        }
        match service_action(&self)? {
            ServiceAction::Attach => {}
            ServiceAction::Start | ServiceAction::Wait => {
                return Err(format!("yard service is {}", self.state.name()));
            }
        }
        let url = self
            .url
            .ok_or_else(|| "yard service status omitted its URL".to_owned())?;
        validate_loopback_url(&url)?;
        Ok(ReadyService { url })
    }
}

impl ServiceState {
    const fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug)]
pub struct ReadyService {
    pub url: String,
}

/// Validate that service discovery returned a dynamic loopback HTTP URL.
///
/// # Errors
///
/// Returns an error for malformed, credential-bearing, non-HTTP,
/// non-loopback, or portless URLs.
pub fn validate_loopback_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| "yard service URL is invalid".to_owned())?;
    if url.scheme() != "http" || url.username() != "" || url.password().is_some() {
        return Err("yard service URL must use loopback HTTP without credentials".to_owned());
    }
    let loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    if !loopback {
        return Err("yard service URL must use a loopback host".to_owned());
    }
    if url.port().is_none() {
        return Err("yard service URL must include its dynamic port".to_owned());
    }
    Ok(())
}

#[derive(Debug)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// Run one Yard lifecycle command with bounded time and captured output.
///
/// # Errors
///
/// Returns an error when the child cannot be started, inspected, terminated,
/// reaped, or when an output reader fails.
pub fn run_bounded_command<I, S, E, K, V>(
    binary: &Path,
    args: I,
    environment: E,
    deadline: Duration,
) -> Result<CommandOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let expires = Instant::now() + deadline;
    let mut child = Command::new(binary)
        .args(args)
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", binary.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "yard command stdout was unavailable".to_owned())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "yard command stderr was unavailable".to_owned())?;
    set_nonblocking(&stdout, "stdout")?;
    set_nonblocking(&stderr, "stderr")?;
    let mut stdout_capture = OutputCapture::new();
    let mut stderr_capture = OutputCapture::new();
    let mut child_status = None;

    loop {
        drain_available(&mut stdout, &mut stdout_capture, "stdout", expires)?;
        drain_available(&mut stderr, &mut stderr_capture, "stderr", expires)?;
        if child_status.is_none() {
            child_status = child
                .try_wait()
                .map_err(|error| format!("could not inspect Yard command: {error}"))?;
        }
        if let Some(status) = child_status
            && stdout_capture.eof
            && stderr_capture.eof
        {
            return Ok(CommandOutput {
                status,
                stdout: stdout_capture.retained,
                stderr: stderr_capture.retained,
                stdout_truncated: stdout_capture.truncated,
                stderr_truncated: stderr_capture.truncated,
            });
        }
        if Instant::now() >= expires {
            if child_status.is_none() {
                child.kill().map_err(|error| {
                    format!("timed out and could not terminate Yard command: {error}")
                })?;
                reap_terminated_child(&mut child)?;
                return Err(format!(
                    "Yard command timed out after {} seconds and was terminated",
                    deadline.as_secs()
                ));
            }
            return Err(format!(
                "Yard command output remained open after its process exited and the {}-second deadline elapsed",
                deadline.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct OutputCapture {
    retained: Vec<u8>,
    truncated: bool,
    eof: bool,
}

impl OutputCapture {
    fn new() -> Self {
        Self {
            retained: Vec::with_capacity(MAX_COMMAND_OUTPUT_BYTES),
            truncated: false,
            eof: false,
        }
    }
}

fn set_nonblocking(pipe: &impl std::os::fd::AsFd, stream: &str) -> Result<(), String> {
    let flags = rustix::fs::fcntl_getfl(pipe)
        .map_err(|error| format!("could not inspect Yard command {stream}: {error}"))?;
    rustix::fs::fcntl_setfl(pipe, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|error| format!("could not make Yard command {stream} nonblocking: {error}"))
}

fn drain_available(
    reader: &mut impl Read,
    capture: &mut OutputCapture,
    stream: &str,
    expires: Instant,
) -> Result<(), String> {
    let mut buffer = [0_u8; 8192];
    loop {
        if Instant::now() >= expires {
            return Ok(());
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                capture.eof = true;
                return Ok(());
            }
            Ok(read) => {
                let available = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(capture.retained.len());
                let kept = read.min(available);
                capture.retained.extend_from_slice(&buffer[..kept]);
                capture.truncated |= kept < read;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(format!("could not read Yard command {stream}: {error}")),
        }
    }
}

fn reap_terminated_child(child: &mut std::process::Child) -> Result<(), String> {
    let expires = Instant::now() + Duration::from_secs(3);
    loop {
        match child
            .try_wait()
            .map_err(|error| format!("could not reap terminated Yard command: {error}"))?
        {
            Some(_) => return Ok(()),
            None if Instant::now() < expires => thread::sleep(Duration::from_millis(10)),
            None => {
                return Err(
                    "terminated Yard command did not exit within the three-second reap deadline"
                        .to_owned(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::Path, time::Duration};

    use super::{
        ExitDecision, MAX_COMMAND_OUTPUT_BYTES, ServiceAction, ServiceMode, ServiceState,
        ServiceStatus, can_stop_service, exit_decision, run_bounded_command, service_action,
        validate_loopback_url,
    };

    fn status(document: &str) -> ServiceStatus {
        serde_json::from_str(document).unwrap()
    }

    #[test]
    fn exit_decision_only_prevents_last_window_auto_exit() {
        assert_eq!(
            exit_decision(true, None),
            ExitDecision::PreventLastWindowExit
        );
        assert_eq!(exit_decision(false, None), ExitDecision::AllowExit);
        assert_eq!(exit_decision(true, Some(0)), ExitDecision::AllowExit);
    }

    #[test]
    fn status_actions_keep_attach_start_and_wait_distinct() {
        let managed = status(
            r#"{"schema_version":1,"state":"running","mode":"managed","url":"http://127.0.0.1:4317/","pid":42,"log_path":null}"#,
        );
        let stopped = status(
            r#"{"schema_version":1,"state":"stopped","mode":null,"url":null,"pid":null,"log_path":null}"#,
        );
        let stopping = status(
            r#"{"schema_version":1,"state":"stopping","mode":"managed","url":"http://127.0.0.1:4317/","pid":42,"log_path":null}"#,
        );
        assert_eq!(service_action(&managed).unwrap(), ServiceAction::Attach);
        assert!(can_stop_service(&managed));
        assert_eq!(service_action(&stopped).unwrap(), ServiceAction::Start);
        assert!(!can_stop_service(&stopped));
        assert_eq!(service_action(&stopping).unwrap(), ServiceAction::Wait);
        assert!(can_stop_service(&stopping));
    }

    #[test]
    fn accepts_only_running_managed_v1_status_on_loopback() {
        let status = status(
            r#"{"schema_version":1,"state":"running","mode":"managed","url":"http://127.0.0.1:43210/","pid":42,"log_path":"/tmp/yard.log"}"#,
        );
        assert_eq!(status.mode, Some(ServiceMode::Managed));
        assert_eq!(status.state, ServiceState::Running);
        assert_eq!(status.ready().unwrap().url, "http://127.0.0.1:43210/");
    }

    #[test]
    fn rejects_foreground_unknown_and_incompatible_status() {
        let foreground = status(
            r#"{"schema_version":1,"state":"running","mode":"foreground","url":"http://127.0.0.1:4317/","pid":42,"log_path":null}"#,
        );
        assert!(!can_stop_service(&foreground));
        assert!(foreground.ready().unwrap_err().contains("foreground"));
        assert!(serde_json::from_str::<ServiceStatus>(
            r#"{"schema_version":1,"state":"running","mode":"future","url":"http://127.0.0.1:4317/","pid":42,"log_path":null}"#,
        )
        .is_err());
        let incompatible = status(
            r#"{"schema_version":2,"state":"stopped","mode":null,"url":null,"pid":null,"log_path":null}"#,
        );
        assert!(service_action(&incompatible).is_err());
        assert!(incompatible.ready().is_err());
    }

    #[test]
    fn rejects_non_loopback_transitional_and_incomplete_status() {
        for url in [
            "https://127.0.0.1:4317/",
            "http://example.com:4317/",
            "http://127.0.0.1.evil:4317/",
            "http://user@127.0.0.1:4317/",
            "http://127.0.0.1/",
        ] {
            assert!(validate_loopback_url(url).is_err(), "{url}");
        }
        for document in [
            r#"{"schema_version":1,"state":"stopping","mode":"managed","url":"http://127.0.0.1:4317/","pid":42,"log_path":null}"#,
            r#"{"schema_version":1,"state":"running","mode":"managed","url":null,"pid":42,"log_path":null}"#,
        ] {
            assert!(status(document).ready().is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_output_is_capped_while_pipes_are_fully_drained() {
        let output = run_bounded_command(
            Path::new("/bin/sh"),
            ["-c", "yes x | head -c 100000; yes e | head -c 100000 >&2"],
            Vec::<(OsString, OsString)>::new(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), MAX_COMMAND_OUTPUT_BYTES);
        assert_eq!(output.stderr.len(), MAX_COMMAND_OUTPUT_BYTES);
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
    }

    #[cfg(unix)]
    #[test]
    fn inherited_output_pipe_respects_the_original_deadline() {
        let started = std::time::Instant::now();
        let error = run_bounded_command(
            Path::new("/bin/sh"),
            ["-c", "sleep 10 & exit 0"],
            Vec::<(OsString, OsString)>::new(),
            Duration::from_millis(150),
        )
        .unwrap_err();
        assert!(error.contains("output remained open"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_terminates_and_reaps_child() {
        let started = std::time::Instant::now();
        let error = run_bounded_command(
            Path::new("/bin/sh"),
            ["-c", "exec sleep 10"],
            Vec::<(OsString, OsString)>::new(),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

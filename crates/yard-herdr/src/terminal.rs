use std::{
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::timeout,
};

use crate::HerdrConfig;

pub const MIN_TERMINAL_COLS: u16 = 20;
pub const MAX_TERMINAL_COLS: u16 = 400;
pub const MIN_TERMINAL_ROWS: u16 = 5;
pub const MAX_TERMINAL_ROWS: u16 = 200;
/// Largest decoded `terminal.input` payload. A whole bracketed paste travels
/// as one command so Herdr can re-bracket it for the pane; this stays well
/// under Herdr's 1 MiB per-message input limit.
pub const MAX_TERMINAL_INPUT_BYTES: usize = 512 * 1024;
/// Largest NDJSON command line. JSON escapes a control byte as `\u00XX`, so
/// the bound covers a maximal input made entirely of control bytes.
pub const MAX_TERMINAL_COMMAND_LINE_BYTES: usize = MAX_TERMINAL_INPUT_BYTES * 6 + 1024;
pub const MAX_TERMINAL_EVENT_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Most repeats of one [`TerminalScroll`] sent to Herdr in a single write.
pub const MAX_TERMINAL_SCROLL_REPEAT: u8 = 16;

const MAX_TERMINAL_STDERR_BYTES: usize = 64 * 1024;
const MAX_TERMINAL_TARGET_BYTES: usize = 256;
const TERMINAL_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalDimensions {
    cols: u16,
    rows: u16,
}

impl TerminalDimensions {
    /// Create terminal dimensions accepted at the Herdr process boundary.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError::InvalidDimensions`] when either dimension
    /// falls outside Yard's supported terminal range.
    pub fn new(cols: u16, rows: u16) -> Result<Self, HerdrTerminalError> {
        if !dimensions_are_valid(cols, rows) {
            return Err(HerdrTerminalError::InvalidDimensions { cols, rows });
        }
        Ok(Self { cols, rows })
    }

    #[must_use]
    pub fn cols(self) -> u16 {
        self.cols
    }

    #[must_use]
    pub fn rows(self) -> u16 {
        self.rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTerminalRequest {
    session: String,
    terminal_id: String,
    dimensions: TerminalDimensions,
}

impl OpenTerminalRequest {
    /// Create a validated request for one Herdr terminal controller process.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError::InvalidTarget`] for an empty or oversized
    /// session name or terminal ID.
    pub fn new(
        session: impl Into<String>,
        terminal_id: impl Into<String>,
        dimensions: TerminalDimensions,
    ) -> Result<Self, HerdrTerminalError> {
        let session = session.into();
        validate_target("session", &session)?;
        let terminal_id = terminal_id.into();
        validate_target("terminal_id", &terminal_id)?;
        Ok(Self {
            session,
            terminal_id,
            dimensions,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    Text(String),
    BytesBase64(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalScrollDirection {
    Up,
    Down,
}

/// How Herdr should interpret a scroll request.
///
/// `Wheel` is routed like a mouse wheel: mouse-reporting apps receive wheel
/// reports, alternate-scroll apps receive arrow keys, and everything else
/// scrolls Herdr's own pane scrollback. `PageKey` scrolls Herdr's scrollback
/// at a shell-like prompt and otherwise forwards PageUp/PageDown to the app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalScrollSource {
    #[default]
    Wheel,
    PageKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalScroll {
    direction: TerminalScrollDirection,
    lines: u16,
    source: TerminalScrollSource,
    column: Option<u16>,
    row: Option<u16>,
    repeat: u8,
}

impl TerminalScroll {
    /// Create a bounded scroll request for Herdr's `terminal.scroll` command.
    ///
    /// `column` and `row` are the zero-based pointer cell used for wheel
    /// reports sent to mouse-aware apps.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError::InvalidScroll`] when `lines` is zero or
    /// taller than the largest terminal, or the pointer cell lies outside the
    /// largest terminal.
    pub fn new(
        direction: TerminalScrollDirection,
        lines: u16,
        source: TerminalScrollSource,
        column: Option<u16>,
        row: Option<u16>,
    ) -> Result<Self, HerdrTerminalError> {
        if !(1..=MAX_TERMINAL_ROWS).contains(&lines)
            || column.is_some_and(|column| column >= MAX_TERMINAL_COLS)
            || row.is_some_and(|row| row >= MAX_TERMINAL_ROWS)
        {
            return Err(HerdrTerminalError::InvalidScroll);
        }
        Ok(Self {
            direction,
            lines,
            source,
            column,
            row,
            repeat: 1,
        })
    }

    /// Send this scroll `repeat` times back to back in one write.
    ///
    /// Herdr turns each command into one wheel report (or arrow key) for a
    /// mouse-aware app, so a batch of steps must stay separate commands. When
    /// they arrive together, Herdr drains them in one loop pass and answers
    /// with one render instead of rendering the first at once and the rest a
    /// render interval later.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError::InvalidScroll`] when `repeat` is zero or
    /// above [`MAX_TERMINAL_SCROLL_REPEAT`].
    pub fn repeated(self, repeat: u8) -> Result<Self, HerdrTerminalError> {
        if !(1..=MAX_TERMINAL_SCROLL_REPEAT).contains(&repeat) {
            return Err(HerdrTerminalError::InvalidScroll);
        }
        Ok(Self { repeat, ..self })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCommand {
    Input(TerminalInput),
    Resize(TerminalDimensions),
    Scroll(TerminalScroll),
    /// Return the pane's viewport to the live bottom. Herdr keeps one scroll
    /// offset per pane and resets it only on input, so this is sent as an
    /// empty `terminal.input`: Herdr applies its scroll reset and writes no
    /// bytes to the pane.
    ScrollReset,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalEncoding {
    Ansi,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TerminalFrame {
    pub bytes: String,
    pub encoding: TerminalEncoding,
    pub seq: u64,
    pub width: u16,
    pub height: u16,
    pub full: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TerminalClosed {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum TerminalEvent {
    #[serde(rename = "terminal.frame")]
    Frame(TerminalFrame),
    #[serde(rename = "terminal.closed")]
    Closed(TerminalClosed),
}

#[derive(Debug, Error)]
pub enum HerdrTerminalError {
    #[error("terminal dimensions {cols}x{rows} are outside the supported range")]
    InvalidDimensions { cols: u16, rows: u16 },
    #[error(
        "terminal scroll must move 1 to {MAX_TERMINAL_ROWS} lines at a cell inside the terminal"
    )]
    InvalidScroll,
    #[error("terminal {field} must contain 1 to {MAX_TERMINAL_TARGET_BYTES} bytes")]
    InvalidTarget { field: &'static str },
    #[error("terminal input must not be empty")]
    EmptyInput,
    #[error("terminal input exceeds {MAX_TERMINAL_INPUT_BYTES} decoded bytes")]
    InputTooLarge,
    #[error("terminal input bytes are not canonical base64")]
    InvalidBase64Input,
    #[error("serialized terminal command exceeds {MAX_TERMINAL_COMMAND_LINE_BYTES} bytes")]
    CommandLineTooLarge,
    #[error("failed to serialize Herdr terminal command: {0}")]
    CommandEncode(#[source] serde_json::Error),
    #[error("failed to spawn Herdr terminal controller: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed to write Herdr terminal command: {0}")]
    Stdin(#[source] io::Error),
    #[error("failed to read Herdr terminal output: {0}")]
    Stdout(#[source] io::Error),
    #[error("Herdr terminal output line exceeded {MAX_TERMINAL_EVENT_LINE_BYTES} bytes")]
    EventLineTooLarge,
    #[error("Herdr terminal output ended with unterminated NDJSON")]
    UnterminatedEvent,
    #[error("Herdr terminal output contained malformed NDJSON: {0}")]
    MalformedEvent(#[source] serde_json::Error),
    #[error("Herdr terminal output contained invalid {0}")]
    InvalidEvent(&'static str),
    #[error("failed to inspect or wait for Herdr terminal process: {0}")]
    Wait(#[source] io::Error),
    #[error("failed to kill Herdr terminal process: {0}")]
    Kill(#[source] io::Error),
    #[error("failed to drain Herdr terminal stderr: {0}")]
    Stderr(#[source] io::Error),
    #[error("Herdr terminal stderr task failed: {0}")]
    StderrTask(String),
    #[error(
        "Herdr terminal process exited unexpectedly with {status}; \
         stderr (truncated: {stderr_truncated}): {stderr}"
    )]
    UnexpectedExit {
        status: ExitStatus,
        stderr: String,
        stderr_truncated: bool,
    },
    #[error("Herdr terminal process closed before accepting the command")]
    Closed,
    #[error("Herdr terminal process did not close after release")]
    CloseTimeout,
}

pub struct HerdrTerminal {
    child: Option<Child>,
    exit_status: Option<ExitStatus>,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Bytes of the NDJSON line currently being read. It lives on the
    /// terminal, not in the read future, so [`Self::next_event`] stays
    /// cancel-safe when a `select!` drops it between partial writes.
    stdout_line: Vec<u8>,
    stderr_task: Option<JoinHandle<io::Result<StderrCapture>>>,
    saw_closed: bool,
    release_sent: bool,
    /// A scroll was sent since the last input or scroll reset, so Herdr's
    /// per-pane viewport may be scrolled back. Release resets it first, so a
    /// closed Yard view never leaves the pane (Herdr's own client, or the
    /// next viewer) away from the latest output.
    scrolled_since_input: bool,
}

impl HerdrTerminal {
    pub(crate) fn spawn(
        config: &HerdrConfig,
        request: OpenTerminalRequest,
    ) -> Result<Self, HerdrTerminalError> {
        let OpenTerminalRequest {
            session,
            terminal_id,
            dimensions,
        } = request;
        let mut command = controller_command(config, &session, &terminal_id, dimensions);
        let mut child = command.spawn().map_err(HerdrTerminalError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .expect("piped Herdr terminal stdin must be present");
        let stdout = child
            .stdout
            .take()
            .expect("piped Herdr terminal stdout must be present");
        let stderr = child
            .stderr
            .take()
            .expect("piped Herdr terminal stderr must be present");
        let stderr_task = tokio::spawn(drain_stderr(stderr));

        Ok(Self {
            child: Some(child),
            exit_status: None,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stdout_line: Vec::new(),
            stderr_task: Some(stderr_task),
            saw_closed: false,
            release_sent: false,
            scrolled_since_input: false,
        })
    }

    /// Read and validate the next Herdr terminal event.
    ///
    /// `Ok(None)` is returned only after a `terminal.closed` event and a
    /// successful controller exit.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel-safe. Herdr writes one frame line in several
    /// `write(2)` calls, so a relay `select!` may drop this future after only
    /// part of a line arrived; those bytes are kept and the next call resumes
    /// the same line instead of parsing its tail as a new event.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError`] for process I/O, malformed or oversized
    /// NDJSON, invalid frame fields, and exits not preceded by
    /// `terminal.closed`.
    pub async fn next_event(&mut self) -> Result<Option<TerminalEvent>, HerdrTerminalError> {
        let Some(line) = read_event_line(&mut self.stdout, &mut self.stdout_line).await? else {
            return self.finish_after_stdout().await;
        };
        let event = parse_event(&line)?;
        if matches!(event, TerminalEvent::Closed(_)) {
            self.saw_closed = true;
        }
        Ok(Some(event))
    }

    /// Send one validated, newline-delimited command to Herdr.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError`] for invalid input, serialization or
    /// process I/O failures, and commands sent after release or closure.
    pub async fn send(&mut self, command: TerminalCommand) -> Result<(), HerdrTerminalError> {
        if matches!(command, TerminalCommand::Release) && self.release_sent {
            return Ok(());
        }
        if self.saw_closed || self.release_sent {
            return Err(HerdrTerminalError::Closed);
        }

        let release = matches!(command, TerminalCommand::Release);
        let mut line = if release && self.scrolled_since_input {
            encode_command(&TerminalCommand::ScrollReset)?
        } else {
            Vec::new()
        };
        line.extend(encode_command(&command)?);
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(HerdrTerminalError::Closed);
        };
        if let Err(source) = stdin.write_all(&line).await {
            return self.write_error(source).await;
        }
        if let Err(source) = stdin.flush().await {
            return self.write_error(source).await;
        }
        match command {
            TerminalCommand::Scroll(_) => self.scrolled_since_input = true,
            TerminalCommand::Input(_) | TerminalCommand::ScrollReset => {
                self.scrolled_since_input = false;
            }
            TerminalCommand::Resize(_) | TerminalCommand::Release => {}
        }
        if release {
            self.release_sent = true;
            self.stdin.take();
        }
        Ok(())
    }

    /// Ask Herdr to release terminal control and close its stdin.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError`] when the release message cannot be
    /// serialized or written. Call [`Self::next_event`] until it returns
    /// `None`, or use [`Self::close`] to wait with forced cleanup.
    pub async fn release(&mut self) -> Result<(), HerdrTerminalError> {
        self.send(TerminalCommand::Release).await
    }

    /// Release terminal control and wait for Herdr to close and exit.
    ///
    /// If Herdr does not close promptly, the child is killed and reaped before
    /// this method returns.
    ///
    /// # Errors
    ///
    /// Returns [`HerdrTerminalError`] when release, output validation, process
    /// exit, or cleanup fails.
    pub async fn close(mut self) -> Result<(), HerdrTerminalError> {
        if !self.saw_closed
            && !self.release_sent
            && let Err(error) = self.send(TerminalCommand::Release).await
        {
            self.kill_and_reap().await?;
            return Err(error);
        }

        let close = async {
            while self.next_event().await?.is_some() {}
            Ok::<(), HerdrTerminalError>(())
        };
        if let Ok(result) = timeout(TERMINAL_CLOSE_TIMEOUT, close).await {
            if let Err(error) = result {
                self.kill_and_reap().await?;
                return Err(error);
            }
            Ok(())
        } else {
            self.kill_and_reap().await?;
            Err(HerdrTerminalError::CloseTimeout)
        }
    }

    async fn write_error(&mut self, source: io::Error) -> Result<(), HerdrTerminalError> {
        let status = self
            .child
            .as_mut()
            .ok_or(HerdrTerminalError::Closed)?
            .try_wait()
            .map_err(HerdrTerminalError::Wait)?;
        let Some(status) = status else {
            return Err(HerdrTerminalError::Stdin(source));
        };
        self.child.take();
        let stderr = self.take_stderr().await?;
        Err(unexpected_exit(status, stderr))
    }

    async fn finish_after_stdout(&mut self) -> Result<Option<TerminalEvent>, HerdrTerminalError> {
        // Keep the reaped status so a call cancelled while draining stderr
        // can resume without losing it.
        let status = if let Some(status) = self.exit_status {
            status
        } else {
            let status = self
                .child
                .as_mut()
                .ok_or(HerdrTerminalError::Closed)?
                .wait()
                .await
                .map_err(HerdrTerminalError::Wait)?;
            self.exit_status = Some(status);
            self.child.take();
            status
        };
        self.stdin.take();
        let stderr = self.take_stderr().await?;
        if self.saw_closed && status.success() {
            return Ok(None);
        }
        Err(unexpected_exit(status, stderr))
    }

    async fn kill_and_reap(&mut self) -> Result<(), HerdrTerminalError> {
        self.stdin.take();
        let Some(child) = self.child.as_mut() else {
            self.take_stderr().await?;
            return Ok(());
        };
        if child
            .try_wait()
            .map_err(HerdrTerminalError::Wait)?
            .is_none()
        {
            child.start_kill().map_err(HerdrTerminalError::Kill)?;
        }
        child.wait().await.map_err(HerdrTerminalError::Wait)?;
        self.child.take();
        self.take_stderr().await?;
        Ok(())
    }

    async fn take_stderr(&mut self) -> Result<StderrCapture, HerdrTerminalError> {
        let Some(task) = self.stderr_task.as_mut() else {
            return Ok(StderrCapture::default());
        };
        let result = task
            .await
            .map_err(|error| HerdrTerminalError::StderrTask(error.to_string()))?
            .map_err(HerdrTerminalError::Stderr);
        self.stderr_task.take();
        result
    }
}

fn controller_command(
    config: &HerdrConfig,
    session: &str,
    terminal_id: &str,
    dimensions: TerminalDimensions,
) -> Command {
    let mut command = Command::new(&config.binary);
    command
        .arg("--session")
        .arg(session)
        .args(["terminal", "session", "control"])
        .arg(terminal_id)
        .arg("--cols")
        .arg(dimensions.cols.to_string())
        .arg("--rows")
        .arg(dimensions.rows.to_string())
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

impl Drop for HerdrTerminal {
    fn drop(&mut self) {
        self.stdin.take();
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();
        let stderr_task = self.stderr_task.take();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = child.wait().await;
                if let Some(task) = stderr_task {
                    let _ = task.await;
                }
            });
        }
    }
}

#[derive(Debug, Default)]
struct StderrCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn drain_stderr(stderr: ChildStderr) -> io::Result<StderrCapture> {
    drain_stderr_from(stderr).await
}

async fn drain_stderr_from(mut stderr: impl AsyncRead + Unpin) -> io::Result<StderrCapture> {
    let mut capture = StderrCapture::default();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = stderr.read(&mut chunk).await?;
        if count == 0 {
            return Ok(capture);
        }
        let remaining = MAX_TERMINAL_STDERR_BYTES.saturating_sub(capture.bytes.len());
        let retained = count.min(remaining);
        capture.bytes.extend_from_slice(&chunk[..retained]);
        capture.truncated |= retained < count;
    }
}

async fn read_event_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, HerdrTerminalError> {
    read_bounded_line(reader, line, MAX_TERMINAL_EVENT_LINE_BYTES).await
}

/// Read one newline-terminated line of at most `max_bytes` into `line`.
///
/// Cancel-safe: `read_until` appends each consumed chunk to `line` before it
/// yields, so when the future is dropped mid-line the partial bytes remain in
/// `line` and the next call continues that line. A completed line is taken
/// out of `line`, leaving it empty for the next one.
async fn read_bounded_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, HerdrTerminalError> {
    let remaining = max_bytes.saturating_add(1).saturating_sub(line.len());
    let limit = u64::try_from(remaining).unwrap_or(u64::MAX);
    reader
        .take(limit)
        .read_until(b'\n', line)
        .await
        .map_err(HerdrTerminalError::Stdout)?;
    if line.len() > max_bytes {
        line.clear();
        return Err(HerdrTerminalError::EventLineTooLarge);
    }
    if line.last() == Some(&b'\n') {
        return Ok(Some(std::mem::take(line)));
    }
    // `read_until` stops short of a newline only at end of output.
    if line.is_empty() {
        return Ok(None);
    }
    line.clear();
    Err(HerdrTerminalError::UnterminatedEvent)
}

fn parse_event(line: &[u8]) -> Result<TerminalEvent, HerdrTerminalError> {
    let event: TerminalEvent =
        serde_json::from_slice(line).map_err(HerdrTerminalError::MalformedEvent)?;
    match &event {
        TerminalEvent::Frame(frame) => {
            if !dimensions_are_valid(frame.width, frame.height) {
                return Err(HerdrTerminalError::InvalidEvent("frame dimensions"));
            }
            if !is_canonical_base64(&frame.bytes, true) {
                return Err(HerdrTerminalError::InvalidEvent(
                    "base64 terminal.frame bytes",
                ));
            }
        }
        TerminalEvent::Closed(_) => {}
    }
    Ok(event)
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum WireCommand<'a> {
    #[serde(rename = "terminal.input")]
    InputText { text: &'a str },
    #[serde(rename = "terminal.input")]
    InputBytes { bytes: &'a str },
    #[serde(rename = "terminal.resize")]
    Resize { cols: u16, rows: u16 },
    #[serde(rename = "terminal.scroll")]
    Scroll {
        direction: TerminalScrollDirection,
        lines: u16,
        source: TerminalScrollSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        column: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        row: Option<u16>,
    },
    #[serde(rename = "terminal.release")]
    Release,
}

fn encode_command(command: &TerminalCommand) -> Result<Vec<u8>, HerdrTerminalError> {
    let wire = match command {
        TerminalCommand::Input(TerminalInput::Text(text)) => {
            if text.is_empty() {
                return Err(HerdrTerminalError::EmptyInput);
            }
            if text.len() > MAX_TERMINAL_INPUT_BYTES {
                return Err(HerdrTerminalError::InputTooLarge);
            }
            WireCommand::InputText { text }
        }
        TerminalCommand::Input(TerminalInput::BytesBase64(bytes)) => {
            validate_input_base64(bytes)?;
            WireCommand::InputBytes { bytes }
        }
        TerminalCommand::Resize(dimensions) => WireCommand::Resize {
            cols: dimensions.cols,
            rows: dimensions.rows,
        },
        TerminalCommand::Scroll(scroll) => WireCommand::Scroll {
            direction: scroll.direction,
            lines: scroll.lines,
            source: scroll.source,
            column: scroll.column,
            row: scroll.row,
        },
        TerminalCommand::ScrollReset => WireCommand::InputText { text: "" },
        TerminalCommand::Release => WireCommand::Release,
    };

    let mut line = serde_json::to_vec(&wire).map_err(HerdrTerminalError::CommandEncode)?;
    line.push(b'\n');
    if line.len() > MAX_TERMINAL_COMMAND_LINE_BYTES {
        return Err(HerdrTerminalError::CommandLineTooLarge);
    }
    if let TerminalCommand::Scroll(scroll) = command {
        return Ok(line.repeat(usize::from(scroll.repeat)));
    }
    Ok(line)
}

fn validate_input_base64(bytes: &str) -> Result<(), HerdrTerminalError> {
    if bytes.is_empty() {
        return Err(HerdrTerminalError::EmptyInput);
    }
    let max_encoded_bytes = MAX_TERMINAL_INPUT_BYTES.div_ceil(3) * 4;
    if bytes.len() > max_encoded_bytes {
        return Err(HerdrTerminalError::InputTooLarge);
    }
    if !is_canonical_base64(bytes, false) {
        return Err(HerdrTerminalError::InvalidBase64Input);
    }
    let padding = bytes.bytes().rev().take_while(|byte| *byte == b'=').count();
    let decoded_bytes = (bytes.len() / 4)
        .checked_mul(3)
        .and_then(|length| length.checked_sub(padding))
        .ok_or(HerdrTerminalError::InputTooLarge)?;
    if decoded_bytes > MAX_TERMINAL_INPUT_BYTES {
        return Err(HerdrTerminalError::InputTooLarge);
    }
    Ok(())
}

fn is_canonical_base64(value: &str, allow_empty: bool) -> bool {
    if value.is_empty() {
        return allow_empty;
    }
    let bytes = value.as_bytes();
    if bytes.len() % 4 != 0 {
        return false;
    }

    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return false;
    }
    let data_length = bytes.len() - padding;
    if data_length == 0
        || !bytes[..data_length]
            .iter()
            .all(|byte| base64_value(*byte).is_some())
        || !bytes[data_length..].iter().all(|byte| *byte == b'=')
    {
        return false;
    }

    match padding {
        0 => true,
        1 => base64_value(bytes[data_length - 1]).is_some_and(|value| value.trailing_zeros() >= 2),
        2 => base64_value(bytes[data_length - 1]).is_some_and(|value| value.trailing_zeros() >= 4),
        _ => false,
    }
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn dimensions_are_valid(cols: u16, rows: u16) -> bool {
    (MIN_TERMINAL_COLS..=MAX_TERMINAL_COLS).contains(&cols)
        && (MIN_TERMINAL_ROWS..=MAX_TERMINAL_ROWS).contains(&rows)
}

fn validate_target(field: &'static str, value: &str) -> Result<(), HerdrTerminalError> {
    if value.is_empty() || value.len() > MAX_TERMINAL_TARGET_BYTES {
        return Err(HerdrTerminalError::InvalidTarget { field });
    }
    Ok(())
}

fn unexpected_exit(status: ExitStatus, capture: StderrCapture) -> HerdrTerminalError {
    let StderrCapture { bytes, truncated } = capture;
    HerdrTerminalError::UnexpectedExit {
        status,
        stderr: String::from_utf8_lossy(&bytes).trim().to_owned(),
        stderr_truncated: truncated,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        time::Duration,
    };

    use rustix::{
        io::Errno,
        process::{Pid, test_kill_process},
    };
    use tokio::{
        io::{AsyncWriteExt, BufReader},
        time::{interval, sleep, timeout},
    };

    use super::{
        HerdrTerminal, HerdrTerminalError, MAX_TERMINAL_COLS, MAX_TERMINAL_COMMAND_LINE_BYTES,
        MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_ROWS, MAX_TERMINAL_SCROLL_REPEAT,
        OpenTerminalRequest, TerminalClosed, TerminalCommand, TerminalDimensions, TerminalEncoding,
        TerminalEvent, TerminalFrame, TerminalInput, TerminalScroll, TerminalScrollDirection,
        TerminalScrollSource, controller_command, encode_command, parse_event, read_bounded_line,
    };
    use crate::HerdrConfig;

    #[test]
    fn parses_terminal_frames_with_sequence_and_closed_events() {
        let frame = parse_event(
            br#"{"bytes":"G1sySg==","encoding":"ansi","full":true,"height":24,"seq":42,"type":"terminal.frame","width":80}
"#,
        )
        .unwrap();
        assert_eq!(
            frame,
            TerminalEvent::Frame(TerminalFrame {
                bytes: "G1sySg==".to_owned(),
                encoding: TerminalEncoding::Ansi,
                seq: 42,
                width: 80,
                height: 24,
                full: true,
            })
        );
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            serde_json::json!({
                "type": "terminal.frame",
                "bytes": "G1sySg==",
                "encoding": "ansi",
                "seq": 42,
                "width": 80,
                "height": 24,
                "full": true
            })
        );

        let closed = parse_event(
            br#"{"reason":"detached","type":"terminal.closed"}
"#,
        )
        .unwrap();
        assert_eq!(
            closed,
            TerminalEvent::Closed(TerminalClosed {
                reason: "detached".to_owned(),
            })
        );
    }

    #[test]
    fn serializes_all_stdin_commands_as_ndjson() {
        let dimensions = TerminalDimensions::new(100, 40).unwrap();
        assert_eq!(
            encode_command(&TerminalCommand::Input(TerminalInput::Text(
                "ls\r".to_owned()
            )))
            .unwrap(),
            br#"{"type":"terminal.input","text":"ls\r"}
"#
        );
        assert_eq!(
            encode_command(&TerminalCommand::Input(TerminalInput::BytesBase64(
                "G1tB".to_owned()
            )))
            .unwrap(),
            br#"{"type":"terminal.input","bytes":"G1tB"}
"#
        );
        assert_eq!(
            encode_command(&TerminalCommand::Resize(dimensions)).unwrap(),
            br#"{"type":"terminal.resize","cols":100,"rows":40}
"#
        );
        assert_eq!(
            encode_command(&TerminalCommand::Release).unwrap(),
            b"{\"type\":\"terminal.release\"}\n"
        );
    }

    #[test]
    fn serializes_scroll_commands_for_herdr_terminal_scroll() {
        // Herdr 0.9.x `terminal session control` accepts
        // {"type":"terminal.scroll","direction","lines","source","column","row"}.
        let wheel = TerminalScroll::new(
            TerminalScrollDirection::Up,
            3,
            TerminalScrollSource::Wheel,
            Some(12),
            Some(4),
        )
        .unwrap();
        assert_eq!(
            encode_command(&TerminalCommand::Scroll(wheel)).unwrap(),
            br#"{"type":"terminal.scroll","direction":"up","lines":3,"source":"wheel","column":12,"row":4}
"#
        );
        let page = TerminalScroll::new(
            TerminalScrollDirection::Down,
            23,
            TerminalScrollSource::PageKey,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            encode_command(&TerminalCommand::Scroll(page)).unwrap(),
            br#"{"type":"terminal.scroll","direction":"down","lines":23,"source":"page_key"}
"#
        );
    }

    #[test]
    fn encodes_a_repeated_scroll_as_separate_commands_in_one_buffer() {
        // One write, so Herdr drains every step before it renders, but still
        // one command per step, so a mouse-aware app gets one report each.
        let wheel = TerminalScroll::new(
            TerminalScrollDirection::Up,
            3,
            TerminalScrollSource::Wheel,
            Some(1),
            Some(2),
        )
        .unwrap();
        let line = r#"{"type":"terminal.scroll","direction":"up","lines":3,"source":"wheel","column":1,"row":2}
"#;
        assert_eq!(
            encode_command(&TerminalCommand::Scroll(wheel.repeated(3).unwrap())).unwrap(),
            line.repeat(3).into_bytes()
        );
        assert_eq!(
            encode_command(&TerminalCommand::Scroll(wheel.repeated(1).unwrap())).unwrap(),
            line.as_bytes()
        );
        for repeat in [0, MAX_TERMINAL_SCROLL_REPEAT + 1] {
            assert!(matches!(
                wheel.repeated(repeat),
                Err(HerdrTerminalError::InvalidScroll)
            ));
        }
    }

    #[test]
    fn rejects_scrolls_outside_the_terminal_bounds() {
        for (lines, column, row) in [
            (0, None, None),
            (MAX_TERMINAL_ROWS + 1, None, None),
            (1, Some(MAX_TERMINAL_COLS), None),
            (1, None, Some(MAX_TERMINAL_ROWS)),
        ] {
            assert!(matches!(
                TerminalScroll::new(
                    TerminalScrollDirection::Up,
                    lines,
                    TerminalScrollSource::Wheel,
                    column,
                    row,
                ),
                Err(HerdrTerminalError::InvalidScroll)
            ));
        }
        assert!(
            TerminalScroll::new(
                TerminalScrollDirection::Down,
                MAX_TERMINAL_ROWS,
                TerminalScrollSource::PageKey,
                Some(MAX_TERMINAL_COLS - 1),
                Some(MAX_TERMINAL_ROWS - 1),
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_malformed_and_oversized_input() {
        assert!(matches!(
            TerminalDimensions::new(10, 24),
            Err(HerdrTerminalError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            encode_command(&TerminalCommand::Input(TerminalInput::BytesBase64(
                "not base64".to_owned()
            ))),
            Err(HerdrTerminalError::InvalidBase64Input)
        ));
        assert!(matches!(
            encode_command(&TerminalCommand::Input(TerminalInput::Text(
                "x".repeat(MAX_TERMINAL_INPUT_BYTES + 1)
            ))),
            Err(HerdrTerminalError::InputTooLarge)
        ));
        // The largest accepted input still fits one command line even when
        // every byte needs a six-byte JSON escape.
        let escaped = encode_command(&TerminalCommand::Input(TerminalInput::Text(
            "\0".repeat(MAX_TERMINAL_INPUT_BYTES),
        )))
        .unwrap();
        assert!(escaped.len() <= MAX_TERMINAL_COMMAND_LINE_BYTES);
    }

    #[test]
    fn carries_a_whole_bracketed_paste_in_one_command() {
        let paste = format!(
            "\u{1b}[200~{}\u{1b}[201~",
            "line of pasted text\r".repeat(10_000)
        );
        assert!(paste.len() > 64 * 1024);
        let line =
            encode_command(&TerminalCommand::Input(TerminalInput::Text(paste.clone()))).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(decoded["text"], paste);
    }

    #[tokio::test]
    async fn rejects_malformed_and_oversized_stdout_ndjson() {
        assert!(matches!(
            parse_event(b"{not-json}\n"),
            Err(HerdrTerminalError::MalformedEvent(_))
        ));
        assert!(matches!(
            parse_event(
                br#"{"bytes":"%%%=","encoding":"ansi","full":false,"height":24,"seq":1,"type":"terminal.frame","width":80}
"#
            ),
            Err(HerdrTerminalError::InvalidEvent(_))
        ));

        let mut line = Vec::new();
        let mut oversized = BufReader::new(&b"123456789\n"[..]);
        assert!(matches!(
            read_bounded_line(&mut oversized, &mut line, 8).await,
            Err(HerdrTerminalError::EventLineTooLarge)
        ));
        let mut unterminated = BufReader::new(&b"{}\n{}"[..]);
        assert!(
            read_bounded_line(&mut unterminated, &mut line, 8)
                .await
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            read_bounded_line(&mut unterminated, &mut line, 8).await,
            Err(HerdrTerminalError::UnterminatedEvent)
        ));
        let mut empty = BufReader::new(&b""[..]);
        assert!(
            read_bounded_line(&mut empty, &mut line, 8)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn resumes_a_partial_line_after_the_read_is_cancelled() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        writer
            .write_all(br#"{"type":"terminal.closed","#)
            .await
            .unwrap();
        tokio::select! {
            biased;
            read = read_bounded_line(&mut reader, &mut line, 1024) => {
                panic!("an incomplete line must not resolve: {read:?}");
            }
            () = sleep(Duration::from_millis(20)) => {}
        }
        writer
            .write_all(b"\"reason\":\"detached\"}\n")
            .await
            .unwrap();

        let resumed = read_bounded_line(&mut reader, &mut line, 1024)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parse_event(&resumed).unwrap(),
            TerminalEvent::Closed(TerminalClosed {
                reason: "detached".to_owned(),
            })
        );
        assert!(line.is_empty());
    }

    #[tokio::test]
    async fn next_event_survives_a_select_that_drops_it_mid_frame() {
        // Herdr's CLI writes one frame line in several write(2) calls. The
        // relay polls next_event() inside select! next to a 250 ms lease tick;
        // dropping the future between those writes used to discard the
        // frame's head and fail on its tail with MalformedEvent, which the
        // relay reported as `runtime_error`, freezing the browser terminal.
        let temp = tempfile::tempdir().unwrap();
        let binary = write_script(
            temp.path(),
            "split-frame-herdr",
            r#"printf '%s' '{"type":"terminal.frame","seq":2,"encoding":"ansi",'
sleep 0.3
printf '%s\n' '"width":80,"height":24,"full":false,"bytes":"dGFpbA=="}'
printf '%s\n' '{"type":"terminal.closed","reason":"detached"}'
"#,
        );
        let config = HerdrConfig {
            binary: binary.into_os_string(),
            ..HerdrConfig::default()
        };
        let request = OpenTerminalRequest::new(
            "alpha",
            "term_123",
            TerminalDimensions::new(80, 24).unwrap(),
        )
        .unwrap();
        let mut terminal = HerdrTerminal::spawn(&config, request).unwrap();
        let mut ticker = interval(Duration::from_millis(25));
        let mut ticks = 0_u32;
        let mut events = Vec::new();

        timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _ = ticker.tick() => ticks += 1,
                    event = terminal.next_event() => match event.unwrap() {
                        Some(event) => events.push(event),
                        None => break,
                    },
                }
            }
        })
        .await
        .expect("controller output should finish");

        assert!(ticks > 2, "the ticker must have interrupted the read");
        assert_eq!(
            events,
            [
                TerminalEvent::Frame(TerminalFrame {
                    bytes: "dGFpbA==".to_owned(),
                    encoding: TerminalEncoding::Ansi,
                    seq: 2,
                    width: 80,
                    height: 24,
                    full: false,
                }),
                TerminalEvent::Closed(TerminalClosed {
                    reason: "detached".to_owned(),
                }),
            ]
        );
    }

    async fn recorded_release_commands(commands: Vec<TerminalCommand>) -> String {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("stdin.ndjson");
        let binary = write_script(
            temp.path(),
            "recording-herdr",
            &format!(
                "cat > '{}'\nprintf '%s\\n' '{{\"type\":\"terminal.closed\",\"reason\":\"detached\"}}'\n",
                record.display()
            ),
        );
        let config = HerdrConfig {
            binary: binary.into_os_string(),
            ..HerdrConfig::default()
        };
        let request = OpenTerminalRequest::new(
            "alpha",
            "term_123",
            TerminalDimensions::new(80, 24).unwrap(),
        )
        .unwrap();
        let mut terminal = HerdrTerminal::spawn(&config, request).unwrap();
        for command in commands {
            terminal.send(command).await.unwrap();
        }
        terminal.close().await.unwrap();
        fs::read_to_string(record).unwrap()
    }

    #[tokio::test]
    async fn release_returns_a_scrolled_pane_to_the_latest_output() {
        // Herdr keeps one scroll offset per pane and resets it only on input,
        // so a Yard view closed while scrolled back used to leave the pane
        // (Herdr's own client and the next viewer) away from the latest output.
        let up = TerminalScroll::new(
            TerminalScrollDirection::Up,
            3,
            TerminalScrollSource::Wheel,
            None,
            None,
        )
        .unwrap();
        let scroll = r#"{"type":"terminal.scroll","direction":"up","lines":3,"source":"wheel"}"#;
        let reset = r#"{"type":"terminal.input","text":""}"#;
        let release = r#"{"type":"terminal.release"}"#;
        assert_eq!(
            recorded_release_commands(vec![TerminalCommand::Scroll(up)]).await,
            format!("{scroll}\n{reset}\n{release}\n")
        );
        // Input already reset Herdr's offset, so release sends nothing extra.
        assert_eq!(
            recorded_release_commands(vec![
                TerminalCommand::Scroll(up),
                TerminalCommand::Input(TerminalInput::Text("x".to_owned())),
            ])
            .await,
            format!("{scroll}\n{{\"type\":\"terminal.input\",\"text\":\"x\"}}\n{release}\n")
        );
        assert_eq!(
            recorded_release_commands(vec![
                TerminalCommand::Scroll(up),
                TerminalCommand::ScrollReset,
            ])
            .await,
            format!("{scroll}\n{reset}\n{release}\n")
        );
        assert_eq!(
            recorded_release_commands(Vec::new()).await,
            format!("{release}\n")
        );
    }

    #[test]
    fn builds_exact_controller_argv_without_takeover() {
        let config = HerdrConfig {
            binary: "herdr-test".into(),
            ..HerdrConfig::default()
        };
        let command = controller_command(
            &config,
            "session alpha",
            "term_123",
            TerminalDimensions::new(80, 24).unwrap(),
        );
        let command = command.as_std();
        assert_eq!(command.get_program(), "herdr-test");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "--session",
                "session alpha",
                "terminal",
                "session",
                "control",
                "term_123",
                "--cols",
                "80",
                "--rows",
                "24",
            ]
        );
        assert!(!args.iter().any(|arg| *arg == "--takeover"));
    }

    #[tokio::test]
    async fn reports_unexpected_exit_and_drains_bounded_stderr() {
        let temp = tempfile::tempdir().unwrap();
        let binary = write_script(
            temp.path(),
            "failing-herdr",
            r#"i=0
while [ "$i" -lt 2000 ]; do
    printf '%s\n' 'fatal terminal controller output xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' >&2
    i=$((i + 1))
done
exit 7
"#,
        );
        let config = HerdrConfig {
            binary: binary.into_os_string(),
            ..HerdrConfig::default()
        };
        let request = OpenTerminalRequest::new(
            "alpha",
            "term_123",
            TerminalDimensions::new(80, 24).unwrap(),
        )
        .unwrap();
        let mut terminal = HerdrTerminal::spawn(&config, request).unwrap();

        let error = timeout(Duration::from_secs(3), terminal.next_event())
            .await
            .unwrap()
            .unwrap_err();
        match error {
            HerdrTerminalError::UnexpectedExit {
                status,
                stderr,
                stderr_truncated,
            } => {
                assert_eq!(status.code(), Some(7));
                assert!(stderr.starts_with("fatal terminal controller output"));
                assert!(stderr_truncated);
            }
            other => panic!("expected unexpected exit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn drop_kills_and_reaps_controller() {
        let temp = tempfile::tempdir().unwrap();
        let binary = write_script(
            temp.path(),
            "sleeping-herdr",
            r#"printf '%s\n' "$$" > "$0.pid"
exec sleep 30
"#,
        );
        let config = HerdrConfig {
            binary: binary.clone().into_os_string(),
            ..HerdrConfig::default()
        };
        let request = OpenTerminalRequest::new(
            "alpha",
            "term_123",
            TerminalDimensions::new(80, 24).unwrap(),
        )
        .unwrap();
        let terminal = HerdrTerminal::spawn(&config, request).unwrap();
        let pid_path = PathBuf::from(format!("{}.pid", binary.display()));
        wait_for_file(&pid_path).await;
        let pid = fixture_pid(&pid_path);
        assert!(process_exists(pid));

        drop(terminal);

        timeout(Duration::from_secs(3), async {
            while process_exists(pid) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("dropped controller should be killed and reaped");
    }

    #[tokio::test]
    async fn close_kills_and_reaps_controller_that_ignores_release() {
        let temp = tempfile::tempdir().unwrap();
        let binary = write_script(
            temp.path(),
            "unresponsive-herdr",
            r#"printf '%s\n' "$$" > "$0.pid"
exec sleep 30
"#,
        );
        let config = HerdrConfig {
            binary: binary.clone().into_os_string(),
            ..HerdrConfig::default()
        };
        let request = OpenTerminalRequest::new(
            "alpha",
            "term_123",
            TerminalDimensions::new(80, 24).unwrap(),
        )
        .unwrap();
        let terminal = HerdrTerminal::spawn(&config, request).unwrap();
        let pid_path = PathBuf::from(format!("{}.pid", binary.display()));
        wait_for_file(&pid_path).await;
        let pid = fixture_pid(&pid_path);

        assert!(matches!(
            terminal.close().await,
            Err(HerdrTerminalError::CloseTimeout)
        ));
        assert!(!process_exists(pid), "closed controller should be reaped");
    }

    fn write_script(directory: &Path, name: &str, body: &str) -> PathBuf {
        let path = directory.join(name);
        let temporary = directory.join(format!(".{name}.tmp"));
        fs::write(&temporary, format!("#!/bin/sh\nset -eu\n{body}")).unwrap();
        let mut permissions = fs::metadata(&temporary).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&temporary, permissions).unwrap();
        fs::rename(temporary, &path).unwrap();
        path
    }

    async fn wait_for_file(path: &Path) {
        timeout(Duration::from_secs(2), async {
            while !path.exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("controller fixture should create its marker file");
    }

    fn fixture_pid(path: &Path) -> Pid {
        let raw = fs::read_to_string(path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        Pid::from_raw(raw).expect("controller fixture should report a positive process ID")
    }

    fn process_exists(pid: Pid) -> bool {
        match test_kill_process(pid) {
            Ok(()) | Err(Errno::PERM) => true,
            Err(Errno::SRCH) => false,
            Err(error) => panic!("failed to inspect controller process: {error}"),
        }
    }
}

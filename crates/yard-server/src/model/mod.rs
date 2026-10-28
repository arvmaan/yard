//! Tool-less model calls through `aws bedrock-runtime converse`: the CLI
//! runs from an argv (never a shell) with
//! `AWS_PAGER=""`, a timeout and `kill_on_drop`; the request travels in a
//! 0600 temp file deleted after the call; stdout is capped at 1 MiB; a request
//! carrying `toolConfig` is refused; every call first runs the account guard
//! (the profile must resolve to the configured account).
//!
//! Credentials: resolving the profile runs its `credential_process` once per
//! `aws` spawn, which made every call slow. The runner therefore asks the CLI
//! once (`aws configure export-credentials --format process`) and hands the
//! short-lived result to the following calls through their environment
//! (`--profile` is then omitted). The values live only in memory, are never
//! logged, printed or written to disk, and are dropped before they expire;
//! when the export fails, calls fall back to `--profile`.

pub(crate) mod price;

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};
use zeroize::Zeroizing;

/// Converse stdout cap.
pub(crate) const MAX_CONVERSE_STDOUT_BYTES: usize = 1024 * 1024;
/// stderr cap.
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// How long a matching `sts get-caller-identity` is trusted.
const ACCOUNT_GUARD_TTL: Duration = Duration::from_secs(15 * 60);
/// Exported credentials are reused at most this long (and never within
/// [`CREDENTIALS_MARGIN`] of their own expiry).
const CREDENTIALS_TTL: Duration = Duration::from_secs(10 * 60);
const CREDENTIALS_MARGIN: Duration = Duration::from_secs(2 * 60);
/// `export-credentials` output cap.
const MAX_CREDENTIALS_BYTES: usize = 16 * 1024;
const MAX_ERROR_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum ModelError {
    #[error("the aws CLI could not run: {0}")]
    Spawn(String),
    #[error("the model call timed out after {0} s")]
    Timeout(u64),
    #[error("the aws CLI printed more than {0} bytes")]
    TooLarge(usize),
    #[error("{code}: {message}")]
    Failed { code: String, message: String },
    #[error("unexpected model output: {0}")]
    BadOutput(String),
    #[error("the AWS profile does not resolve to the configured account")]
    AccountMismatch,
    #[error("{0}")]
    Invalid(String),
    #[error("temporary file: {0}")]
    TempFile(String),
}

/// One Converse call. The production runner is [`AwsConverse`]; tests use a
/// fake.
#[async_trait]
pub(crate) trait ConverseRunner: Send + Sync {
    async fn converse(&self, request: &Value) -> Result<Value, ModelError>;
}

/// The request of one tool-less call: one system block, one user message,
/// temperature 0.
pub(crate) fn request(model: &str, system: &str, user: &str, max_tokens: u64) -> Value {
    json!({
        "modelId": model,
        "system": [{"text": system}],
        "messages": [{"role": "user", "content": [{"text": user}]}],
        "inferenceConfig": {"maxTokens": max_tokens, "temperature": 0},
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stop {
    Complete,
    MaxTokens,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Reply {
    pub text: String,
    pub stop: Stop,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Read a Converse response: the text blocks, `stopReason` and `usage`.
pub(crate) fn parse(model: &str, response: &Value) -> Result<Reply, ModelError> {
    let bad = |what: &str| ModelError::BadOutput(format!("converse response has no {what}"));
    let stop = match response.get("stopReason").and_then(Value::as_str) {
        Some("end_turn" | "stop_sequence") => Stop::Complete,
        Some("max_tokens") => Stop::MaxTokens,
        Some(other) => Stop::Other(other.chars().take(64).collect()),
        None => return Err(bad("stopReason")),
    };
    let usage = response.get("usage").ok_or_else(|| bad("usage"))?;
    let tokens = |key: &str| {
        usage
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| bad(key))
    };
    let (input_tokens, output_tokens) = (tokens("inputTokens")?, tokens("outputTokens")?);
    let text = response
        .pointer("/output/message/content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    Ok(Reply {
        text,
        stop,
        input_tokens,
        output_tokens,
        cost_usd: price::cost_usd(model, input_tokens, output_tokens),
    })
}

/// Short-lived credentials exported from the profile. `Debug` never shows
/// the values.
#[derive(Clone)]
struct Credentials {
    access_key_id: Zeroizing<String>,
    secret_access_key: Zeroizing<String>,
    session_token: Option<Zeroizing<String>>,
    /// Reuse until this instant.
    usable_until: Instant,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credentials([redacted])")
    }
}

/// `credential_process` JSON as `export-credentials --format process`
/// prints it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExportedCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    expiration: Option<String>,
}

#[derive(Debug, Default)]
struct Session {
    credentials: Option<Credentials>,
    /// The export failed: use `--profile` per call until this instant.
    export_failed_until: Option<Instant>,
    verified_at: Option<Instant>,
}

/// `aws bedrock-runtime converse` under a named profile, guarded by the
/// expected account.
pub(crate) struct AwsConverse {
    binary: PathBuf,
    profile: String,
    region: String,
    account_id: String,
    temp_dir: PathBuf,
    timeout: Duration,
    /// Single flight: concurrent calls wait for one export and one account
    /// check instead of each running the credential process.
    session: tokio::sync::Mutex<Session>,
}

/// Seconds from now until an RFC 3339 `Expiration`, when it parses.
fn seconds_until(expiration: &str) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(expiration).ok()?;
    Some((at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds())
}

impl AwsConverse {
    pub(crate) fn new(
        binary: PathBuf,
        profile: String,
        region: String,
        account_id: String,
        temp_dir: PathBuf,
        timeout: Duration,
    ) -> Self {
        Self {
            binary,
            profile,
            region,
            account_id,
            temp_dir,
            timeout,
            session: tokio::sync::Mutex::new(Session::default()),
        }
    }

    /// One credential export, reused while fresh. `None` = use `--profile`.
    async fn credentials(&self, session: &mut Session) -> Option<Credentials> {
        let now = Instant::now();
        if let Some(credentials) = &session.credentials
            && credentials.usable_until > now
        {
            return Some(credentials.clone());
        }
        session.credentials = None;
        if session.export_failed_until.is_some_and(|until| until > now) {
            return None;
        }
        let exported = self
            .run(
                args(["configure", "export-credentials", "--format", "process"]),
                MAX_CREDENTIALS_BYTES,
                None,
            )
            .await
            .ok()
            .map(Zeroizing::new)
            .and_then(|stdout| serde_json::from_slice::<ExportedCredentials>(&stdout).ok())
            .filter(|exported| {
                !exported.access_key_id.is_empty() && !exported.secret_access_key.is_empty()
            });
        let Some(exported) = exported else {
            session.export_failed_until = Some(now + ACCOUNT_GUARD_TTL);
            return None;
        };
        let mut usable = CREDENTIALS_TTL;
        if let Some(left) = exported.expiration.as_deref().and_then(seconds_until) {
            let left = u64::try_from(left).unwrap_or(0);
            usable = usable.min(Duration::from_secs(left).saturating_sub(CREDENTIALS_MARGIN));
        }
        // Wrapped before any early return, so the values are wiped on drop.
        let credentials = Credentials {
            access_key_id: Zeroizing::new(exported.access_key_id),
            secret_access_key: Zeroizing::new(exported.secret_access_key),
            session_token: exported
                .session_token
                .filter(|token| !token.is_empty())
                .map(Zeroizing::new),
            usable_until: now + usable,
        };
        if usable.is_zero() {
            return None;
        }
        session.credentials = Some(credentials.clone());
        Some(credentials)
    }

    /// Credentials for one call (exported once, shared by concurrent calls)
    /// after the account guard passed for them.
    async fn prepare(&self) -> Result<Option<Credentials>, ModelError> {
        let mut session = self.session.lock().await;
        let credentials = self.credentials(&mut session).await;
        if session
            .verified_at
            .is_some_and(|at| at.elapsed() < ACCOUNT_GUARD_TTL)
        {
            return Ok(credentials);
        }
        let stdout = self
            .run(
                args([
                    "sts",
                    "get-caller-identity",
                    "--query",
                    "Account",
                    "--output",
                    "text",
                ]),
                4096,
                credentials.as_ref(),
            )
            .await?;
        let account = String::from_utf8_lossy(&stdout).trim().to_owned();
        if account.len() == 12
            && account.bytes().all(|byte| byte.is_ascii_digit())
            && account == self.account_id
        {
            session.verified_at = Some(Instant::now());
            Ok(credentials)
        } else {
            session.credentials = None;
            Err(ModelError::AccountMismatch)
        }
    }

    async fn run(
        &self,
        argv: Vec<OsString>,
        stdout_cap: usize,
        credentials: Option<&Credentials>,
    ) -> Result<Vec<u8>, ModelError> {
        let mut command = Command::new(&self.binary);
        command.args(argv).args(["--region", self.region.as_str()]);
        match credentials {
            // Environment credentials only win when no profile is named.
            Some(credentials) => {
                command
                    .env_remove("AWS_PROFILE")
                    .env_remove("AWS_DEFAULT_PROFILE")
                    .env("AWS_ACCESS_KEY_ID", credentials.access_key_id.as_str())
                    .env(
                        "AWS_SECRET_ACCESS_KEY",
                        credentials.secret_access_key.as_str(),
                    );
                match &credentials.session_token {
                    Some(token) => command.env("AWS_SESSION_TOKEN", token.as_str()),
                    None => command.env_remove("AWS_SESSION_TOKEN"),
                };
            }
            None => {
                command.args(["--profile", self.profile.as_str()]);
            }
        }
        command
            .env("AWS_PAGER", "")
            .env("AWS_CLI_AUTO_PROMPT", "off")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| ModelError::Spawn(error.kind().to_string()))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(ModelError::Spawn("pipes unavailable".to_owned()));
        };
        let operation = async move {
            let (stdout, stderr, status) = tokio::try_join!(
                read_bounded(stdout, stdout_cap),
                read_bounded(stderr, MAX_STDERR_BYTES),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|error| ModelError::Spawn(error.kind().to_string()))
                }
            )?;
            Ok::<_, ModelError>((status, stdout, stderr))
        };
        // On timeout the future, and with it the child, is dropped:
        // `kill_on_drop` kills it.
        let (status, stdout, stderr) = timeout(self.timeout, operation)
            .await
            .map_err(|_| ModelError::Timeout(self.timeout.as_secs()))??;
        if status.success() {
            Ok(stdout)
        } else {
            Err(failure(status.code(), &stderr))
        }
    }
}

#[async_trait]
impl ConverseRunner for AwsConverse {
    async fn converse(&self, request: &Value) -> Result<Value, ModelError> {
        if !request.is_object() || request.get("toolConfig").is_some() {
            return Err(ModelError::Invalid(
                "a Converse request is a JSON object without toolConfig".to_owned(),
            ));
        }
        let credentials = self.prepare().await?;
        let payload =
            serde_json::to_vec(request).map_err(|error| ModelError::Invalid(error.to_string()))?;
        let input = TempFile::create(&self.temp_dir, &payload)?;
        let mut file_arg = OsString::from("file://");
        file_arg.push(input.path.as_os_str());
        let mut argv = args(["bedrock-runtime", "converse", "--cli-input-json"]);
        argv.push(file_arg);
        let read_timeout = self.timeout.as_secs().max(1).to_string();
        argv.extend(args([
            "--cli-read-timeout",
            read_timeout.as_str(),
            "--output",
            "json",
        ]));
        let stdout = self
            .run(argv, MAX_CONVERSE_STDOUT_BYTES, credentials.as_ref())
            .await?;
        serde_json::from_slice(&stdout)
            .map_err(|_| ModelError::BadOutput("the aws CLI printed unreadable JSON".to_owned()))
    }
}

fn args<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, ModelError> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0_u8; 8192];
    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|error| ModelError::Spawn(error.kind().to_string()))?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len() + read > limit {
            return Err(ModelError::TooLarge(limit));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// The AWS error code and a short redacted message of a failed call.
fn failure(exit_code: Option<i32>, stderr: &[u8]) -> ModelError {
    let text = String::from_utf8_lossy(stderr);
    let fallback = exit_code.map_or_else(
        || "terminated_by_signal".to_owned(),
        |code| format!("exit_status_{code}"),
    );
    let aws = text.lines().find_map(|line| {
        let rest = line.split_once("An error occurred (")?.1;
        let (code, rest) = rest.split_once(')')?;
        let ok = !code.is_empty()
            && code.len() <= 64
            && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
        let message = rest.split_once(": ").map_or("", |(_, message)| message);
        ok.then(|| (code.to_owned(), message.trim().to_owned()))
    });
    let (code, message) = aws.unwrap_or_else(|| {
        (
            fallback,
            text.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("no error output")
                .to_owned(),
        )
    });
    ModelError::Failed {
        code,
        message: crate::redact::clean_text(&message, MAX_ERROR_CHARS),
    }
}

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn create(dir: &Path, contents: &[u8]) -> Result<Self, ModelError> {
        let path = dir.join(format!("yard-board-{}.json", uuid::Uuid::now_v7()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| ModelError::TempFile(error.kind().to_string()))?;
        let temp = Self { path };
        file.write_all(contents)
            .and_then(|()| file.sync_all())
            .map_err(|error| ModelError::TempFile(error.kind().to_string()))?;
        Ok(temp)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests;

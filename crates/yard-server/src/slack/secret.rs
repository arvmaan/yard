//! Read the bot token (and, for inbound Socket Mode, the app-level token)
//! from AWS Secrets Manager through the AWS CLI.
//!
//! Yard only reads the secret; the owner creates and rotates it. The CLI is
//! run from an argv (no shell), with a timeout, `kill_on_drop` and bounded
//! output, the same shape as Herdr session discovery. The token lives only in
//! memory ([`BotToken`] zeroizes on drop and never formats itself). Errors
//! never include stdout, and stderr is redacted and truncated before it can
//! reach a log line or the status API.

use std::{ffi::OsString, fmt, io, process::Stdio, time::Duration};

use serde::Deserialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};
use zeroize::Zeroizing;

use super::message::redact;
use crate::config::SlackSettings;

pub const SECRET_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SECRET_STDOUT_BYTES: usize = 64 * 1024;
const MAX_SECRET_STDERR_BYTES: usize = 64 * 1024;
const MAX_ERROR_DETAIL_CHARS: usize = 200;

/// A Slack bot token held only in memory.
#[derive(Clone)]
pub struct BotToken(Zeroizing<String>);

impl BotToken {
    /// Validate a candidate token: `xoxb-` prefix, printable ASCII, bounded.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::NotABotToken`] otherwise.
    pub fn new(value: Zeroizing<String>) -> Result<Self, SecretError> {
        let valid = value.starts_with("xoxb-")
            && (10..=512).contains(&value.len())
            && value.bytes().all(|byte| byte.is_ascii_graphic());
        if valid {
            Ok(Self(value))
        } else {
            Err(SecretError::NotABotToken)
        }
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BotToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BotToken(<redacted>)")
    }
}

/// A Slack app-level token (`xapp-…`, scope `connections:write`) held only
/// in memory. It opens Socket Mode connections and nothing else.
#[derive(Clone)]
pub struct AppToken(Zeroizing<String>);

impl AppToken {
    /// Validate a candidate token: `xapp-` prefix, printable ASCII, bounded.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::NotAnAppToken`] otherwise.
    pub fn new(value: Zeroizing<String>) -> Result<Self, SecretError> {
        let valid = value.starts_with("xapp-")
            && (10..=512).contains(&value.len())
            && value.bytes().all(|byte| byte.is_ascii_graphic());
        if valid {
            Ok(Self(value))
        } else {
            Err(SecretError::NotAnAppToken)
        }
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AppToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AppToken(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("could not run the AWS CLI ({0}); install AWS CLI v2 or check PATH")]
    Spawn(String),
    #[error("the AWS CLI did not answer within {0} seconds")]
    Timeout(u64),
    #[error("the AWS CLI output exceeded {0} bytes")]
    TooLarge(usize),
    #[error("aws secretsmanager get-secret-value failed ({status}): {detail}")]
    Failed { status: String, detail: String },
    #[error("the secret must be a Slack bot token (xoxb-…) or JSON {{\"bot_token\": \"xoxb-…\"}}")]
    NotABotToken,
    #[error(
        "the app secret must be a Slack app-level token (xapp-…) or JSON {{\"app_token\": \"xapp-…\"}}"
    )]
    NotAnAppToken,
}

impl SecretError {
    /// Whether the owner must change configuration or the secret (as opposed
    /// to a transient failure worth retrying soon).
    #[must_use]
    pub const fn is_misconfiguration(&self) -> bool {
        matches!(self, Self::NotABotToken | Self::NotAnAppToken)
    }
}

/// How to reach the secret.
#[derive(Debug, Clone)]
pub struct SecretSource {
    pub aws_binary: OsString,
    pub secret_id: String,
    pub profile: Option<String>,
    pub region: String,
    pub timeout: Duration,
}

impl SecretSource {
    #[must_use]
    pub fn from_settings(settings: &SlackSettings) -> Self {
        Self {
            aws_binary: OsString::from("aws"),
            secret_id: settings.secret_id.clone(),
            profile: settings.aws_profile.clone(),
            region: settings.aws_region.clone(),
            timeout: SECRET_FETCH_TIMEOUT,
        }
    }

    /// The same AWS profile and region, pointed at another secret (the
    /// app-level token for Socket Mode).
    #[must_use]
    pub fn for_secret(&self, secret_id: &str) -> Self {
        Self {
            secret_id: secret_id.to_owned(),
            ..self.clone()
        }
    }

    fn args(&self) -> Vec<&str> {
        let mut args = vec![
            "secretsmanager",
            "get-secret-value",
            "--secret-id",
            self.secret_id.as_str(),
            "--query",
            "SecretString",
            "--output",
            "text",
            "--region",
            self.region.as_str(),
        ];
        if let Some(profile) = &self.profile {
            args.extend(["--profile", profile.as_str()]);
        }
        args
    }

    /// Run the AWS CLI once and parse the bot token.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError`] when the CLI cannot run, times out, fails, or
    /// prints something that is not a bot token.
    pub async fn fetch(&self) -> Result<BotToken, SecretError> {
        parse_secret(&self.fetch_secret_string().await?)
    }

    /// Run the AWS CLI once and parse an app-level token.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError`] when the CLI cannot run, times out, fails, or
    /// prints something that is not an app-level token.
    pub async fn fetch_app_token(&self) -> Result<AppToken, SecretError> {
        parse_app_secret(&self.fetch_secret_string().await?)
    }

    /// # Panics
    ///
    /// Never: stdout and stderr are always piped before spawning.
    async fn fetch_secret_string(&self) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        let mut command = Command::new(&self.aws_binary);
        command
            .args(self.args())
            .env("AWS_PAGER", "")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| SecretError::Spawn(error.kind().to_string()))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let operation = async move {
            let (stdout, stderr, status) = tokio::try_join!(
                read_bounded(stdout, MAX_SECRET_STDOUT_BYTES),
                read_bounded(stderr, MAX_SECRET_STDERR_BYTES),
                async { child.wait().await.map_err(ReadError::Io) }
            )?;
            Ok::<_, ReadError>((status, stdout, stderr))
        };
        let (status, stdout, stderr) = timeout(self.timeout, operation)
            .await
            .map_err(|_| SecretError::Timeout(self.timeout.as_secs()))?
            .map_err(|error| match error {
                ReadError::Io(error) => SecretError::Spawn(error.kind().to_string()),
                ReadError::TooLarge(limit) => SecretError::TooLarge(limit),
            })?;
        if !status.success() {
            return Err(SecretError::Failed {
                status: status.code().map_or_else(
                    || "terminated by a signal".to_owned(),
                    |code| format!("exit status {code}"),
                ),
                detail: error_detail(&stderr),
            });
        }
        Ok(stdout)
    }
}

/// Accept a plain token or JSON `{"bot_token": "xoxb-…"}`.
fn parse_secret(stdout: &[u8]) -> Result<BotToken, SecretError> {
    let text = std::str::from_utf8(stdout).map_err(|_| SecretError::NotABotToken)?;
    let text = text.trim();
    if text.starts_with('{') {
        #[derive(Deserialize)]
        struct SecretJson {
            bot_token: String,
        }
        let secret: SecretJson =
            serde_json::from_str(text).map_err(|_| SecretError::NotABotToken)?;
        return BotToken::new(Zeroizing::new(secret.bot_token));
    }
    BotToken::new(Zeroizing::new(text.to_owned()))
}

/// Accept a plain app-level token or JSON `{"app_token": "xapp-…"}`.
fn parse_app_secret(stdout: &[u8]) -> Result<AppToken, SecretError> {
    let text = std::str::from_utf8(stdout).map_err(|_| SecretError::NotAnAppToken)?;
    let text = text.trim();
    if text.starts_with('{') {
        #[derive(Deserialize)]
        struct SecretJson {
            app_token: String,
        }
        let secret: SecretJson =
            serde_json::from_str(text).map_err(|_| SecretError::NotAnAppToken)?;
        return AppToken::new(Zeroizing::new(secret.app_token));
    }
    AppToken::new(Zeroizing::new(text.to_owned()))
}

/// The first stderr line, redacted and truncated.
fn error_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no error output");
    let line = redact(line);
    if line.chars().count() > MAX_ERROR_DETAIL_CHARS {
        let mut kept = line
            .chars()
            .take(MAX_ERROR_DETAIL_CHARS)
            .collect::<String>();
        kept.push('…');
        kept
    } else {
        line
    }
}

enum ReadError {
    Io(io::Error),
    TooLarge(usize),
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, ReadError> {
    // One fixed allocation filled in place: `read_to_end` would grow the
    // buffer by reallocating and free earlier copies of the token unzeroized.
    let mut bytes = Zeroizing::new(vec![0_u8; limit.saturating_add(1)]);
    let mut filled = 0;
    loop {
        let read = reader
            .read(&mut bytes[filled..])
            .await
            .map_err(ReadError::Io)?;
        if read == 0 {
            break;
        }
        filled += read;
        if filled > limit {
            return Err(ReadError::TooLarge(limit));
        }
    }
    // Shrinks the length only; the capacity (zeroized on drop) is kept.
    bytes.truncate(filled);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

    use tempfile::TempDir;

    use super::{SecretError, SecretSource, parse_app_secret, parse_secret};

    const TOKEN: &str = concat!("xo", "xb-111-222-TESTTOKENVALUE");

    /// A fake `aws` executable that records its argv and runs `body`.
    pub(crate) fn fake_aws(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("aws");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n{body}\n",
                dir.join("argv").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn source(aws: &Path, profile: Option<&str>) -> SecretSource {
        SecretSource {
            aws_binary: aws.into(),
            secret_id: "yard/slack-bot".to_owned(),
            profile: profile.map(str::to_owned),
            region: "us-west-2".to_owned(),
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn reads_json_secret_with_exact_argv() {
        let temp = TempDir::new().unwrap();
        let aws = fake_aws(
            temp.path(),
            &format!("printf '%s\\n' '{{\"bot_token\":\"{TOKEN}\"}}'"),
        );
        let token = source(&aws, Some("yard-dev")).fetch().await.unwrap();
        assert_eq!(token.expose(), TOKEN);
        assert_eq!(format!("{token:?}"), "BotToken(<redacted>)");
        let argv = std::fs::read_to_string(temp.path().join("argv")).unwrap();
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            [
                "secretsmanager",
                "get-secret-value",
                "--secret-id",
                "yard/slack-bot",
                "--query",
                "SecretString",
                "--output",
                "text",
                "--region",
                "us-west-2",
                "--profile",
                "yard-dev",
            ]
        );
    }

    #[tokio::test]
    async fn reads_plain_secret_without_profile() {
        let temp = TempDir::new().unwrap();
        let aws = fake_aws(temp.path(), &format!("printf '%s\\n' '{TOKEN}'"));
        let token = source(&aws, None).fetch().await.unwrap();
        assert_eq!(token.expose(), TOKEN);
        let argv = std::fs::read_to_string(temp.path().join("argv")).unwrap();
        assert!(!argv.contains("--profile"));
    }

    #[tokio::test]
    async fn non_zero_exit_reports_redacted_stderr_only() {
        let temp = TempDir::new().unwrap();
        let aws = fake_aws(
            temp.path(),
            &format!(
                "printf '%s\\n' '{TOKEN}'\nprintf '%s\\n' 'An error occurred (AccessDeniedException) leaked {TOKEN}' >&2\nexit 254"
            ),
        );
        let error = source(&aws, None).fetch().await.unwrap_err();
        let SecretError::Failed { status, detail } = &error else {
            panic!("unexpected {error:?}");
        };
        assert_eq!(status, "exit status 254");
        assert!(detail.contains("AccessDeniedException"), "{detail}");
        assert!(!error.to_string().contains("TESTTOKENVALUE"), "{error}");
    }

    #[tokio::test]
    async fn slow_cli_times_out_and_is_killed() {
        let temp = TempDir::new().unwrap();
        let aws = fake_aws(temp.path(), "sleep 30");
        let mut source = source(&aws, None);
        source.timeout = Duration::from_millis(200);
        let started = std::time::Instant::now();
        assert_eq!(source.fetch().await.unwrap_err(), SecretError::Timeout(0));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn missing_cli_is_a_spawn_error() {
        let temp = TempDir::new().unwrap();
        let error = source(&temp.path().join("missing-aws"), None)
            .fetch()
            .await
            .unwrap_err();
        assert!(matches!(error, SecretError::Spawn(_)), "{error:?}");
    }

    #[test]
    fn rejects_secrets_that_are_not_bot_tokens() {
        for secret in [
            "",
            concat!("xo", "xp-user-token-value"),
            concat!("{\"token\":\"xo", "xb-1-2-3456789\"}"),
            "{\"bot_token\": 5}",
            concat!("xo", "xb-has space"),
            "{not json",
        ] {
            assert_eq!(
                parse_secret(secret.as_bytes()).unwrap_err(),
                SecretError::NotABotToken,
                "{secret}"
            );
        }
        assert!(SecretError::NotABotToken.is_misconfiguration());
    }

    #[tokio::test]
    async fn reads_the_app_token_from_its_own_secret() {
        let app = concat!("xa", "pp-1-A111-222-APPTOKENVALUE");
        let temp = TempDir::new().unwrap();
        let aws = fake_aws(
            temp.path(),
            &format!("printf '%s\\n' '{{\"app_token\":\"{app}\"}}'"),
        );
        let bot = source(&aws, Some("yard-dev"));
        let token = bot
            .for_secret("yard/slack-app")
            .fetch_app_token()
            .await
            .unwrap();
        assert_eq!(token.expose(), app);
        assert_eq!(format!("{token:?}"), "AppToken(<redacted>)");
        let argv = std::fs::read_to_string(temp.path().join("argv")).unwrap();
        let argv = argv.lines().collect::<Vec<_>>();
        assert_eq!(argv[3], "yard/slack-app");
        assert_eq!(argv[argv.len() - 1], "yard-dev");
        // A bot token in the app secret (or the reverse) is rejected.
        assert_eq!(
            bot.fetch().await.unwrap_err(),
            SecretError::NotABotToken,
            "an app token is not a bot token"
        );
    }

    #[test]
    fn rejects_app_secrets_that_are_not_app_tokens() {
        for secret in [
            "",
            concat!("xo", "xb-1-2-3456789"),
            concat!("{\"bot_token\":\"xa", "pp-1-2-3456789\"}"),
            concat!("xa", "pp-has space"),
            "{\"app_token\": 5}",
        ] {
            assert_eq!(
                parse_app_secret(secret.as_bytes()).unwrap_err(),
                SecretError::NotAnAppToken,
                "{secret}"
            );
        }
        assert!(parse_app_secret(concat!("xa", "pp-1-A1-2-PLAINVALUE\n").as_bytes()).is_ok());
        assert!(SecretError::NotAnAppToken.is_misconfiguration());
    }
}

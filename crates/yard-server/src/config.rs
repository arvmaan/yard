use std::{
    env,
    ffi::OsString,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub herdr_binary: OsString,
    pub database_path: PathBuf,
    pub artifact_path: PathBuf,
    pub orchestrator_cwd: PathBuf,
    pub coordination_path: PathBuf,
    pub knowledge_path: PathBuf,
    pub storage: StorageConfig,
    pub slack: SlackConfig,
}

/// Default for `YARD_STORAGE_LOG_RETENTION_DAYS`.
pub const DEFAULT_STORAGE_LOG_RETENTION_DAYS: u32 = 14;
const MAX_STORAGE_LOG_RETENTION_DAYS: u32 = 3650;

/// Storage scan settings (preview only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// Canonical scan roots. `None` means `YARD_STORAGE_ROOTS` is unset and
    /// nothing is scanned (fail closed).
    pub roots: Option<Vec<PathBuf>>,
    /// Build logs newer than this many days keep a candidate in review.
    pub log_retention_days: u32,
    /// File names that mark a workspace root next to a `src/` directory
    /// (`YARD_STORAGE_WORKSPACE_MARKERS`). Empty disables the workspace
    /// classes.
    pub workspace_markers: Vec<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            roots: None,
            log_retention_days: DEFAULT_STORAGE_LOG_RETENTION_DAYS,
            workspace_markers: Vec::new(),
        }
    }
}

impl StorageConfig {
    /// Parse `YARD_STORAGE_ROOTS`, `YARD_STORAGE_LOG_RETENTION_DAYS`, and
    /// `YARD_STORAGE_WORKSPACE_MARKERS`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a root is relative, missing, not a
    /// directory, itself a symlink, or the filesystem root, when the
    /// retention is not a whole number of days between 1 and 3650, or when a
    /// workspace marker is not a plain file name.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self::parse(
            env::var_os("YARD_STORAGE_ROOTS").as_deref(),
            env::var_os("YARD_STORAGE_LOG_RETENTION_DAYS").as_deref(),
        )?;
        config.workspace_markers = Self::parse_workspace_markers(
            env::var_os("YARD_STORAGE_WORKSPACE_MARKERS").as_deref(),
        )?;
        Ok(config)
    }

    /// Parse comma-separated workspace marker file names. Unset or empty
    /// means none (workspace classes disabled); blank entries are ignored and
    /// duplicates collapse.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidStorageWorkspaceMarker`] when an entry
    /// is not valid UTF-8 or not a plain file name.
    pub fn parse_workspace_markers(
        value: Option<&std::ffi::OsStr>,
    ) -> Result<Vec<String>, ConfigError> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let text = value.to_str().ok_or_else(|| {
            ConfigError::InvalidStorageWorkspaceMarker(value.to_string_lossy().into_owned())
        })?;
        let mut markers: Vec<String> = Vec::new();
        for entry in text
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            if entry == "." || entry == ".." || entry.contains(['/', '\\', '\0']) {
                return Err(ConfigError::InvalidStorageWorkspaceMarker(entry.to_owned()));
            }
            if !markers.iter().any(|marker| marker == entry) {
                markers.push(entry.to_owned());
            }
        }
        Ok(markers)
    }

    /// Parse raw roots and retention; `from_env` passes the environment.
    /// Workspace markers come from [`StorageConfig::parse_workspace_markers`].
    ///
    /// # Errors
    ///
    /// See [`StorageConfig::from_env`].
    pub fn parse(
        roots: Option<&std::ffi::OsStr>,
        retention: Option<&std::ffi::OsStr>,
    ) -> Result<Self, ConfigError> {
        let log_retention_days = match retention {
            None => DEFAULT_STORAGE_LOG_RETENTION_DAYS,
            Some(value) => value
                .to_str()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|days| (1..=MAX_STORAGE_LOG_RETENTION_DAYS).contains(days))
                .ok_or_else(|| {
                    ConfigError::InvalidStorageLogRetention(value.to_string_lossy().into_owned())
                })?,
        };
        let roots = match roots {
            None => None,
            Some(value) if value.is_empty() => None,
            Some(value) => Some(parse_storage_roots(value)?),
        };
        Ok(Self {
            roots,
            log_retention_days,
            workspace_markers: Vec::new(),
        })
    }
}

fn parse_storage_roots(value: &std::ffi::OsStr) -> Result<Vec<PathBuf>, ConfigError> {
    use std::os::unix::ffi::OsStrExt;

    let mut roots = Vec::new();
    for entry in value.as_bytes().split(|byte| *byte == b':') {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(entry));
        roots.push(canonical_storage_root(&path)?);
    }
    roots.sort();
    roots.dedup();
    // A root inside another root would be scanned twice.
    let outer = roots.clone();
    roots.retain(|root| {
        !outer
            .iter()
            .any(|other| other != root && root.starts_with(other))
    });
    Ok(roots)
}

/// Canonicalize one storage root. The root itself must not be a symlink;
/// symlinked ancestors such as `/home -> /local/home` are allowed.
fn canonical_storage_root(path: &Path) -> Result<PathBuf, ConfigError> {
    let invalid = |reason: &str| ConfigError::InvalidStorageRoot {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    };
    if path.as_os_str().is_empty() {
        return Err(invalid("entries must not be empty"));
    }
    if !path.is_absolute() {
        return Err(invalid("entries must be absolute paths"));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(invalid("entries must not contain `..`"));
    }
    // Rebuild from components so a trailing `/` or `/.` cannot make
    // `symlink_metadata` follow a symlinked final component.
    let path: PathBuf = path.components().collect();
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|source| invalid(&format!("the path is not accessible: {source}")))?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("a root must not itself be a symlink"));
    }
    if !metadata.is_dir() {
        return Err(invalid("a root must be a directory"));
    }
    let canonical = std::fs::canonicalize(&path)
        .map_err(|source| invalid(&format!("the path could not be resolved: {source}")))?;
    if canonical.parent().is_none() {
        return Err(invalid("the filesystem root is not allowed"));
    }
    Ok(canonical)
}

/// Default for `YARD_SLACK_AWS_REGION`.
pub const DEFAULT_SLACK_AWS_REGION: &str = "us-west-2";

/// Slack DM notifications (phase 1) and optional inbound Socket Mode
/// (phase 2, [`SlackInbound`]).
///
/// Parsing never fails: with `YARD_SLACK_NOTIFICATIONS=on` and a missing or
/// invalid setting, Yard still starts and the notifier reports
/// `misconfigured` with the reason. The bot token is never configuration;
/// only the Secrets Manager secret that holds it is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SlackConfig {
    /// `YARD_SLACK_NOTIFICATIONS` is unset, empty or `off`.
    #[default]
    Off,
    Enabled(SlackSettings),
    /// Notifications were requested but a setting is missing or invalid.
    Misconfigured(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackSettings {
    /// Secrets Manager secret id or ARN (a pointer, not a credential).
    pub secret_id: String,
    /// `None` uses the default AWS credential chain.
    pub aws_profile: Option<String>,
    pub aws_region: String,
    /// Slack member id (`U…` or `W…`) of the one person Yard DMs.
    pub owner_user_id: String,
    /// When set, `auth.test` must report this enterprise (fail closed).
    pub enterprise_id: Option<String>,
    /// Inbound Socket Mode (phase 2); off unless explicitly enabled.
    pub inbound: SlackInbound,
}

/// Inbound Slack over Socket Mode (phase 2).
///
/// Runs only when notifications are on, `YARD_SLACK_INBOUND=on` and
/// `YARD_SLACK_APP_SECRET_ID` names the secret holding the app-level
/// (`xapp-…`) token. A problem here never disables outbound notifications.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SlackInbound {
    /// `YARD_SLACK_INBOUND` is unset, empty or `off`.
    #[default]
    Off,
    Enabled {
        /// Secrets Manager secret id or ARN holding the app-level token.
        app_secret_id: String,
    },
    /// Inbound was requested but a setting is missing or invalid.
    Misconfigured(String),
}

impl SlackInbound {
    fn parse(values: &SlackEnv) -> Self {
        let mode = values
            .inbound
            .as_deref()
            .map(|value| value.to_str().map(str::trim));
        match mode {
            None | Some(Some("" | "off")) => return Self::Off,
            Some(Some("on")) => {}
            Some(_) => {
                return Self::Misconfigured("YARD_SLACK_INBOUND must be `on` or `off`".to_owned());
            }
        }
        match optional_setting(
            "YARD_SLACK_APP_SECRET_ID",
            values.app_secret_id.as_deref(),
            2048,
            |byte| byte.is_ascii_alphanumeric() || b"/_+=.@-:".contains(&byte),
            "a Secrets Manager secret name or ARN",
        ) {
            Ok(Some(app_secret_id)) => Self::Enabled { app_secret_id },
            Ok(None) => Self::Misconfigured(
                "YARD_SLACK_APP_SECRET_ID is required when YARD_SLACK_INBOUND=on".to_owned(),
            ),
            Err(reason) => Self::Misconfigured(reason),
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }
}

impl SlackConfig {
    /// Read the `YARD_SLACK_*` variables.
    #[must_use]
    pub fn from_env() -> Self {
        let read = |name: &str| env::var_os(name);
        Self::parse(&SlackEnv {
            notifications: read("YARD_SLACK_NOTIFICATIONS"),
            secret_id: read("YARD_SLACK_SECRET_ID"),
            aws_profile: read("YARD_SLACK_AWS_PROFILE"),
            aws_region: read("YARD_SLACK_AWS_REGION"),
            owner_user_id: read("YARD_SLACK_OWNER_USER_ID"),
            enterprise_id: read("YARD_SLACK_ENTERPRISE_ID"),
            inbound: read("YARD_SLACK_INBOUND"),
            app_secret_id: read("YARD_SLACK_APP_SECRET_ID"),
        })
    }

    /// Parse raw values; `from_env` passes the environment.
    #[must_use]
    pub fn parse(values: &SlackEnv) -> Self {
        let mode = values
            .notifications
            .as_deref()
            .map(|value| value.to_str().map(str::trim));
        match mode {
            None | Some(Some("" | "off")) => return Self::Off,
            Some(Some("on")) => {}
            Some(_) => {
                return Self::Misconfigured(
                    "YARD_SLACK_NOTIFICATIONS must be `on` or `off`".to_owned(),
                );
            }
        }
        match SlackSettings::parse(values) {
            Ok(settings) => Self::Enabled(settings),
            Err(reason) => Self::Misconfigured(reason),
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Raw `YARD_SLACK_*` values.
#[derive(Debug, Clone, Default)]
pub struct SlackEnv {
    pub notifications: Option<OsString>,
    pub secret_id: Option<OsString>,
    pub aws_profile: Option<OsString>,
    pub aws_region: Option<OsString>,
    pub owner_user_id: Option<OsString>,
    pub enterprise_id: Option<OsString>,
    pub inbound: Option<OsString>,
    pub app_secret_id: Option<OsString>,
}

impl SlackSettings {
    fn parse(values: &SlackEnv) -> Result<Self, String> {
        let secret_id = required_setting(
            "YARD_SLACK_SECRET_ID",
            values.secret_id.as_deref(),
            2048,
            |byte| byte.is_ascii_alphanumeric() || b"/_+=.@-:".contains(&byte),
            "a Secrets Manager secret name or ARN",
        )?;
        let aws_profile = optional_setting(
            "YARD_SLACK_AWS_PROFILE",
            values.aws_profile.as_deref(),
            128,
            |byte| byte.is_ascii_alphanumeric() || b"_.+@-".contains(&byte),
            "an AWS CLI profile name",
        )?;
        let aws_region = optional_setting(
            "YARD_SLACK_AWS_REGION",
            values.aws_region.as_deref(),
            32,
            |byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-',
            "an AWS region such as us-west-2",
        )?
        .unwrap_or_else(|| DEFAULT_SLACK_AWS_REGION.to_owned());
        let owner_user_id = required_setting(
            "YARD_SLACK_OWNER_USER_ID",
            values.owner_user_id.as_deref(),
            32,
            |byte| byte.is_ascii_uppercase() || byte.is_ascii_digit(),
            "a Slack member id such as U01ABCDEF",
        )?;
        if !(owner_user_id.starts_with('U') || owner_user_id.starts_with('W'))
            || owner_user_id.len() < 3
        {
            return Err(
                "YARD_SLACK_OWNER_USER_ID must be a Slack member id such as U01ABCDEF".to_owned(),
            );
        }
        let enterprise_id = optional_setting(
            "YARD_SLACK_ENTERPRISE_ID",
            values.enterprise_id.as_deref(),
            32,
            |byte| byte.is_ascii_uppercase() || byte.is_ascii_digit(),
            "a Slack enterprise id such as E01ABCDEF",
        )?;
        if enterprise_id
            .as_deref()
            .is_some_and(|id| !id.starts_with('E') || id.len() < 3)
        {
            return Err(
                "YARD_SLACK_ENTERPRISE_ID must be a Slack enterprise id such as E01ABCDEF"
                    .to_owned(),
            );
        }
        Ok(Self {
            secret_id,
            aws_profile,
            aws_region,
            owner_user_id,
            enterprise_id,
            inbound: SlackInbound::parse(values),
        })
    }
}

fn required_setting(
    name: &str,
    value: Option<&std::ffi::OsStr>,
    max_len: usize,
    allowed: impl Fn(u8) -> bool,
    expected: &str,
) -> Result<String, String> {
    optional_setting(name, value, max_len, allowed, expected)?
        .ok_or_else(|| format!("{name} is required when YARD_SLACK_NOTIFICATIONS=on"))
}

/// A value is passed to the AWS CLI as its own argument, so it must not look
/// like an option and may only use the listed bytes.
fn optional_setting(
    name: &str,
    value: Option<&std::ffi::OsStr>,
    max_len: usize,
    allowed: impl Fn(u8) -> bool,
    expected: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(value) = value.to_str().map(str::trim) else {
        return Err(format!("{name} must be {expected}"));
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > max_len || value.starts_with('-') || !value.bytes().all(allowed) {
        return Err(format!("{name} must be {expected}"));
    }
    Ok(Some(value.to_owned()))
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("YARD_BIND must be a valid socket address: {0}")]
    InvalidBind(#[source] std::net::AddrParseError),
    #[error("YARD_BIND must use a loopback address until network authentication exists")]
    NonLoopbackBind,
    #[error("YARD_ORCHESTRATOR_CWD '{path}' is not an accessible local directory: {source}")]
    InvalidOrchestratorCwd {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("YARD_DATABASE_PATH '{path}' could not be normalized: {source}")]
    InvalidDatabasePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("YARD_STORAGE_ROOTS entry '{path}' is invalid: {reason}")]
    InvalidStorageRoot { path: PathBuf, reason: String },
    #[error(
        "YARD_STORAGE_LOG_RETENTION_DAYS must be a whole number of days from 1 to 3650, got '{0}'"
    )]
    InvalidStorageLogRetention(String),
    #[error("YARD_STORAGE_WORKSPACE_MARKERS entry '{0}' must be a plain file name")]
    InvalidStorageWorkspaceMarker(String),
}

impl ServerConfig {
    /// Load local server settings from environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when `YARD_BIND` is present but is not a socket
    /// address.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind = env::var("YARD_BIND").map_or_else(
            |_| Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4317)),
            |value| value.parse().map_err(ConfigError::InvalidBind),
        )?;
        if !bind.ip().is_loopback() {
            return Err(ConfigError::NonLoopbackBind);
        }
        let herdr_binary = env::var_os("YARD_HERDR_BIN").unwrap_or_else(|| OsString::from("herdr"));
        let database_path = database_path_from_env()?;
        yard_store::validate_database_file_identity(&database_path).map_err(|source| {
            ConfigError::InvalidDatabasePath {
                path: database_path.clone(),
                source,
            }
        })?;
        let artifact_path = env::var_os("YARD_ARTIFACT_PATH")
            .map_or_else(|| default_artifact_path(&database_path), PathBuf::from);
        let coordination_path = managed_path_from_env(
            "YARD_COORDINATION_PATH",
            default_coordination_path(&database_path),
        )?;
        let knowledge_path = managed_path_from_env(
            "YARD_KNOWLEDGE_PATH",
            default_knowledge_path(&database_path),
        )?;
        let orchestrator_cwd = env::var_os("YARD_ORCHESTRATOR_CWD")
            .map_or_else(env::current_dir, |path| {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    Ok(path)
                } else {
                    env::current_dir().map(|cwd| cwd.join(path))
                }
            })
            .and_then(std::fs::canonicalize)
            .map_err(|source| ConfigError::InvalidOrchestratorCwd {
                path: env::var_os("YARD_ORCHESTRATOR_CWD")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from),
                source,
            })?;
        let storage = StorageConfig::from_env()?;
        let slack = SlackConfig::from_env();

        Ok(Self {
            bind,
            herdr_binary,
            database_path,
            artifact_path,
            orchestrator_cwd,
            coordination_path,
            knowledge_path,
            storage,
            slack,
        })
    }
}

fn managed_path_from_env(name: &str, default: PathBuf) -> Result<PathBuf, ConfigError> {
    let path = env::var_os(name).map_or(default, PathBuf::from);
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|source| ConfigError::InvalidOrchestratorCwd {
                path: PathBuf::from("."),
                source,
            })
    }
}

fn default_artifact_path(database_path: &Path) -> PathBuf {
    database_path.parent().map_or_else(
        || PathBuf::from(".yard/artifacts"),
        |parent| parent.join("artifacts"),
    )
}

fn default_coordination_path(database_path: &Path) -> PathBuf {
    database_path.parent().map_or_else(
        || PathBuf::from(".yard/coordination"),
        |parent| parent.join("coordination"),
    )
}

fn default_knowledge_path(database_path: &Path) -> PathBuf {
    database_path.parent().map_or_else(
        || PathBuf::from(".yard/knowledge"),
        |parent| parent.join("knowledge"),
    )
}

/// Resolve and normalize the configured database path without loading unrelated settings.
///
/// # Errors
///
/// Returns [`ConfigError`] when the current directory or an existing path
/// ancestor cannot be resolved.
pub fn database_path_from_env() -> Result<PathBuf, ConfigError> {
    let path = env::var_os("YARD_DATABASE_PATH").map_or_else(default_database_path, PathBuf::from);
    yard_store::normalize_database_path(&path)
        .map_err(|source| ConfigError::InvalidDatabasePath { path, source })
}

fn default_database_path() -> PathBuf {
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("yard/yard.sqlite3");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/yard/yard.sqlite3");
    }
    PathBuf::from(".yard/yard.sqlite3")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use tempfile::TempDir;

    use super::{
        ConfigError, DEFAULT_SLACK_AWS_REGION, DEFAULT_STORAGE_LOG_RETENTION_DAYS, SlackConfig,
        SlackEnv, SlackInbound, SlackSettings, StorageConfig,
    };

    fn slack_env(pairs: &[(&str, &str)]) -> SlackEnv {
        let mut env = SlackEnv::default();
        for (name, value) in pairs {
            let value = Some(std::ffi::OsString::from(value));
            match *name {
                "notifications" => env.notifications = value,
                "secret" => env.secret_id = value,
                "profile" => env.aws_profile = value,
                "region" => env.aws_region = value,
                "owner" => env.owner_user_id = value,
                "enterprise" => env.enterprise_id = value,
                "inbound" => env.inbound = value,
                "app_secret" => env.app_secret_id = value,
                other => panic!("unknown slack setting {other}"),
            }
        }
        env
    }

    #[test]
    fn slack_notifications_are_off_unless_explicitly_on() {
        for pairs in [
            &[][..],
            &[("notifications", "")][..],
            &[("notifications", "off")][..],
            &[("notifications", " off "), ("secret", "yard/slack-bot")][..],
        ] {
            assert_eq!(SlackConfig::parse(&slack_env(pairs)), SlackConfig::Off);
        }
    }

    #[test]
    fn slack_on_parses_settings_with_defaults() {
        let config = SlackConfig::parse(&slack_env(&[
            ("notifications", "on"),
            ("secret", "yard/slack-bot"),
            ("owner", "U01ABCDEF"),
        ]));
        assert_eq!(
            config,
            SlackConfig::Enabled(SlackSettings {
                secret_id: "yard/slack-bot".to_owned(),
                aws_profile: None,
                aws_region: DEFAULT_SLACK_AWS_REGION.to_owned(),
                owner_user_id: "U01ABCDEF".to_owned(),
                enterprise_id: None,
                inbound: SlackInbound::Off,
            })
        );
        let config = SlackConfig::parse(&slack_env(&[
            ("notifications", "on"),
            (
                "secret",
                "arn:aws:secretsmanager:us-west-2:123456789012:secret:yard/slack-bot-AbCdEf",
            ),
            ("profile", "yard-dev"),
            ("region", "us-east-1"),
            ("owner", "W012AB"),
            ("enterprise", "E01SANDBOX0"),
        ]));
        let SlackConfig::Enabled(settings) = config else {
            panic!("expected enabled: {config:?}");
        };
        assert_eq!(settings.aws_profile.as_deref(), Some("yard-dev"));
        assert_eq!(settings.aws_region, "us-east-1");
        assert_eq!(settings.enterprise_id.as_deref(), Some("E01SANDBOX0"));
    }

    #[test]
    fn slack_on_with_missing_or_invalid_settings_is_misconfigured_not_fatal() {
        for (pairs, needle) in [
            (&[("notifications", "yes")][..], "YARD_SLACK_NOTIFICATIONS"),
            (
                &[("notifications", "on"), ("owner", "U01ABCDEF")][..],
                "YARD_SLACK_SECRET_ID",
            ),
            (
                &[("notifications", "on"), ("secret", "yard/slack-bot")][..],
                "YARD_SLACK_OWNER_USER_ID",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "--endpoint-url=http://x"),
                    ("owner", "U01"),
                ][..],
                "YARD_SLACK_SECRET_ID",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "yard slack"),
                    ("owner", "U01"),
                ][..],
                "YARD_SLACK_SECRET_ID",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "yard/slack-bot"),
                    ("owner", "@sample"),
                ][..],
                "YARD_SLACK_OWNER_USER_ID",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "yard/slack-bot"),
                    ("owner", "C0123"),
                ][..],
                "YARD_SLACK_OWNER_USER_ID",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "yard/slack-bot"),
                    ("owner", "U01"),
                    ("profile", "-x"),
                ][..],
                "YARD_SLACK_AWS_PROFILE",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "yard/slack-bot"),
                    ("owner", "U01"),
                    ("region", "US_WEST"),
                ][..],
                "YARD_SLACK_AWS_REGION",
            ),
            (
                &[
                    ("notifications", "on"),
                    ("secret", "yard/slack-bot"),
                    ("owner", "U01"),
                    ("enterprise", "T0123"),
                ][..],
                "YARD_SLACK_ENTERPRISE_ID",
            ),
        ] {
            let config = SlackConfig::parse(&slack_env(pairs));
            let SlackConfig::Misconfigured(reason) = &config else {
                panic!("{pairs:?}: expected misconfigured, got {config:?}");
            };
            assert!(reason.contains(needle), "{pairs:?}: {reason}");
            assert!(config.enabled());
        }
    }

    #[test]
    fn slack_inbound_needs_its_own_switch_and_app_secret() {
        let base = [
            ("notifications", "on"),
            ("secret", "yard/slack-bot"),
            ("owner", "U01ABCDEF"),
        ];
        let inbound = |extra: &[(&str, &str)]| {
            let pairs = base.iter().chain(extra).copied().collect::<Vec<_>>();
            let SlackConfig::Enabled(settings) = SlackConfig::parse(&slack_env(&pairs)) else {
                panic!("notifications must stay enabled for {extra:?}");
            };
            settings.inbound
        };
        // Default off, and an app secret alone does not turn it on.
        assert_eq!(inbound(&[]), SlackInbound::Off);
        assert_eq!(
            inbound(&[("app_secret", "yard/slack-app")]),
            SlackInbound::Off
        );
        assert_eq!(
            inbound(&[("inbound", " off "), ("app_secret", "yard/slack-app")]),
            SlackInbound::Off
        );
        assert_eq!(
            inbound(&[("inbound", "on"), ("app_secret", "yard/slack-app")]),
            SlackInbound::Enabled {
                app_secret_id: "yard/slack-app".to_owned()
            }
        );
        for (extra, needle) in [
            (
                &[("inbound", "on")][..],
                "YARD_SLACK_APP_SECRET_ID is required",
            ),
            (
                &[("inbound", "on"), ("app_secret", "")][..],
                "YARD_SLACK_APP_SECRET_ID is required",
            ),
            (
                &[("inbound", "on"), ("app_secret", "--profile=x")][..],
                "YARD_SLACK_APP_SECRET_ID",
            ),
            (
                &[("inbound", "yes"), ("app_secret", "yard/slack-app")][..],
                "YARD_SLACK_INBOUND",
            ),
        ] {
            let SlackInbound::Misconfigured(reason) = inbound(extra) else {
                panic!("{extra:?}: expected misconfigured inbound");
            };
            assert!(reason.contains(needle), "{extra:?}: {reason}");
        }
        // Inbound never turns notifications on by itself.
        assert_eq!(
            SlackConfig::parse(&slack_env(&[
                ("inbound", "on"),
                ("app_secret", "yard/slack-app")
            ])),
            SlackConfig::Off
        );
        assert!(!SlackInbound::Misconfigured(String::new()).enabled());
    }

    #[test]
    fn unset_or_empty_storage_roots_are_not_configured() {
        let unset = StorageConfig::parse(None, None).unwrap();
        assert_eq!(unset.roots, None);
        assert_eq!(unset.log_retention_days, DEFAULT_STORAGE_LOG_RETENTION_DAYS);
        assert_eq!(
            StorageConfig::parse(Some(OsStr::new("")), None)
                .unwrap()
                .roots,
            None
        );
        assert!(unset.workspace_markers.is_empty());
    }

    #[test]
    fn workspace_markers_are_plain_file_names_and_default_to_none() {
        assert!(
            StorageConfig::parse_workspace_markers(None)
                .unwrap()
                .is_empty()
        );
        assert!(
            StorageConfig::parse_workspace_markers(Some(OsStr::new(" , ")))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            StorageConfig::parse_workspace_markers(Some(OsStr::new(
                "workspace.toml, .ws-root,workspace.toml"
            )))
            .unwrap(),
            ["workspace.toml", ".ws-root"]
        );
        for value in ["a/b", "..", ".", "/abs"] {
            assert!(
                matches!(
                    StorageConfig::parse_workspace_markers(Some(OsStr::new(value))),
                    Err(ConfigError::InvalidStorageWorkspaceMarker(_))
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn storage_roots_are_canonical_deduplicated_and_allow_symlinked_ancestors() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir_all(real.join("workspaces/nested")).unwrap();
        std::fs::create_dir_all(real.join("yard")).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let canonical = std::fs::canonicalize(&real).unwrap();
        let value = format!(
            "{}:{}:{}:{}",
            alias.join("workspaces").display(),
            real.join("yard").display(),
            real.join("workspaces/nested").display(),
            real.join("workspaces").display(),
        );

        let config =
            StorageConfig::parse(Some(OsStr::new(&value)), Some(OsStr::new("30"))).unwrap();

        assert_eq!(
            config.roots,
            Some(vec![canonical.join("workspaces"), canonical.join("yard")])
        );
        assert_eq!(config.log_retention_days, 30);
    }

    #[test]
    fn rejects_symlinked_relative_missing_and_file_roots_and_bad_retention() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let file = temp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        for value in [
            link.display().to_string(),
            format!("{}/", link.display()),
            format!("{}/.", link.display()),
            format!("{}//./", link.display()),
            "relative/path".to_owned(),
            temp.path().join("missing").display().to_string(),
            file.display().to_string(),
            format!("{}::{}", real.display(), real.display()),
            format!("{}/../real", real.display()),
            "/".to_owned(),
        ] {
            let error = StorageConfig::parse(Some(OsStr::new(&value)), None).unwrap_err();
            assert!(
                matches!(error, ConfigError::InvalidStorageRoot { .. }),
                "{value}: {error}"
            );
        }
        for retention in ["0", "3651", "two", "-1", "1.5"] {
            let error = StorageConfig::parse(None, Some(OsStr::new(retention))).unwrap_err();
            assert!(
                matches!(error, ConfigError::InvalidStorageLogRetention(_)),
                "{retention}"
            );
        }
    }
}

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

        Ok(Self {
            bind,
            herdr_binary,
            database_path,
            artifact_path,
            orchestrator_cwd,
            coordination_path,
            knowledge_path,
            storage,
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

    use super::{ConfigError, DEFAULT_STORAGE_LOG_RETENTION_DAYS, StorageConfig};

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

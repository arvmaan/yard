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
        let database_path =
            env::var_os("YARD_DATABASE_PATH").map_or_else(default_database_path, PathBuf::from);
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

        Ok(Self {
            bind,
            herdr_binary,
            database_path,
            artifact_path,
            orchestrator_cwd,
            coordination_path,
            knowledge_path,
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

fn default_database_path() -> PathBuf {
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("yard/yard.sqlite3");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/yard/yard.sqlite3");
    }
    PathBuf::from(".yard/yard.sqlite3")
}

use std::{path::Path, process::Stdio, time::Duration};

use tokio::{
    process::{Child, Command},
    time::{Instant, sleep},
};

use crate::{HerdrConfig, HerdrError, discovery::discover_sessions};

const SESSION_START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn ensure_session(
    config: &HerdrConfig,
    session_name: &str,
    startup_cwd: &Path,
) -> Result<(), HerdrError> {
    if session_is_running(config, session_name).await? {
        return Ok(());
    }

    let mut child = session_server_command(config, session_name, startup_cwd)
        .spawn()
        .map_err(HerdrError::SessionStartIo)?;
    let deadline = Instant::now() + SESSION_START_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(HerdrError::SessionStartIo)? {
            return Err(HerdrError::SessionStartFailed { status });
        }
        if session_is_running(config, session_name).await? {
            reap_session_server(child, session_name.to_owned());
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(HerdrError::SessionStartTimeout(session_name.to_owned()));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn session_is_running(config: &HerdrConfig, session_name: &str) -> Result<bool, HerdrError> {
    Ok(discover_sessions(config)
        .await?
        .into_iter()
        .any(|session| session.name == session_name && session.running))
}

fn session_server_command(config: &HerdrConfig, session_name: &str, startup_cwd: &Path) -> Command {
    let mut command = Command::new(&config.binary);
    command
        .arg("--session")
        .arg(session_name)
        .arg("server")
        .env("HERDR_STARTUP_CWD", startup_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn reap_session_server(mut child: Child, session_name: String) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) if !status.success() => {
                tracing::warn!(session = %session_name, %status, "Herdr session server exited");
            }
            Err(error) => {
                tracing::warn!(
                    session = %session_name,
                    %error,
                    "failed to reap Herdr session server"
                );
            }
            Ok(_) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path};

    use crate::HerdrConfig;

    use super::session_server_command;

    #[test]
    fn builds_named_persistent_session_command() {
        let config = HerdrConfig {
            binary: "herdr-test".into(),
            ..HerdrConfig::default()
        };
        let command = session_server_command(&config, "yard-orchestrator", Path::new("/srv/yard"));
        let command = command.as_std();

        assert_eq!(command.get_program(), "herdr-test");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--session", "yard-orchestrator", "server"]
        );
        assert!(command.get_envs().any(|(key, value)| {
            key == OsStr::new("HERDR_STARTUP_CWD") && value == Some(OsStr::new("/srv/yard"))
        }));
    }
}

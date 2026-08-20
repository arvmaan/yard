use std::{future::IntoFuture, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{sync::watch, task::JoinHandle, time::timeout};
use tracing_subscriber::EnvFilter;
use yard_herdr::{HerdrAdapter, HerdrConfig};
use yard_server::{
    app_with_reconciliation_and_paths_and_automation_and_shutdown,
    config::{ConfigError, ServerConfig},
    inventory_service::HerdrInventorySource,
    reconciliation_service::ReconciliationService,
    runtime_cleanup_service::RuntimeCleanupService,
};
use yard_store::{ProjectStoreError, SqliteProjectStore};

use crate::lifecycle::{self, LifecycleError};

const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECTION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Error)]
pub(crate) enum ServerError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error("server I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl ServerError {
    pub(crate) const fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) => 2,
            Self::Lifecycle(error) => error.exit_code(),
            Self::Store(_) | Self::Io(_) => 1,
        }
    }
}

enum RunMode<'a> {
    Foreground,
    Managed { instance_id: &'a str },
}

enum ServerEvent {
    Http(std::io::Result<()>),
    Shutdown,
    Control(Result<(), tokio::task::JoinError>),
}

pub(crate) fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("yard_server=info")),
        )
        .try_init();
}

pub(crate) async fn run_foreground() -> Result<(), ServerError> {
    run(RunMode::Foreground).await
}

pub(crate) async fn run_managed(instance_id: &str) -> Result<(), ServerError> {
    lifecycle::parse_instance_id(instance_id)?;
    run(RunMode::Managed { instance_id }).await
}

#[allow(clippy::too_many_lines)]
async fn run(mode: RunMode<'_>) -> Result<(), ServerError> {
    let config = ServerConfig::from_env()?;
    let lifecycle_mode = match mode {
        RunMode::Foreground => lifecycle::InstanceMode::Foreground,
        RunMode::Managed { .. } => lifecycle::InstanceMode::Managed,
    };
    let mut runtime_claim = lifecycle::claim_runtime(&config.database_path, lifecycle_mode).await?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let address = listener.local_addr()?;
    let ui_url = format!("http://{address}/");

    let adapter = HerdrAdapter::new(HerdrConfig {
        binary: config.herdr_binary.clone(),
        ..HerdrConfig::default()
    });
    let runtime = Arc::new(HerdrInventorySource::new(adapter));
    let store = Arc::new(SqliteProjectStore::open(&config.database_path).await?);
    let reconciliation = ReconciliationService::new(runtime.clone(), store.clone());
    let cleanup = RuntimeCleanupService::new(runtime.clone(), store.clone());
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (app, automations, connections) =
        app_with_reconciliation_and_paths_and_automation_and_shutdown(
            runtime.clone(),
            runtime.clone(),
            runtime.clone(),
            runtime,
            store,
            config.artifact_path.clone(),
            reconciliation.clone(),
            config.orchestrator_cwd.clone(),
            config.coordination_path.clone(),
            config.knowledge_path.clone(),
            shutdown_receiver.clone(),
        );

    let instance_id = match mode {
        RunMode::Foreground => lifecycle::new_instance_id(),
        RunMode::Managed { instance_id } => instance_id.to_owned(),
    };
    let metadata = lifecycle::InstanceMetadata::new(
        instance_id.clone(),
        &config,
        lifecycle_mode,
        address,
        ui_url.clone(),
        chrono::Utc::now().to_rfc3339(),
    )?;
    let control = lifecycle::bind_control(runtime_claim.paths())?;
    lifecycle::write_metadata(runtime_claim.paths(), &metadata)?;
    let owner_guard = lifecycle::RuntimeOwnerGuard::new(runtime_claim.paths().clone(), instance_id);
    let mut control_task = lifecycle::spawn_control(control, metadata, shutdown.clone());
    runtime_claim.published();

    let reconciliation_task = tokio::spawn(reconciliation.run());
    let cleanup_task = tokio::spawn(cleanup.run());
    let automation_task = tokio::spawn(automations.run());

    tracing::info!(
        target: "yard_server",
        address = %address,
        ui_url = %ui_url,
        database = %config.database_path.display(),
        artifacts = %config.artifact_path.display(),
        orchestrator_cwd = %config.orchestrator_cwd.display(),
        coordination = %config.coordination_path.display(),
        knowledge = %config.knowledge_path.display(),
        mode = %lifecycle_mode,
        "Yard UI available"
    );

    let signal_task = spawn_signal_handler(shutdown.clone());
    let (result, control_finished) = {
        let mut shutdown_observer = shutdown_receiver.clone();
        let mut server_shutdown = shutdown_receiver;
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = server_shutdown.wait_for(|requested| *requested).await;
            })
            .into_future();
        tokio::pin!(server);

        let event = tokio::select! {
            result = &mut server => ServerEvent::Http(result),
            _ = shutdown_observer.wait_for(|requested| *requested) => ServerEvent::Shutdown,
            result = &mut control_task => ServerEvent::Control(result),
        };
        let (http_result, control_finished) = match event {
            ServerEvent::Http(result) => (Some(result), false),
            ServerEvent::Shutdown => (None, false),
            ServerEvent::Control(result) => {
                match result {
                    Ok(()) => tracing::error!(
                        target: "yard_server",
                        "lifecycle control listener stopped unexpectedly"
                    ),
                    Err(error) => tracing::error!(
                        target: "yard_server",
                        %error,
                        "lifecycle control listener task failed"
                    ),
                }
                let _ = shutdown.send(true);
                (None, true)
            }
        };
        if http_result.is_none() {
            connections.close();
        }
        let result = if let Some(result) = http_result {
            result.map_err(ServerError::Io)
        } else if let Ok(result) = timeout(GRACEFUL_SHUTDOWN_TIMEOUT, &mut server).await {
            result.map_err(ServerError::Io)
        } else {
            tracing::warn!(
                target: "yard_server",
                timeout_seconds = GRACEFUL_SHUTDOWN_TIMEOUT.as_secs(),
                "graceful shutdown timed out; closing remaining connections"
            );
            Ok(())
        };
        (result, control_finished)
    };

    let _ = shutdown.send(true);
    connections.close();
    signal_task.abort();
    reconciliation_task.abort();
    cleanup_task.abort();
    automation_task.abort();
    let _ = reconciliation_task.await;
    let _ = cleanup_task.await;
    let _ = automation_task.await;
    if !control_finished {
        control_task.abort();
        let _ = control_task.await;
    }
    if timeout(CONNECTION_SHUTDOWN_TIMEOUT, connections.wait_for_idle())
        .await
        .is_err()
    {
        tracing::warn!(
            target: "yard_server",
            timeout_seconds = CONNECTION_SHUTDOWN_TIMEOUT.as_secs(),
            "upgraded connection shutdown timed out; forcing runtime teardown"
        );
    }
    drop(owner_guard);
    runtime_claim.retain_instance_lock_until_process_exit();
    result
}

fn spawn_signal_handler(shutdown: watch::Sender<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        wait_for_os_signal().await;
        let _ = shutdown.send(true);
    })
}

async fn wait_for_os_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(target: "yard_server", %error, "could not install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

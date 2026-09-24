use std::{
    fs::File,
    future::IntoFuture,
    io::{self, Seek, SeekFrom, Write},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use thiserror::Error;
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};
use yard_herdr::{HerdrAdapter, HerdrConfig};
use yard_server::{
    allocation_service::RuntimeControl,
    app_with_reconciliation_and_paths_and_automation_and_shutdown,
    config::{ConfigError, ServerConfig},
    intervention_service::RuntimeIntervention,
    inventory_service::{HerdrInventorySource, InventorySource},
    orchestrator_replacement_service::OrchestratorReplacementService,
    pane_management_service::PaneManagementService,
    reconciliation_service::ReconciliationService,
    runtime_cleanup_service::RuntimeCleanupService,
    terminal_service::RuntimeTerminal,
};
use yard_store::{ProjectStoreError, SqliteProjectStore, YardStore};

use crate::lifecycle::{self, LifecycleError};

const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECTION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_MANAGED_LOG_BYTES: u64 = 8 * 1024 * 1024;

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
    #[error("lifecycle control failed: {0}")]
    Control(String),
    #[error("background service failed: {0}")]
    Background(String),
}

impl ServerError {
    pub(crate) const fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) => 2,
            Self::Lifecycle(error) => error.exit_code(),
            Self::Store(_) | Self::Io(_) | Self::Control(_) | Self::Background(_) => 1,
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
    Background(Option<Result<&'static str, tokio::task::JoinError>>),
}

pub(crate) fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(tracing_filter())
        .try_init();
}

fn init_managed_tracing(file: File) {
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(tracing_filter())
        .with_writer(BoundedLogWriter::new(file, MAX_MANAGED_LOG_BYTES))
        .try_init();
}

fn tracing_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("yard_server=info"))
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
    let runtime_claim = lifecycle::claim_runtime(&config.database_path, lifecycle_mode).await?;
    if lifecycle_mode == lifecycle::InstanceMode::Managed {
        init_managed_tracing(runtime_claim.paths().open_log()?);
    }
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let address = listener.local_addr()?;
    let ui_url = format!("http://{address}/");

    let adapter = HerdrAdapter::new(HerdrConfig {
        binary: config.herdr_binary.clone(),
        ..HerdrConfig::default()
    });
    let runtime = Arc::new(HerdrInventorySource::new(adapter));
    let store = Arc::new(SqliteProjectStore::open(&config.database_path).await?);
    run_with_services(
        mode,
        lifecycle_mode,
        config,
        runtime_claim,
        listener,
        address,
        ui_url,
        runtime,
        store,
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_with_services<R>(
    mode: RunMode<'_>,
    lifecycle_mode: lifecycle::InstanceMode,
    config: ServerConfig,
    mut runtime_claim: lifecycle::RuntimeClaim,
    listener: tokio::net::TcpListener,
    address: std::net::SocketAddr,
    ui_url: String,
    runtime: Arc<R>,
    store: Arc<SqliteProjectStore>,
) -> Result<(), ServerError>
where
    R: InventorySource + RuntimeControl + RuntimeIntervention + RuntimeTerminal + 'static,
{
    let source: Arc<dyn InventorySource> = runtime.clone();
    let control: Arc<dyn RuntimeControl> = runtime.clone();
    let intervention: Arc<dyn RuntimeIntervention> = runtime.clone();
    let terminal: Arc<dyn RuntimeTerminal> = runtime;
    let store: Arc<dyn YardStore> = store;
    let replacement_recovery =
        OrchestratorReplacementService::new(source.clone(), control.clone(), store.clone());
    match replacement_recovery
        .reconcile_ambiguous_replacements()
        .await
    {
        Ok(report)
            if report.captures > 0 || report.prepare_intents > 0 || report.deferred_items > 0 =>
        {
            tracing::info!(
                target: "yard_server",
                captures = report.captures,
                prepare_intents = report.prepare_intents,
                adopted_commands = report.adopted_commands,
                absent_captures = report.absent_captures,
                conflicting_captures = report.conflicting_captures,
                quarantined_captures = report.quarantined_captures,
                deferred_items = report.deferred_items,
                "Reconciled ambiguous orchestrator replacement runtimes before startup"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                target: "yard_server",
                %error,
                "Orchestrator replacement startup reconciliation failed; durable captures remain reserved for inspection"
            );
        }
    }
    let reconciliation = ReconciliationService::new(source.clone(), store.clone());
    let cleanup = RuntimeCleanupService::new(control.clone(), store.clone());
    let pane_management = PaneManagementService::new(source.clone(), store.clone());
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (app, automations, connections) =
        app_with_reconciliation_and_paths_and_automation_and_shutdown(
            source,
            control,
            intervention,
            terminal,
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

    let mut background_tasks = JoinSet::new();
    background_tasks.spawn(async move {
        reconciliation.run().await;
        "reconciliation"
    });
    background_tasks.spawn(async move {
        cleanup.run().await;
        "runtime cleanup"
    });
    background_tasks.spawn(async move {
        pane_management.run().await;
        "pane management renewal"
    });
    background_tasks.spawn(async move {
        replacement_recovery.run().await;
        "orchestrator replacement recovery"
    });
    background_tasks.spawn(async move {
        automations.run().await;
        "automation scheduler"
    });

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
            result = background_tasks.join_next() => ServerEvent::Background(result),
        };
        let (http_result, control_finished, event_failure) = match event {
            ServerEvent::Http(result) => (Some(result), false, None),
            ServerEvent::Shutdown => (None, false, None),
            ServerEvent::Control(result) => {
                let message = match result {
                    Ok(()) => "listener stopped unexpectedly".to_owned(),
                    Err(error) => format!("listener task failed: {error}"),
                };
                tracing::error!(target: "yard_server", error = %message, "lifecycle control failed");
                (None, true, Some(ServerError::Control(message)))
            }
            ServerEvent::Background(result) => {
                let message = match result {
                    Some(Ok(name)) => format!("{name} stopped unexpectedly"),
                    Some(Err(error)) => format!("task failed: {error}"),
                    None => "supervisor became empty".to_owned(),
                };
                tracing::error!(target: "yard_server", error = %message, "background service failed");
                (None, false, Some(ServerError::Background(message)))
            }
        };
        let _ = shutdown.send(true);
        connections.close();
        abort_background_tasks(&mut background_tasks).await;
        let server_result = if let Some(result) = http_result {
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
        let result = match (server_result, event_failure) {
            (Err(error), _) | (Ok(()), Some(error)) => Err(error),
            (Ok(()), None) => Ok(()),
        };
        (result, control_finished)
    };

    let _ = shutdown.send(true);
    connections.close();
    signal_task.abort();
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

async fn abort_background_tasks(tasks: &mut JoinSet<&'static str>) {
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

#[derive(Clone)]
struct BoundedLogWriter {
    file: Arc<Mutex<File>>,
    maximum: u64,
}

impl BoundedLogWriter {
    fn new(file: File, maximum: u64) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
            maximum,
        }
    }
}

impl<'writer> MakeWriter<'writer> for BoundedLogWriter {
    type Writer = BoundedLogGuard<'writer>;

    fn make_writer(&'writer self) -> Self::Writer {
        BoundedLogGuard {
            file: self
                .file
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            maximum: self.maximum,
        }
    }
}

struct BoundedLogGuard<'writer> {
    file: MutexGuard<'writer, File>,
    maximum: u64,
}

impl Write for BoundedLogGuard<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let maximum = usize::try_from(self.maximum).unwrap_or(usize::MAX);
        let retained = if buffer.len() > maximum {
            &buffer[buffer.len() - maximum..]
        } else {
            buffer
        };
        let length = self.file.metadata()?.len();
        if length.saturating_add(retained.len() as u64) > self.maximum {
            self.file.set_len(0)?;
            self.file.seek(SeekFrom::Start(0))?;
        }
        if retained.len() == buffer.len() {
            self.file.write(retained)
        } else {
            self.file.write_all(retained)?;
            Ok(buffer.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        future::pending,
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use tempfile::{TempDir, tempfile};
    use tokio::{task::JoinSet, time::timeout};
    use yard_domain::{
        CanvasPlacement, CreateProject, FocusObservation, ObservedStatus, ObservedWorker,
        ProjectRuntimeBinding, RuntimeInventory, RuntimeObservationState, RuntimeProcessState,
        RuntimeSession, RuntimeSessions, WorkerRuntimeBinding,
    };
    use yard_server::{
        allocation_service::{RuntimeControl, RuntimeProvisionError, RuntimeProvisionRequest},
        config::ServerConfig,
        intervention_service::{
            RuntimeIntervention, RuntimeInterventionError, RuntimeOutputRequest,
            RuntimeOutputResult, RuntimePromptRequest, RuntimePromptResult,
        },
        inventory_service::{InventoryServiceError, InventorySource},
        terminal_service::{
            OpenTerminalRequest, RuntimeTerminal, RuntimeTerminalError, RuntimeTerminalSession,
        },
    };
    use yard_store::{SqliteProjectStore, YardStore};

    use super::{BoundedLogWriter, RunMode, abort_background_tasks, run_with_services};
    use crate::lifecycle::{self, InstanceMode};

    const EXPLICIT_TERMINAL: &str = "terminal-explicit";
    const UNKNOWN_TERMINAL: &str = "terminal-unknown";

    struct LifecycleRuntime {
        inventory_calls: AtomicUsize,
    }

    impl LifecycleRuntime {
        fn observed_worker(terminal_id: &str, name: &str, sequence: u64) -> ObservedWorker {
            ObservedWorker {
                runtime_id: terminal_id.to_owned(),
                pane_instance_id: None,
                terminal_id: terminal_id.to_owned(),
                workspace_id: "workspace-supervised".to_owned(),
                tab_id: format!("tab-{terminal_id}"),
                pane_id: format!("pane-{terminal_id}"),
                name: Some(name.to_owned()),
                provider: Some("codex".to_owned()),
                display_provider: Some("Codex".to_owned()),
                status: ObservedStatus::Idle,
                focused: false,
                launch_pending: false,
                interactive_ready: true,
                state_change_sequence: sequence,
                cwd: Some("/tmp/supervised".to_owned()),
                foreground_cwd: Some("/tmp/supervised".to_owned()),
                tokens: BTreeMap::new(),
                provider_session: None,
                revision: sequence,
            }
        }
    }

    #[async_trait]
    impl InventorySource for LifecycleRuntime {
        async fn sessions(&self) -> Result<RuntimeSessions, InventoryServiceError> {
            Ok(RuntimeSessions {
                adapter: "herdr".to_owned(),
                sessions: vec![RuntimeSession {
                    name: "default".to_owned(),
                    is_default: true,
                    running: true,
                }],
            })
        }

        async fn inventory(
            &self,
            session_name: &str,
        ) -> Result<RuntimeInventory, InventoryServiceError> {
            let sequence =
                u64::try_from(self.inventory_calls.fetch_add(1, Ordering::SeqCst) + 1).unwrap();
            Ok(RuntimeInventory {
                adapter: "herdr".to_owned(),
                session: session_name.to_owned(),
                runtime_version: "test".to_owned(),
                protocol: 19,
                observed_at_unix_ms: 100 + sequence,
                focus: FocusObservation::default(),
                workspaces: Vec::new(),
                tabs: Vec::new(),
                panes: Vec::new(),
                workers: vec![
                    Self::observed_worker(EXPLICIT_TERMINAL, "Explicit worker", sequence),
                    Self::observed_worker(UNKNOWN_TERMINAL, "Unknown agent", sequence),
                ],
                child_agents: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl RuntimeControl for LifecycleRuntime {
        async fn provision_worker(
            &self,
            _request: RuntimeProvisionRequest,
        ) -> Result<WorkerRuntimeBinding, RuntimeProvisionError> {
            Err(RuntimeProvisionError::BeforeWorker(
                "the supervised lifecycle test does not provision workers".to_owned(),
            ))
        }
    }

    #[async_trait]
    impl RuntimeIntervention for LifecycleRuntime {
        async fn prompt(
            &self,
            _request: RuntimePromptRequest,
        ) -> Result<RuntimePromptResult, RuntimeInterventionError> {
            Err(RuntimeInterventionError::Unavailable(
                "the supervised lifecycle test does not send prompts".to_owned(),
            ))
        }

        async fn read_output(
            &self,
            _request: RuntimeOutputRequest,
        ) -> Result<RuntimeOutputResult, RuntimeInterventionError> {
            Err(RuntimeInterventionError::Unavailable(
                "the supervised lifecycle test does not read output".to_owned(),
            ))
        }
    }

    #[async_trait]
    impl RuntimeTerminal for LifecycleRuntime {
        async fn open_terminal(
            &self,
            _request: OpenTerminalRequest,
        ) -> Result<Box<dyn RuntimeTerminalSession>, RuntimeTerminalError> {
            Err(RuntimeTerminalError::Unavailable(
                "the supervised lifecycle test does not open terminals".to_owned(),
            ))
        }
    }

    fn explicit_runtime_binding() -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-supervised".to_owned(),
            terminal_id: EXPLICIT_TERMINAL.to_owned(),
            tab_id: Some(format!("tab-{EXPLICIT_TERMINAL}")),
            pane_id: format!("pane-{EXPLICIT_TERMINAL}"),
            provider_session: None,
            owns_tab: false,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Idle,
            state_change_sequence: 1,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: 1,
        }
    }

    #[tokio::test]
    async fn background_task_exit_is_observed_and_remaining_tasks_are_aborted() {
        let mut tasks = JoinSet::new();
        tasks.spawn(async { "exited service" });
        tasks.spawn(async {
            pending::<()>().await;
            "pending service"
        });

        let exited = tasks
            .join_next()
            .await
            .expect("supervised task")
            .expect("task result");
        assert_eq!(exited, "exited service");
        abort_background_tasks(&mut tasks).await;
        assert!(tasks.is_empty());
    }

    #[allow(
        clippy::too_many_lines,
        reason = "end-to-end supervised lifecycle proof"
    )]
    #[tokio::test(flavor = "multi_thread")]
    async fn supervised_server_keeps_unknown_agent_lens_only_after_reopen() {
        let temp = TempDir::new().unwrap();
        let database_path = temp.path().join("yard.sqlite3");
        let config = ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            herdr_binary: OsString::from("unused"),
            database_path: database_path.clone(),
            artifact_path: temp.path().join("artifacts"),
            orchestrator_cwd: temp.path().to_path_buf(),
            coordination_path: temp.path().join("coordination"),
            knowledge_path: temp.path().join("knowledge"),
        };
        let runtime_claim = lifecycle::claim_runtime(&database_path, InstanceMode::Managed)
            .await
            .unwrap();
        let lifecycle_paths = runtime_claim.paths().clone();
        let listener = tokio::net::TcpListener::bind(config.bind).await.unwrap();
        let address = listener.local_addr().unwrap();
        let ui_url = format!("http://{address}/");
        let store = Arc::new(SqliteProjectStore::open(&database_path).await.unwrap());
        let project = store
            .create_project(
                CreateProject {
                    name: "Explicit worker project".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "workspace-supervised".to_owned(),
                    },
                    orchestrator_observed_worker_id: EXPLICIT_TERMINAL.to_owned(),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                explicit_runtime_binding(),
            )
            .await
            .unwrap();
        let project_id = project.id.clone();
        let worker_id = project.orchestrator.id.clone();
        let runtime = Arc::new(LifecycleRuntime {
            inventory_calls: AtomicUsize::new(0),
        });
        let server = tokio::spawn(run_with_services(
            RunMode::Managed {
                instance_id: "018f0000-0000-7000-8000-000000000001",
            },
            InstanceMode::Managed,
            config,
            runtime_claim,
            listener,
            address,
            ui_url,
            runtime.clone(),
            store.clone(),
        ));

        timeout(Duration::from_secs(4), async {
            while runtime.inventory_calls.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let candidates = store.list_worker_candidates().await.unwrap();
        assert_eq!(candidates.workers.len(), 1);
        assert_eq!(candidates.workers[0].worker.id, worker_id);
        assert_eq!(
            candidates.workers[0]
                .worker
                .runtime
                .as_ref()
                .unwrap()
                .terminal_id,
            EXPLICIT_TERMINAL
        );

        lifecycle::request_managed_stop(&lifecycle_paths)
            .await
            .unwrap();
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(store);

        let reopened = SqliteProjectStore::open(&database_path).await.unwrap();
        let restored = reopened.get_project(&project_id).await.unwrap();
        assert_eq!(restored.orchestrator.id, worker_id);
        assert_eq!(
            restored.orchestrator.runtime.as_ref().unwrap().terminal_id,
            EXPLICIT_TERMINAL
        );
        drop(reopened);

        let connection = rusqlite::Connection::open(database_path).unwrap();
        let worker_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM workers", [], |row| row.get(0))
            .unwrap();
        let binding_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM worker_runtime_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        let unknown_binding_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM worker_runtime_bindings WHERE terminal_id = ?1",
                [UNKNOWN_TERMINAL],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(worker_count, 1);
        assert_eq!(binding_count, 1);
        assert_eq!(unknown_binding_count, 0);
    }

    #[test]
    fn managed_log_writer_discards_old_records_at_the_size_limit() {
        let file = tempfile().expect("temporary log");
        let writer = BoundedLogWriter::new(file.try_clone().expect("clone log"), 32);
        {
            let mut output = tracing_subscriber::fmt::MakeWriter::make_writer(&writer);
            output.write_all(&[b'a'; 24]).expect("first record");
            output.write_all(&[b'b'; 24]).expect("second record");
            output.flush().expect("flush");
        }
        assert!(file.metadata().expect("metadata").len() <= 32);
    }
}

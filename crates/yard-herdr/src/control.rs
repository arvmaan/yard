use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use tokio::time::{Instant, sleep};
use yard_domain::{
    ProviderSessionRef, RuntimeObservationState, RuntimeProcessState, WorkerRuntimeBinding,
};

use crate::{
    HerdrConfig, HerdrError,
    discovery::discover_sessions,
    socket::request_command,
    wire::{Agent, Pane, Tab},
};

const AGENT_START_TIMEOUT_MS: u64 = 30_000;
const AGENT_START_READY_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_PROMPT_READY_TIMEOUT: Duration = Duration::from_millis(AGENT_START_TIMEOUT_MS);
const HERDR_PANE_READ_MAX_LINES: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionAgentRequest {
    pub command_id: String,
    pub session: String,
    pub workspace_id: String,
    pub cwd: String,
    pub tab_label: String,
    pub agent_name: String,
    pub kind: String,
    pub args: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapAgentRequest {
    pub command_id: String,
    pub session: String,
    pub workspace_label: String,
    pub cwd: String,
    pub agent_name: String,
    pub kind: String,
    pub args: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareAgentRequest {
    pub command_id: String,
    pub session: String,
    pub workspace_id: String,
    pub cwd: String,
    pub tab_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareWorkspaceAgentRequest {
    pub command_id: String,
    pub session: String,
    pub workspace_label: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedAgent {
    pub runtime: WorkerRuntimeBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPreparedAgentRequest {
    pub command_id: String,
    pub prepared: WorkerRuntimeBinding,
    pub agent_name: String,
    pub kind: String,
    pub args: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedAgent {
    pub runtime: WorkerRuntimeBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAgentRequest {
    pub command_id: String,
    pub session: String,
    pub pane_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptedAgent {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPaneRequest {
    pub request_id: String,
    pub session: String,
    pub pane_id: String,
    pub lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneOutput {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub source: String,
    pub format: String,
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
}

#[derive(Debug, Error)]
pub enum HerdrControlError {
    #[error(transparent)]
    Runtime(#[from] HerdrError),
    #[error("Herdr workspace creation failed: {source}; command outcome ambiguous: {ambiguous}")]
    WorkspaceCreateFailed { source: HerdrError, ambiguous: bool },
    #[error("Herdr tab preparation failed: {source}; command outcome ambiguous: {ambiguous}")]
    PrepareFailed { source: HerdrError, ambiguous: bool },
    #[error("Herdr agent start failed: {start}; runtime rollback: {rollback}")]
    StartFailed {
        start: HerdrError,
        rollback: String,
        rollback_succeeded: bool,
        started_runtime: Option<Box<WorkerRuntimeBinding>>,
    },
    #[error(
        "Herdr created the worker but initial prompt delivery failed: {source}; runtime rollback: {rollback}"
    )]
    PromptDeliveryFailed {
        source: HerdrError,
        rollback: String,
        rollback_succeeded: bool,
        started_runtime: Option<Box<WorkerRuntimeBinding>>,
    },
}

pub(crate) async fn provision_agent(
    config: &HerdrConfig,
    request: ProvisionAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let session = running_session(config, &request.session).await?;
    provision_agent_at_socket(config, &session.socket_path, request).await
}

async fn provision_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: ProvisionAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let prepared = prepare_agent_at_socket(
        config,
        socket_path,
        PrepareAgentRequest {
            command_id: request.command_id.clone(),
            session: request.session,
            workspace_id: request.workspace_id,
            cwd: request.cwd,
            tab_label: request.tab_label,
        },
    )
    .await?;
    start_prepared_agent_at_socket(
        config,
        socket_path,
        StartPreparedAgentRequest {
            command_id: request.command_id,
            prepared: prepared.runtime,
            agent_name: request.agent_name,
            kind: request.kind,
            args: request.args,
            prompt: request.prompt,
        },
    )
    .await
}

pub(crate) async fn bootstrap_agent(
    config: &HerdrConfig,
    request: BootstrapAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let session = running_session(config, &request.session).await?;
    bootstrap_agent_at_socket(config, &session.socket_path, request).await
}

async fn bootstrap_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: BootstrapAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let prepared = prepare_workspace_agent_at_socket(
        config,
        socket_path,
        PrepareWorkspaceAgentRequest {
            command_id: request.command_id.clone(),
            session: request.session,
            workspace_label: request.workspace_label,
            cwd: request.cwd,
        },
    )
    .await?;
    start_prepared_workspace_agent_at_socket(
        config,
        socket_path,
        StartPreparedAgentRequest {
            command_id: request.command_id,
            prepared: prepared.runtime,
            agent_name: request.agent_name,
            kind: request.kind,
            args: request.args,
            prompt: request.prompt,
        },
    )
    .await
}

pub(crate) async fn prepare_workspace_agent(
    config: &HerdrConfig,
    request: PrepareWorkspaceAgentRequest,
) -> Result<PreparedAgent, HerdrControlError> {
    let session = running_session(config, &request.session).await?;
    prepare_workspace_agent_at_socket(config, &session.socket_path, request).await
}

async fn prepare_workspace_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: PrepareWorkspaceAgentRequest,
) -> Result<PreparedAgent, HerdrControlError> {
    let create_result = request_command(
        config,
        socket_path,
        &format!("yard:{}:workspace", request.command_id),
        "workspace.create",
        json!({
            "cwd": request.cwd,
            "label": request.workspace_label,
            "focus": false,
            "env": {}
        }),
        config.request_timeout,
    )
    .await
    .map_err(|source| HerdrControlError::WorkspaceCreateFailed {
        ambiguous: runtime_creation_outcome_ambiguous(&source),
        source,
    })?;
    expect_result_type(&create_result, "workspace_created").map_err(|source| {
        HerdrControlError::WorkspaceCreateFailed {
            source,
            ambiguous: true,
        }
    })?;
    let created: WorkspaceCreated = serde_json::from_value(create_result).map_err(|source| {
        HerdrControlError::WorkspaceCreateFailed {
            source: HerdrError::CommandDecode(source),
            ambiguous: true,
        }
    })?;
    if created.workspace.workspace_id != created.tab.workspace_id
        || created.workspace.workspace_id != created.root_pane.workspace_id
        || created.tab.tab_id != created.root_pane.tab_id
    {
        return Err(HerdrControlError::WorkspaceCreateFailed {
            source: HerdrError::InvalidTopology(
                "created workspace, tab, and root pane have inconsistent ancestry".to_owned(),
            ),
            ambiguous: true,
        });
    }

    let prepared = prepared_runtime_binding(&request.session, &created.tab, &created.root_pane);
    Ok(PreparedAgent { runtime: prepared })
}

pub(crate) async fn prepare_agent(
    config: &HerdrConfig,
    request: PrepareAgentRequest,
) -> Result<PreparedAgent, HerdrControlError> {
    let session = running_session(config, &request.session).await?;
    prepare_agent_at_socket(config, &session.socket_path, request).await
}

async fn prepare_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: PrepareAgentRequest,
) -> Result<PreparedAgent, HerdrControlError> {
    let tab_request_id = format!("yard:{}:tab", request.command_id);
    let tab_result = request_command(
        config,
        socket_path,
        &tab_request_id,
        "tab.create",
        json!({
            "workspace_id": request.workspace_id,
            "cwd": request.cwd,
            "label": request.tab_label,
            "env": {},
            "focus": false
        }),
        config.request_timeout,
    )
    .await
    .map_err(|source| HerdrControlError::PrepareFailed {
        ambiguous: runtime_creation_outcome_ambiguous(&source),
        source,
    })?;
    expect_result_type(&tab_result, "tab_created").map_err(|source| {
        HerdrControlError::PrepareFailed {
            source,
            ambiguous: true,
        }
    })?;
    let tab: TabCreated =
        serde_json::from_value(tab_result).map_err(|source| HerdrControlError::PrepareFailed {
            source: HerdrError::CommandDecode(source),
            ambiguous: true,
        })?;
    if tab.tab.workspace_id != request.workspace_id
        || tab.root_pane.workspace_id != request.workspace_id
        || tab.tab.workspace_id != tab.root_pane.workspace_id
        || tab.tab.tab_id != tab.root_pane.tab_id
    {
        return Err(HerdrControlError::PrepareFailed {
            source: HerdrError::InvalidTopology(
                "created tab and root pane do not belong to the requested workspace and ancestry"
                    .to_owned(),
            ),
            ambiguous: true,
        });
    }
    Ok(PreparedAgent {
        runtime: prepared_runtime_binding(&request.session, &tab.tab, &tab.root_pane),
    })
}

pub(crate) async fn start_prepared_agent(
    config: &HerdrConfig,
    request: StartPreparedAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let session = running_session(config, &request.prepared.session).await?;
    start_prepared_agent_at_socket(config, &session.socket_path, request).await
}

pub(crate) async fn start_existing_agent(
    config: &HerdrConfig,
    request: StartPreparedAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let session = running_session(config, &request.prepared.session).await?;
    start_agent_at_socket(config, &session.socket_path, request, StartRollback::None).await
}

pub(crate) async fn start_prepared_workspace_agent(
    config: &HerdrConfig,
    request: StartPreparedAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let session = running_session(config, &request.prepared.session).await?;
    start_prepared_workspace_agent_at_socket(config, &session.socket_path, request).await
}

async fn start_prepared_workspace_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: StartPreparedAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let workspace_id = request.prepared.workspace_id.clone();
    start_agent_at_socket(
        config,
        socket_path,
        request,
        StartRollback::Workspace(workspace_id),
    )
    .await
}

async fn start_prepared_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: StartPreparedAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let tab_id = request
        .prepared
        .tab_id
        .clone()
        .ok_or_else(|| HerdrError::UnexpectedResult {
            expected: "prepared runtime tab_id",
            actual: "missing".to_owned(),
        })?;
    start_agent_at_socket(config, socket_path, request, StartRollback::Tab(tab_id)).await
}

enum StartRollback {
    None,
    Tab(String),
    Workspace(String),
}

impl StartRollback {
    fn covers_started_runtime(&self) -> bool {
        !matches!(self, Self::None)
    }
}

async fn start_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: StartPreparedAgentRequest,
    rollback: StartRollback,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let ready_deadline = Instant::now() + AGENT_START_READY_TIMEOUT;
    let mut attempt = 1_u32;
    let started = loop {
        let result = request_command(
            config,
            socket_path,
            &format!("yard:{}:start:{attempt}", request.command_id),
            "agent.start",
            json!({
                "name": request.agent_name.clone(),
                "kind": request.kind,
                "pane_id": request.prepared.pane_id,
                "args": request.args.clone(),
                "timeout_ms": AGENT_START_TIMEOUT_MS
            }),
            Duration::from_millis(AGENT_START_TIMEOUT_MS + 5_000),
        )
        .await
        .and_then(|result| {
            expect_result_type(&result, "agent_started")?;
            serde_json::from_value::<AgentStarted>(result).map_err(HerdrError::CommandDecode)
        });
        if matches!(
            &result,
            Err(HerdrError::Api { code, .. }) if code == "agent_pane_busy"
        ) && Instant::now() < ready_deadline
        {
            attempt = attempt.saturating_add(1);
            sleep(Duration::from_millis(100)).await;
            continue;
        }
        break result;
    };
    let started = match started {
        Ok(started) => started,
        Err(start) => {
            let rollback_covers_started_runtime = rollback.covers_started_runtime();
            let started_runtime =
                runtime_creation_outcome_ambiguous(&start).then(|| request.prepared.clone());
            return Err(start_failure_after_rollback(
                config,
                socket_path,
                &request.command_id,
                &rollback,
                start,
                rollback_covers_started_runtime,
                started_runtime,
            )
            .await);
        }
    };
    let runtime =
        validate_started_topology(config, socket_path, &request, &rollback, started).await?;

    let prompt_result = deliver_prompt_when_ready(
        config,
        socket_path,
        &request.command_id,
        &runtime.pane_id,
        &request.prompt,
        AGENT_PROMPT_READY_TIMEOUT,
    )
    .await;
    match prompt_result {
        Ok(()) => Ok(ProvisionedAgent { runtime }),
        Err(source) => Err(prompt_failure_after_rollback(
            config,
            socket_path,
            &request.command_id,
            &rollback,
            source,
            runtime,
        )
        .await),
    }
}

async fn validate_started_topology(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: &StartPreparedAgentRequest,
    rollback: &StartRollback,
    started: AgentStarted,
) -> Result<WorkerRuntimeBinding, HerdrControlError> {
    let runtime = runtime_binding(&request.prepared.session, started.agent);
    if !retained_prepared_topology(&request.prepared, &runtime) {
        let start = HerdrError::InvalidTopology(
            "started agent did not retain the prepared workspace, tab, pane, and terminal"
                .to_owned(),
        );
        return Err(start_failure_after_rollback(
            config,
            socket_path,
            &request.command_id,
            rollback,
            start,
            false,
            Some(runtime),
        )
        .await);
    }
    Ok(runtime)
}

async fn start_failure_after_rollback(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    rollback: &StartRollback,
    start: HerdrError,
    rollback_covers_started_runtime: bool,
    started_runtime: Option<WorkerRuntimeBinding>,
) -> HerdrControlError {
    let rollback_result = rollback_start(config, socket_path, command_id, rollback).await;
    let rollback_succeeded = rollback_covers_started_runtime && rollback_result.is_ok();
    let started_runtime = if rollback_succeeded {
        None
    } else {
        started_runtime
    };
    let rollback = rollback_result.map_or_else(
        |error| error.to_string(),
        |()| match rollback {
            StartRollback::None => "not attempted; existing topology preserved".to_owned(),
            _ if rollback_covers_started_runtime => "succeeded".to_owned(),
            _ => {
                "prepared topology closed; mismatched started runtime remains unverified".to_owned()
            }
        },
    );
    HerdrControlError::StartFailed {
        start,
        rollback,
        rollback_succeeded,
        started_runtime: started_runtime.map(Box::new),
    }
}

async fn prompt_failure_after_rollback(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    rollback: &StartRollback,
    source: HerdrError,
    runtime: WorkerRuntimeBinding,
) -> HerdrControlError {
    if !prompt_definitely_not_submitted(&source) || !rollback.covers_started_runtime() {
        return HerdrControlError::PromptDeliveryFailed {
            source,
            rollback: if rollback.covers_started_runtime() {
                "not attempted; prompt submission outcome is unknown".to_owned()
            } else {
                "not attempted; existing topology preserved".to_owned()
            },
            rollback_succeeded: false,
            started_runtime: Some(Box::new(runtime)),
        };
    }

    match rollback_start(config, socket_path, command_id, rollback).await {
        Ok(()) => HerdrControlError::PromptDeliveryFailed {
            source,
            rollback: "succeeded".to_owned(),
            rollback_succeeded: true,
            started_runtime: None,
        },
        Err(error) => HerdrControlError::PromptDeliveryFailed {
            source,
            rollback: error.to_string(),
            rollback_succeeded: false,
            started_runtime: Some(Box::new(runtime)),
        },
    }
}

fn retained_prepared_topology(
    prepared: &WorkerRuntimeBinding,
    started: &WorkerRuntimeBinding,
) -> bool {
    started.workspace_id == prepared.workspace_id
        && started.tab_id == prepared.tab_id
        && started.pane_id == prepared.pane_id
        && started.terminal_id == prepared.terminal_id
}

pub(crate) async fn prompt_agent(
    config: &HerdrConfig,
    request: PromptAgentRequest,
) -> Result<PromptedAgent, HerdrError> {
    let session = running_session(config, &request.session).await?;
    prompt_agent_at_socket(config, &session.socket_path, request).await
}

async fn prompt_agent_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: PromptAgentRequest,
) -> Result<PromptedAgent, HerdrError> {
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{}:prompt", request.command_id),
        "agent.prompt",
        json!({
            "target": request.pane_id,
            "text": request.text,
            "wait": null
        }),
        config.request_timeout,
    )
    .await?;
    expect_result_type(&result, "agent_prompted")?;
    let result: AgentPrompted =
        serde_json::from_value(result).map_err(HerdrError::CommandDecode)?;
    Ok(PromptedAgent {
        status: result
            .agent
            .agent_status
            .unwrap_or_else(|| "unknown".to_owned()),
    })
}

pub(crate) async fn read_pane(
    config: &HerdrConfig,
    request: ReadPaneRequest,
) -> Result<PaneOutput, HerdrError> {
    let session = running_session(config, &request.session).await?;
    read_pane_at_socket(config, &session.socket_path, request).await
}

async fn read_pane_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: ReadPaneRequest,
) -> Result<PaneOutput, HerdrError> {
    let request_id = request.request_id.as_str();
    let pane_id = request.pane_id.as_str();
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{request_id}:read"),
        "pane.read",
        json!({
            "pane_id": pane_id,
            "source": "recent_unwrapped",
            "lines": request.lines,
            "format": "text",
            "strip_ansi": true
        }),
        config.request_timeout,
    )
    .await?;
    expect_result_type(&result, "pane_read")?;
    let result: PaneRead = serde_json::from_value(result).map_err(HerdrError::CommandDecode)?;
    let mut output = result.read;
    if request.lines > HERDR_PANE_READ_MAX_LINES
        && output.truncated
        && let Ok(Some(history)) = read_retained_pane_history(
            config,
            socket_path,
            request_id,
            pane_id,
            request.lines,
            &output,
        )
        .await
    {
        output.text = history.text;
        output.truncated = history.truncated;
    }
    Ok(output)
}

async fn read_retained_pane_history(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request_id: &str,
    pane_id: &str,
    lines: u32,
    output: &PaneOutput,
) -> Result<Option<RetainedPaneHistory>, HerdrError> {
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{request_id}:read-info"),
        "pane.get",
        json!({ "pane_id": pane_id }),
        config.request_timeout,
    )
    .await?;
    expect_result_type(&result, "pane_info")?;
    let result: PaneInfoResult =
        serde_json::from_value(result).map_err(HerdrError::CommandDecode)?;
    if result.pane.pane_id != pane_id
        || result.pane.workspace_id != output.workspace_id
        || result.pane.tab_id != output.tab_id
    {
        return Ok(None);
    }
    let Some(scroll) = result.pane.scroll else {
        return Ok(None);
    };
    let total_rows = scroll
        .max_offset_from_bottom
        .saturating_add(scroll.viewport_rows);
    if total_rows <= u64::from(HERDR_PANE_READ_MAX_LINES) || total_rows == 0 {
        return Ok(None);
    }
    let start_row = total_rows.saturating_sub(u64::from(lines));
    let end_row = total_rows.saturating_sub(1);
    let Ok(start_row) = u32::try_from(start_row) else {
        return Ok(None);
    };
    let Ok(end_row) = u32::try_from(end_row) else {
        return Ok(None);
    };

    let result = request_command(
        config,
        socket_path,
        &format!("yard:{request_id}:read-line-end"),
        "pane.copy_motion",
        json!({
            "pane_id": pane_id,
            "cursor": {
                "row": end_row,
                "col": 0
            },
            "motion": "line_end"
        }),
        config.request_timeout,
    )
    .await?;
    expect_result_type(&result, "pane_copy_motion")?;
    let motion: PaneCopyMotion =
        serde_json::from_value(result).map_err(HerdrError::CommandDecode)?;
    if motion.pane_id != pane_id || motion.cursor.row != end_row {
        return Ok(None);
    }

    let result = request_command(
        config,
        socket_path,
        &format!("yard:{request_id}:read-selection"),
        "pane.selection.read",
        json!({
            "pane_id": pane_id,
            "anchor": {
                "row": start_row,
                "col": 0
            },
            "cursor": motion.cursor,
            "content_revision": motion.content_revision
        }),
        config.request_timeout,
    )
    .await?;
    expect_result_type(&result, "pane_selection")?;
    let selection: PaneSelection =
        serde_json::from_value(result).map_err(HerdrError::CommandDecode)?;
    if selection.pane_id != pane_id || selection.text.is_empty() {
        return Ok(None);
    }
    Ok(Some(RetainedPaneHistory {
        text: selection.text,
        truncated: start_row > 0,
    }))
}

async fn running_session(
    config: &HerdrConfig,
    session_name: &str,
) -> Result<crate::discovery::HerdrSession, HerdrError> {
    let sessions = discover_sessions(config).await?;
    let session = sessions
        .into_iter()
        .find(|session| session.name == session_name)
        .ok_or_else(|| HerdrError::SessionNotFound(session_name.to_owned()))?;
    if !session.running {
        return Err(HerdrError::SessionNotRunning(session_name.to_owned()));
    }
    Ok(session)
}

async fn deliver_prompt_when_ready(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    pane_id: &str,
    prompt: &str,
    ready_timeout: Duration,
) -> Result<(), HerdrError> {
    let result = prompt_when_ready(
        config,
        socket_path,
        command_id,
        pane_id,
        prompt,
        ready_timeout,
    )
    .await?;
    expect_result_type(&result, "agent_prompted")
}

async fn prompt_when_ready(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    pane_id: &str,
    prompt: &str,
    ready_timeout: Duration,
) -> Result<serde_json::Value, HerdrError> {
    let deadline = Instant::now() + ready_timeout;
    let mut attempt = 1_u32;
    loop {
        let result = request_command(
            config,
            socket_path,
            &format!("yard:{command_id}:prompt:{attempt}"),
            "agent.prompt",
            json!({
                "target": pane_id,
                "text": prompt,
                "wait": null
            }),
            config.request_timeout,
        )
        .await;
        match result {
            Ok(result) => return Ok(result),
            Err(error) => {
                let retryable = is_prompt_readiness_retryable(&error);
                if !retryable || Instant::now() >= deadline {
                    return Err(error);
                }
            }
        }
        attempt = attempt.saturating_add(1);
        sleep(Duration::from_millis(100)).await;
    }
}

fn is_prompt_readiness_retryable(error: &HerdrError) -> bool {
    matches!(
        error,
        HerdrError::Api { code, .. }
            if code == "agent_not_ready" || code == "agent_not_found"
    )
}

fn prompt_definitely_not_submitted(error: &HerdrError) -> bool {
    is_prompt_readiness_retryable(error)
        || matches!(
            error,
            HerdrError::Api { code, .. } if code == "agent_blocked"
        )
}

fn runtime_creation_outcome_ambiguous(error: &HerdrError) -> bool {
    !matches!(
        error,
        HerdrError::SocketConnect { .. } | HerdrError::Api { .. }
    )
}

async fn rollback_start(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    rollback: &StartRollback,
) -> Result<(), HerdrError> {
    match rollback {
        StartRollback::None => Ok(()),
        StartRollback::Tab(tab_id) => close_tab(config, socket_path, command_id, tab_id).await,
        StartRollback::Workspace(workspace_id) => {
            close_workspace(config, socket_path, command_id, workspace_id).await
        }
    }
}

async fn close_tab(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    tab_id: &str,
) -> Result<(), HerdrError> {
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{command_id}:rollback-tab"),
        "tab.close",
        json!({ "tab_id": tab_id }),
        config.request_timeout,
    )
    .await?;
    expect_tab_close_result(&result)
}

async fn close_workspace(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    workspace_id: &str,
) -> Result<(), HerdrError> {
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{command_id}:rollback-workspace"),
        "workspace.close",
        json!({ "workspace_id": workspace_id }),
        config.request_timeout,
    )
    .await;
    match result {
        Ok(result) => expect_result_type(&result, "ok"),
        Err(HerdrError::Api { code, .. }) if code == "workspace_not_found" => Ok(()),
        Err(error) => Err(error),
    }
}

fn expect_result_type(
    result: &serde_json::Value,
    expected: &'static str,
) -> Result<(), HerdrError> {
    let actual = result
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("missing")
        .to_owned();
    if actual == expected {
        Ok(())
    } else {
        Err(HerdrError::UnexpectedResult { expected, actual })
    }
}

fn expect_tab_close_result(result: &serde_json::Value) -> Result<(), HerdrError> {
    let actual = result
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("missing")
        .to_owned();
    if actual == "ok" || actual == "tab_closed" {
        Ok(())
    } else {
        Err(HerdrError::UnexpectedResult {
            expected: "ok or tab_closed",
            actual,
        })
    }
}

fn runtime_binding(session: &str, agent: Agent) -> WorkerRuntimeBinding {
    let provider_session = agent.agent_session.map(|session| ProviderSessionRef {
        source: session.source,
        provider: session.agent,
        kind: session.kind,
        value: session.value,
    });
    WorkerRuntimeBinding {
        adapter: "herdr".to_owned(),
        session: session.to_owned(),
        workspace_id: agent.workspace_id,
        terminal_id: agent.terminal_id,
        tab_id: Some(agent.tab_id),
        pane_id: agent.pane_id,
        provider_session,
        owns_tab: true,
        observation_state: RuntimeObservationState::Observed,
        process_state: RuntimeProcessState::Running,
        status: crate::normalize::status(&agent.agent_status),
        state_change_sequence: agent.state_change_seq,
        revision: agent.revision,
        version: 1,
        last_observed_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            }),
    }
}

fn prepared_runtime_binding(session: &str, tab: &Tab, pane: &Pane) -> WorkerRuntimeBinding {
    WorkerRuntimeBinding {
        adapter: "herdr".to_owned(),
        session: session.to_owned(),
        workspace_id: pane.workspace_id.clone(),
        terminal_id: pane.terminal_id.clone(),
        tab_id: Some(tab.tab_id.clone()),
        pane_id: pane.pane_id.clone(),
        provider_session: None,
        owns_tab: true,
        observation_state: RuntimeObservationState::Observed,
        process_state: RuntimeProcessState::Unknown,
        status: crate::normalize::status(&pane.agent_status),
        state_change_sequence: 0,
        revision: pane.revision,
        version: 1,
        last_observed_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            }),
    }
}

#[derive(Debug, Deserialize)]
struct TabCreated {
    tab: Tab,
    root_pane: Pane,
}

#[derive(Debug, Deserialize)]
struct WorkspaceCreated {
    workspace: CreatedWorkspace,
    tab: Tab,
    root_pane: Pane,
}

#[derive(Debug, Deserialize)]
struct CreatedWorkspace {
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct AgentStarted {
    agent: Agent,
}

#[derive(Debug, Deserialize)]
struct AgentPrompted {
    agent: PromptedAgentWire,
}

#[derive(Debug, Deserialize)]
struct PromptedAgentWire {
    agent_status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaneRead {
    read: PaneOutput,
}

#[derive(Debug, Deserialize)]
struct PaneInfoResult {
    pane: PaneHistoryInfo,
}

#[derive(Debug, Deserialize)]
struct PaneHistoryInfo {
    pane_id: String,
    workspace_id: String,
    tab_id: String,
    scroll: Option<PaneScrollInfo>,
}

#[derive(Debug, Deserialize)]
struct PaneScrollInfo {
    max_offset_from_bottom: u64,
    viewport_rows: u64,
}

#[derive(Debug, Deserialize)]
struct PaneCopyMotion {
    pane_id: String,
    cursor: PaneTextPoint,
    content_revision: u64,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct PaneTextPoint {
    row: u32,
    col: u16,
}

#[derive(Debug, Deserialize)]
struct PaneSelection {
    pane_id: String,
    text: String,
}

struct RetainedPaneHistory {
    text: String,
    truncated: bool,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    use super::{
        BootstrapAgentRequest, HerdrControlError, PrepareAgentRequest, PromptAgentRequest,
        ProvisionAgentRequest, ReadPaneRequest, StartPreparedAgentRequest, StartRollback,
        bootstrap_agent_at_socket, close_tab, close_workspace, deliver_prompt_when_ready,
        expect_tab_close_result, prepare_agent_at_socket, prompt_agent_at_socket,
        prompt_definitely_not_submitted, prompt_failure_after_rollback, provision_agent_at_socket,
        read_pane_at_socket, retained_prepared_topology, runtime_creation_outcome_ambiguous,
        start_agent_at_socket, start_prepared_agent_at_socket,
    };
    use crate::{HerdrConfig, HerdrError};
    use yard_domain::{
        ObservedStatus, RuntimeObservationState, RuntimeProcessState, WorkerRuntimeBinding,
    };

    fn runtime_topology(
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        terminal_id: &str,
    ) -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: workspace_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            tab_id: Some(tab_id.to_owned()),
            pane_id: pane_id.to_owned(),
            provider_session: None,
            owns_tab: true,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Idle,
            state_change_sequence: 1,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: 1,
        }
    }

    fn started_claude_response(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "result": {
                "type": "agent_started",
                "agent": {
                    "terminal_id": "terminal-1",
                    "name": "yard-blocked",
                    "agent": "claude",
                    "display_agent": "Claude",
                    "agent_status": "idle",
                    "tokens": {},
                    "agent_session": {
                        "source": "herdr:claude",
                        "agent": "claude",
                        "kind": "id",
                        "value": "session-1"
                    },
                    "workspace_id": "workspace-1",
                    "tab_id": "tab-1",
                    "pane_id": "pane-1",
                    "focused": false,
                    "launch_pending": false,
                    "interactive_ready": true,
                    "state_change_seq": 2,
                    "cwd": "/tmp/project",
                    "foreground_cwd": "/tmp/project",
                    "revision": 2
                }
            }
        })
    }

    fn extended_history_result(step: usize, request: &serde_json::Value) -> serde_json::Value {
        match step {
            0 => {
                assert_eq!(request["method"], "pane.read");
                assert_eq!(request["params"]["lines"], 10_000);
                serde_json::json!({
                    "type": "pane_read",
                    "read": {
                        "pane_id": "pane-1",
                        "workspace_id": "workspace-1",
                        "tab_id": "tab-1",
                        "source": "recent_unwrapped",
                        "format": "text",
                        "text": "latest 1000 lines",
                        "revision": 0,
                        "truncated": true
                    }
                })
            }
            1 => {
                assert_eq!(request["method"], "pane.get");
                assert_eq!(request["params"]["pane_id"], "pane-1");
                serde_json::json!({
                    "type": "pane_info",
                    "pane": {
                        "pane_id": "pane-1",
                        "workspace_id": "workspace-1",
                        "tab_id": "tab-1",
                        "scroll": {
                            "max_offset_from_bottom": 11_976,
                            "offset_from_bottom": 0,
                            "viewport_rows": 24
                        }
                    }
                })
            }
            2 => {
                assert_eq!(request["method"], "pane.copy_motion");
                assert_eq!(request["params"]["cursor"]["row"], 11_999);
                assert_eq!(request["params"]["cursor"]["col"], 0);
                assert_eq!(request["params"]["motion"], "line_end");
                serde_json::json!({
                    "type": "pane_copy_motion",
                    "pane_id": "pane-1",
                    "cursor": {
                        "row": 11_999,
                        "col": 72
                    },
                    "content_revision": 44
                })
            }
            _ => {
                assert_eq!(request["method"], "pane.selection.read");
                assert_eq!(request["params"]["anchor"]["row"], 2_000);
                assert_eq!(request["params"]["anchor"]["col"], 0);
                assert_eq!(request["params"]["cursor"]["row"], 11_999);
                assert_eq!(request["params"]["cursor"]["col"], 72);
                assert_eq!(request["params"]["content_revision"], 44);
                serde_json::json!({
                    "type": "pane_selection",
                    "pane_id": "pane-1",
                    "text": "selected 10000 lines"
                })
            }
        }
    }

    #[test]
    fn started_agent_must_retain_the_prepared_topology() {
        let prepared = runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1");

        assert!(retained_prepared_topology(&prepared, &prepared));
        assert!(!retained_prepared_topology(
            &prepared,
            &runtime_topology("workspace-2", "tab-1", "pane-1", "terminal-1"),
        ));
    }

    #[test]
    fn classifies_only_explicitly_unsent_prompt_errors_for_rollback() {
        for code in ["agent_blocked", "agent_not_ready", "agent_not_found"] {
            assert!(prompt_definitely_not_submitted(&HerdrError::Api {
                code: code.to_owned(),
                message: "prompt was not submitted".to_owned(),
            }));
        }
        assert!(!prompt_definitely_not_submitted(&HerdrError::SocketTimeout));
    }

    #[tokio::test]
    async fn uncertain_prompt_submission_preserves_started_runtime_without_rollback() {
        let runtime = runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1");
        let error = prompt_failure_after_rollback(
            &HerdrConfig::default(),
            std::path::Path::new("/unused/herdr.sock"),
            "uncertain-prompt",
            &StartRollback::Tab("tab-1".to_owned()),
            HerdrError::SocketTimeout,
            runtime,
        )
        .await;

        assert!(matches!(
            error,
            HerdrControlError::PromptDeliveryFailed {
                rollback,
                rollback_succeeded: false,
                started_runtime: Some(runtime),
                ..
            } if rollback == "not attempted; prompt submission outcome is unknown"
                && runtime.terminal_id == "terminal-1"
        ));
    }

    #[tokio::test]
    async fn blocked_prompt_closes_the_yard_owned_tab_without_raw_input() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let response = match step {
                    0 => {
                        assert_eq!(request["method"], "agent.start");
                        started_claude_response(id)
                    }
                    1 => {
                        assert_eq!(request["method"], "agent.prompt");
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "agent_blocked",
                                "message": "permission prompt requires operator input"
                            }
                        })
                    }
                    _ => {
                        assert_eq!(request["method"], "tab.close");
                        assert_eq!(request["params"]["tab_id"], "tab-1");
                        serde_json::json!({
                            "id": id,
                            "result": { "type": "ok" }
                        })
                    }
                };
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = start_prepared_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "blocked".to_owned(),
                prepared: runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1"),
                agent_name: "yard-blocked".to_owned(),
                kind: "claude".to_owned(),
                args: vec!["--permission-mode".to_owned(), "auto".to_owned()],
                prompt: "Continue.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::PromptDeliveryFailed {
                rollback_succeeded: true,
                started_runtime: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn blocked_prompt_preserves_existing_topology() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let response = if step == 0 {
                    assert_eq!(request["method"], "agent.start");
                    started_claude_response(id)
                } else {
                    assert_eq!(request["method"], "agent.prompt");
                    serde_json::json!({
                        "id": id,
                        "error": {
                            "code": "agent_blocked",
                            "message": "permission prompt requires operator input"
                        }
                    })
                };
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = start_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "blocked-existing".to_owned(),
                prepared: runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1"),
                agent_name: "yard-blocked".to_owned(),
                kind: "claude".to_owned(),
                args: Vec::new(),
                prompt: "Continue.".to_owned(),
            },
            StartRollback::None,
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::PromptDeliveryFailed {
                rollback,
                rollback_succeeded: false,
                started_runtime: Some(runtime),
                ..
            } if rollback == "not attempted; existing topology preserved"
                && runtime.terminal_id == "terminal-1"
        ));
    }

    #[tokio::test]
    async fn failed_prompt_rollback_retains_the_started_runtime() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let response = match step {
                    0 => started_claude_response(id),
                    1 => serde_json::json!({
                        "id": id,
                        "error": {
                            "code": "agent_blocked",
                            "message": "permission prompt requires operator input"
                        }
                    }),
                    _ => {
                        assert_eq!(request["method"], "tab.close");
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "tab_close_failed",
                                "message": "tab is still busy"
                            }
                        })
                    }
                };
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = start_prepared_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "blocked-close-failed".to_owned(),
                prepared: runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1"),
                agent_name: "yard-blocked".to_owned(),
                kind: "claude".to_owned(),
                args: Vec::new(),
                prompt: "Continue.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::PromptDeliveryFailed {
                rollback_succeeded: false,
                started_runtime: Some(runtime),
                ..
            } if runtime.terminal_id == "terminal-1"
        ));
    }

    #[test]
    fn tab_close_result_accepts_legacy_and_herdr_0_8_2_shapes() {
        expect_tab_close_result(&serde_json::json!({ "type": "tab_closed" })).unwrap();
        expect_tab_close_result(&serde_json::json!({ "type": "ok" })).unwrap();
    }

    #[tokio::test]
    async fn definite_existing_pane_start_rejection_does_not_capture_runtime() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "agent.start");
            let id = request["id"].as_str().unwrap();
            writer
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "agent_name_taken",
                                "message": "agent name is already in use"
                            }
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };
        let error = start_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "retained-start-failure".to_owned(),
                prepared: runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1"),
                agent_name: "yard-orchestrator".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Coordinate projects.".to_owned(),
            },
            StartRollback::None,
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        let HerdrControlError::StartFailed {
            rollback,
            rollback_succeeded,
            started_runtime: None,
            ..
        } = error
        else {
            panic!("expected definite retained-pane start rejection");
        };
        assert!(!rollback_succeeded);
        assert_eq!(rollback, "not attempted; existing topology preserved");
    }

    #[tokio::test]
    async fn definite_start_rejection_with_failed_cleanup_does_not_capture_runtime() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for expected_method in ["agent.start", "tab.close"] {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["method"], expected_method);
                let id = request["id"].as_str().unwrap();
                let response = if expected_method == "agent.start" {
                    serde_json::json!({
                        "id": id,
                        "error": {
                            "code": "agent_name_taken",
                            "message": "agent name is already in use"
                        }
                    })
                } else {
                    serde_json::json!({
                        "id": id,
                        "error": {
                            "code": "tab_close_failed",
                            "message": "tab is still busy"
                        }
                    })
                };
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = start_prepared_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "rejected-start-failed-cleanup".to_owned(),
                prepared: runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1"),
                agent_name: "yard-orchestrator".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Coordinate projects.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        let HerdrControlError::StartFailed {
            rollback,
            rollback_succeeded,
            started_runtime,
            ..
        } = error
        else {
            panic!("expected definite start rejection with failed cleanup");
        };
        assert!(!rollback_succeeded);
        assert!(started_runtime.is_none());
        assert!(rollback.contains("tab_close_failed"));
    }

    #[tokio::test]
    async fn ambiguous_existing_pane_start_failure_preserves_runtime_as_unverified() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, _writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "agent.start");
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_millis(100),
            ..HerdrConfig::default()
        };
        let prepared = runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1");

        let error = start_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "ambiguous-retained-start-failure".to_owned(),
                prepared: prepared.clone(),
                agent_name: "yard-orchestrator".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Coordinate projects.".to_owned(),
            },
            StartRollback::None,
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        let HerdrControlError::StartFailed {
            rollback,
            rollback_succeeded,
            started_runtime: Some(started_runtime),
            ..
        } = error
        else {
            panic!("expected ambiguous retained-pane start failure");
        };
        assert!(!rollback_succeeded);
        assert_eq!(rollback, "not attempted; existing topology preserved");
        assert_eq!(*started_runtime, prepared);
    }

    #[tokio::test]
    async fn unavailable_agent_prompt_never_falls_back_to_raw_pane_input() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "agent.prompt");
            let id = request["id"].as_str().unwrap();
            writer
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "agent_not_found",
                                "message": "agent launch exited"
                            }
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = deliver_prompt_when_ready(
            &config,
            &socket_path,
            "retained-prompt-failure",
            "pane-1",
            "Coordinate projects.",
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrError::Api { code, .. } if code == "agent_not_found"
        ));
    }

    #[tokio::test]
    async fn topology_mismatch_retains_the_unverified_started_runtime() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let result = if step == 0 {
                    assert_eq!(request["method"], "agent.start");
                    serde_json::json!({
                        "type": "agent_started",
                        "agent": {
                            "terminal_id": "terminal-mismatch",
                            "name": "yard-replacement",
                            "agent": "codex",
                            "display_agent": "Codex",
                            "agent_status": "idle",
                            "tokens": {},
                            "agent_session": {
                                "source": "herdr:codex",
                                "agent": "codex",
                                "kind": "id",
                                "value": "session-mismatch"
                            },
                            "workspace_id": "workspace-mismatch",
                            "tab_id": "tab-mismatch",
                            "pane_id": "pane-mismatch",
                            "focused": false,
                            "launch_pending": false,
                            "interactive_ready": true,
                            "state_change_seq": 3,
                            "cwd": "/tmp/mismatch",
                            "foreground_cwd": "/tmp/mismatch",
                            "revision": 2
                        }
                    })
                } else {
                    assert_eq!(request["method"], "tab.close");
                    assert_eq!(request["params"]["tab_id"], "tab-prepared");
                    serde_json::json!({ "type": "ok" })
                };
                writer
                    .write_all(
                        format!("{}\n", serde_json::json!({"id": id, "result": result})).as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = start_prepared_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "replacement-mismatch".to_owned(),
                prepared: runtime_topology(
                    "workspace-prepared",
                    "tab-prepared",
                    "pane-prepared",
                    "terminal-prepared",
                ),
                agent_name: "yard-replacement".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Continue.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        let HerdrControlError::StartFailed {
            rollback_succeeded,
            started_runtime: Some(started_runtime),
            ..
        } = error
        else {
            panic!("expected topology mismatch with captured started runtime");
        };
        assert!(!rollback_succeeded);
        assert_eq!(started_runtime.workspace_id, "workspace-mismatch");
        assert_eq!(started_runtime.terminal_id, "terminal-mismatch");
        assert_eq!(
            started_runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("session-mismatch")
        );
    }

    #[tokio::test]
    async fn lost_start_and_failed_rollback_preserve_prepared_runtime_identity() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for expected_method in ["agent.start", "tab.close"] {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, _writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["method"], expected_method);
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_millis(100),
            ..HerdrConfig::default()
        };
        let prepared = runtime_topology(
            "workspace-prepared",
            "tab-prepared",
            "pane-prepared",
            "terminal-prepared",
        );

        let error = start_prepared_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "lost-start".to_owned(),
                prepared: prepared.clone(),
                agent_name: "yard-lost-start".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Continue.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        let HerdrControlError::StartFailed {
            rollback_succeeded,
            started_runtime: Some(captured),
            ..
        } = error
        else {
            panic!("expected failed rollback with captured prepared runtime");
        };
        assert!(!rollback_succeeded);
        assert_eq!(*captured, prepared);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn creates_workspace_and_starts_agent_in_root_pane() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let result = match step {
                    0 => {
                        assert_eq!(request["method"], "workspace.create");
                        assert_eq!(request["params"]["cwd"], "/tmp/project");
                        assert_eq!(request["params"]["label"], "Runtime API");
                        assert_eq!(request["params"]["focus"], false);
                        assert_eq!(request["params"]["env"], serde_json::json!({}));
                        serde_json::json!({
                            "type": "workspace_created",
                            "workspace": { "workspace_id": "workspace-2" },
                            "tab": {
                                "tab_id": "tab-2",
                                "workspace_id": "workspace-2",
                                "number": 1,
                                "label": "1",
                                "focused": false,
                                "pane_count": 1,
                                "agent_status": "unknown"
                            },
                            "root_pane": {
                                "pane_id": "pane-2",
                                "terminal_id": "terminal-2",
                                "workspace_id": "workspace-2",
                                "tab_id": "tab-2",
                                "focused": false,
                                "cwd": "/tmp/project",
                                "foreground_cwd": "/tmp/project",
                                "agent_status": "unknown",
                                "revision": 0
                            }
                        })
                    }
                    1 => {
                        assert_eq!(request["method"], "agent.start");
                        assert_eq!(request["params"]["pane_id"], "pane-2");
                        serde_json::json!({
                            "type": "agent_started",
                            "agent": {
                                "terminal_id": "terminal-2",
                                "name": "yard-bootstrap",
                                "agent": "codex",
                                "display_agent": "Codex",
                                "agent_status": "idle",
                                "tokens": {},
                                "agent_session": {
                                    "source": "herdr:codex",
                                    "agent": "codex",
                                    "kind": "id",
                                    "value": "session-2"
                                },
                                "workspace_id": "workspace-2",
                                "tab_id": "tab-2",
                                "pane_id": "pane-2",
                                "focused": false,
                                "launch_pending": false,
                                "interactive_ready": true,
                                "state_change_seq": 2,
                                "cwd": "/tmp/project",
                                "foreground_cwd": "/tmp/project",
                                "revision": 1
                            }
                        })
                    }
                    _ => {
                        assert_eq!(request["method"], "agent.prompt");
                        serde_json::json!({
                            "type": "agent_prompted",
                            "agent": { "agent_status": "working" }
                        })
                    }
                };
                writer
                    .write_all(
                        format!("{}\n", serde_json::json!({"id": id, "result": result})).as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let created = bootstrap_agent_at_socket(
            &config,
            &socket_path,
            BootstrapAgentRequest {
                command_id: "bootstrap-1".to_owned(),
                session: "default".to_owned(),
                workspace_label: "Runtime API".to_owned(),
                cwd: "/tmp/project".to_owned(),
                agent_name: "yard-bootstrap".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Coordinate the project.".to_owned(),
            },
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(created.runtime.workspace_id, "workspace-2");
        assert_eq!(created.runtime.tab_id.as_deref(), Some("tab-2"));
        assert_eq!(created.runtime.pane_id, "pane-2");
        assert!(created.runtime.owns_tab);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn herdr_0_8_2_workspace_control_contract_matches_protocol_20() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let fixtures: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.2/control.json"))
                .unwrap();
        let server = tokio::spawn(async move {
            for step in 0..5 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let fixture = match step {
                    0 => {
                        assert_eq!(request["method"], "workspace.create");
                        assert_eq!(request["params"]["cwd"], "/tmp/protocol-20-project");
                        assert_eq!(request["params"]["label"], "Protocol 20");
                        assert_eq!(request["params"]["focus"], false);
                        assert_eq!(request["params"]["env"], serde_json::json!({}));
                        "workspace_created"
                    }
                    1 => {
                        assert_eq!(request["method"], "agent.start");
                        assert_eq!(request["params"]["name"], "yard-protocol20");
                        assert_eq!(request["params"]["kind"], "claude");
                        assert_eq!(request["params"]["pane_id"], "w20:p1");
                        assert_eq!(
                            request["params"]["args"],
                            serde_json::json!(["--permission-mode", "auto"])
                        );
                        assert_eq!(request["params"]["timeout_ms"], 30_000);
                        "agent_started"
                    }
                    2 => {
                        assert_eq!(request["method"], "agent.prompt");
                        assert_eq!(request["params"]["target"], "w20:p1");
                        assert_eq!(
                            request["params"]["text"],
                            "Coordinate protocol 20 compatibility."
                        );
                        assert!(request["params"]["wait"].is_null());
                        "agent_prompted"
                    }
                    3 => {
                        assert_eq!(request["method"], "pane.read");
                        assert_eq!(request["params"]["pane_id"], "w20:p1");
                        assert_eq!(request["params"]["source"], "recent_unwrapped");
                        assert_eq!(request["params"]["lines"], 1_000);
                        assert_eq!(request["params"]["format"], "text");
                        assert_eq!(request["params"]["strip_ansi"], true);
                        "pane_read"
                    }
                    _ => {
                        assert_eq!(request["method"], "workspace.close");
                        assert_eq!(request["params"]["workspace_id"], "w20");
                        "ok"
                    }
                };
                let response = serde_json::json!({
                    "id": id,
                    "result": fixtures[fixture].clone()
                });
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let created = bootstrap_agent_at_socket(
            &config,
            &socket_path,
            BootstrapAgentRequest {
                command_id: "protocol-20".to_owned(),
                session: "default".to_owned(),
                workspace_label: "Protocol 20".to_owned(),
                cwd: "/tmp/protocol-20-project".to_owned(),
                agent_name: "yard-protocol20".to_owned(),
                kind: "claude".to_owned(),
                args: vec!["--permission-mode".to_owned(), "auto".to_owned()],
                prompt: "Coordinate protocol 20 compatibility.".to_owned(),
            },
        )
        .await
        .unwrap();
        let output = read_pane_at_socket(
            &config,
            &socket_path,
            ReadPaneRequest {
                request_id: "protocol-20".to_owned(),
                session: "default".to_owned(),
                pane_id: created.runtime.pane_id.clone(),
                lines: 1_000,
            },
        )
        .await
        .unwrap();
        close_workspace(
            &config,
            &socket_path,
            "protocol-20",
            &created.runtime.workspace_id,
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(created.runtime.workspace_id, "w20");
        assert_eq!(created.runtime.tab_id.as_deref(), Some("w20:t1"));
        assert_eq!(created.runtime.pane_id, "w20:p1");
        assert_eq!(
            created
                .runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("session_protocol20")
        );
        assert_eq!(output.text, "Protocol 20 control output");
        assert!(!output.truncated);
    }

    #[tokio::test]
    async fn herdr_0_8_2_tab_control_accepts_ok_close_result() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let fixtures: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.2/control.json"))
                .unwrap();
        let server = tokio::spawn(async move {
            for step in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let fixture = if step == 0 {
                    assert_eq!(request["method"], "tab.create");
                    assert_eq!(request["params"]["workspace_id"], "w20");
                    assert_eq!(request["params"]["cwd"], "/tmp/protocol-20-project");
                    assert_eq!(request["params"]["label"], "Protocol 20 worker");
                    "tab_created"
                } else {
                    assert_eq!(request["method"], "tab.close");
                    assert_eq!(request["params"]["tab_id"], "w20:t2");
                    "ok"
                };
                let response = serde_json::json!({
                    "id": id,
                    "result": fixtures[fixture].clone()
                });
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let prepared = prepare_agent_at_socket(
            &config,
            &socket_path,
            PrepareAgentRequest {
                command_id: "protocol-20-tab".to_owned(),
                session: "default".to_owned(),
                workspace_id: "w20".to_owned(),
                cwd: "/tmp/protocol-20-project".to_owned(),
                tab_label: "Protocol 20 worker".to_owned(),
            },
        )
        .await
        .unwrap();
        close_tab(
            &config,
            &socket_path,
            "protocol-20-tab",
            prepared.runtime.tab_id.as_deref().unwrap(),
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(prepared.runtime.tab_id.as_deref(), Some("w20:t2"));
        assert_eq!(prepared.runtime.pane_id, "w20:p2");
        assert_eq!(prepared.runtime.terminal_id, "term_protocol20_tab");
    }

    #[tokio::test]
    async fn rolls_back_created_workspace_when_agent_start_fails() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let response = match step {
                    0 => serde_json::json!({
                        "id": id,
                        "result": {
                            "type": "workspace_created",
                            "workspace": { "workspace_id": "workspace-2" },
                            "tab": {
                                "tab_id": "tab-2",
                                "workspace_id": "workspace-2",
                                "number": 1,
                                "label": "1",
                                "focused": false,
                                "pane_count": 1,
                                "agent_status": "unknown"
                            },
                            "root_pane": {
                                "pane_id": "pane-2",
                                "terminal_id": "terminal-2",
                                "workspace_id": "workspace-2",
                                "tab_id": "tab-2",
                                "focused": false,
                                "cwd": "/tmp/project",
                                "foreground_cwd": "/tmp/project",
                                "agent_status": "unknown",
                                "revision": 0
                            }
                        }
                    }),
                    1 => {
                        assert_eq!(request["method"], "agent.start");
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "agent_start_failed",
                                "message": "provider unavailable"
                            }
                        })
                    }
                    _ => {
                        assert_eq!(request["method"], "workspace.close");
                        assert_eq!(request["params"]["workspace_id"], "workspace-2");
                        serde_json::json!({
                            "id": id,
                            "result": { "type": "ok" }
                        })
                    }
                };
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = bootstrap_agent_at_socket(
            &config,
            &socket_path,
            BootstrapAgentRequest {
                command_id: "bootstrap-1".to_owned(),
                session: "default".to_owned(),
                workspace_label: "Runtime API".to_owned(),
                cwd: "/tmp/project".to_owned(),
                agent_name: "yard-bootstrap".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Coordinate the project.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::StartFailed {
                rollback_succeeded: true,
                ..
            }
        ));
    }

    #[test]
    fn classifies_runtime_creation_transport_ambiguity() {
        assert!(runtime_creation_outcome_ambiguous(
            &HerdrError::SocketTimeout
        ));
        assert!(!runtime_creation_outcome_ambiguous(&HerdrError::Api {
            code: "agent_name_taken".to_owned(),
            message: "agent name is already in use".to_owned(),
        }));
    }

    #[tokio::test]
    async fn lost_tab_create_response_is_reported_as_ambiguous_preparation() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, _writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "tab.create");
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = prepare_agent_at_socket(
            &config,
            &socket_path,
            PrepareAgentRequest {
                command_id: "replacement-lost-response".to_owned(),
                session: "default".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                cwd: "/tmp/project".to_owned(),
                tab_label: "Yard replacement [replacement-lost-response]".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::PrepareFailed {
                ambiguous: true,
                ..
            }
        ));
    }

    async fn assert_wrong_workspace_stops_before_followup(expected_followup: &'static str) {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "tab.create");
            let id = request["id"].as_str().unwrap();
            let result = serde_json::json!({
                "type": "tab_created",
                "tab": {
                    "tab_id": "foreign-tab",
                    "workspace_id": "foreign-workspace",
                    "number": 1,
                    "label": "Foreign",
                    "focused": false,
                    "pane_count": 1,
                    "agent_status": "unknown"
                },
                "root_pane": {
                    "pane_id": "foreign-pane",
                    "terminal_id": "foreign-terminal",
                    "workspace_id": "foreign-workspace",
                    "tab_id": "foreign-tab",
                    "focused": false,
                    "cwd": "/tmp/foreign",
                    "foreground_cwd": "/tmp/foreign",
                    "label": null,
                    "agent": null,
                    "display_agent": null,
                    "agent_status": "unknown",
                    "tokens": {},
                    "agent_session": null,
                    "revision": 1
                }
            });
            writer
                .write_all(
                    format!("{}\n", serde_json::json!({"id": id, "result": result})).as_bytes(),
                )
                .await
                .unwrap();
            if let Ok(Ok((stream, _))) =
                tokio::time::timeout(Duration::from_millis(100), listener.accept()).await
            {
                let (reader, _) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                panic!(
                    "wrong-workspace response reached {expected_followup} via {}",
                    request["method"]
                );
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = provision_agent_at_socket(
            &config,
            &socket_path,
            ProvisionAgentRequest {
                command_id: format!("wrong-workspace-{expected_followup}"),
                session: "default".to_owned(),
                workspace_id: "requested-workspace".to_owned(),
                cwd: "/tmp/requested".to_owned(),
                tab_label: "Requested".to_owned(),
                agent_name: "yard-requested".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Stay in the requested workspace.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::PrepareFailed {
                ambiguous: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn wrong_workspace_prepare_never_reaches_start_or_prompt_success_path() {
        assert_wrong_workspace_stops_before_followup("start/prompt").await;
    }

    #[tokio::test]
    async fn wrong_workspace_prepare_never_reaches_start_failure_rollback_path() {
        assert_wrong_workspace_stops_before_followup("start/rollback").await;
    }

    #[tokio::test]
    async fn workspace_bootstrap_rejects_mismatched_root_tab_before_any_followup() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "workspace.create");
            let id = request["id"].as_str().unwrap();
            let result = serde_json::json!({
                "type": "workspace_created",
                "workspace": { "workspace_id": "workspace-ancestry" },
                "tab": {
                    "tab_id": "tab-created",
                    "workspace_id": "workspace-ancestry",
                    "number": 1,
                    "label": "Created",
                    "focused": false,
                    "pane_count": 1,
                    "agent_status": "unknown"
                },
                "root_pane": {
                    "pane_id": "pane-created",
                    "terminal_id": "terminal-created",
                    "workspace_id": "workspace-ancestry",
                    "tab_id": "foreign-tab",
                    "focused": false,
                    "cwd": "/tmp/project",
                    "foreground_cwd": "/tmp/project",
                    "label": null,
                    "agent": null,
                    "display_agent": null,
                    "agent_status": "unknown",
                    "tokens": {},
                    "agent_session": null,
                    "revision": 1
                }
            });
            writer
                .write_all(
                    format!("{}\n", serde_json::json!({"id": id, "result": result})).as_bytes(),
                )
                .await
                .unwrap();
            if let Ok(Ok((stream, _))) =
                tokio::time::timeout(Duration::from_millis(100), listener.accept()).await
            {
                let (reader, _) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                panic!(
                    "inconsistent workspace ancestry reached follow-up method {}",
                    request["method"]
                );
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let error = bootstrap_agent_at_socket(
            &config,
            &socket_path,
            BootstrapAgentRequest {
                command_id: "workspace-ancestry-mismatch".to_owned(),
                session: "default".to_owned(),
                workspace_label: "Project".to_owned(),
                cwd: "/tmp/project".to_owned(),
                agent_name: "yard-project".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Coordinate the project.".to_owned(),
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrControlError::WorkspaceCreateFailed {
                ambiguous: true,
                ..
            }
        ));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn prepares_terminal_before_starting_and_prompting_agent() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let result = match step {
                    0 => {
                        assert_eq!(request["method"], "tab.create");
                        assert_eq!(request["params"]["workspace_id"], "workspace-2");
                        serde_json::json!({
                            "type": "tab_created",
                            "tab": {
                                "tab_id": "tab-2",
                                "workspace_id": "workspace-2",
                                "number": 2,
                                "label": "Implementer",
                                "focused": false,
                                "pane_count": 1,
                                "agent_status": "unknown"
                            },
                            "root_pane": {
                                "pane_id": "pane-2",
                                "terminal_id": "terminal-2",
                                "workspace_id": "workspace-2",
                                "tab_id": "tab-2",
                                "focused": false,
                                "cwd": "/tmp/target",
                                "foreground_cwd": "/tmp/target",
                                "label": null,
                                "agent": null,
                                "display_agent": null,
                                "agent_status": "unknown",
                                "tokens": {},
                                "agent_session": null,
                                "revision": 1
                            }
                        })
                    }
                    1 => {
                        assert_eq!(request["method"], "agent.start");
                        assert_eq!(request["params"]["pane_id"], "pane-2");
                        serde_json::json!({
                            "type": "agent_started",
                            "agent": {
                                "terminal_id": "terminal-2",
                                "name": "yard-handoff",
                                "agent": "codex",
                                "display_agent": "Codex",
                                "agent_status": "idle",
                                "tokens": {},
                                "agent_session": {
                                    "source": "herdr:codex",
                                    "agent": "codex",
                                    "kind": "id",
                                    "value": "session-2"
                                },
                                "workspace_id": "workspace-2",
                                "tab_id": "tab-2",
                                "pane_id": "pane-2",
                                "focused": false,
                                "launch_pending": false,
                                "interactive_ready": true,
                                "state_change_seq": 3,
                                "cwd": "/tmp/target",
                                "foreground_cwd": "/tmp/target",
                                "revision": 2
                            }
                        })
                    }
                    _ => {
                        assert_eq!(request["method"], "agent.prompt");
                        assert_eq!(request["params"]["target"], "pane-2");
                        serde_json::json!({
                            "type": "agent_prompted",
                            "agent": { "agent_status": "working" }
                        })
                    }
                };
                writer
                    .write_all(
                        format!("{}\n", serde_json::json!({"id": id, "result": result})).as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let prepared = prepare_agent_at_socket(
            &config,
            &socket_path,
            PrepareAgentRequest {
                command_id: "handoff-1".to_owned(),
                session: "default".to_owned(),
                workspace_id: "workspace-2".to_owned(),
                cwd: "/tmp/target".to_owned(),
                tab_label: "Implementer".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(prepared.runtime.terminal_id, "terminal-2");
        assert!(prepared.runtime.provider_session.is_none());

        let started = start_prepared_agent_at_socket(
            &config,
            &socket_path,
            StartPreparedAgentRequest {
                command_id: "handoff-1".to_owned(),
                prepared: prepared.runtime,
                agent_name: "yard-handoff".to_owned(),
                kind: "codex".to_owned(),
                args: Vec::new(),
                prompt: "Continue in the target project.".to_owned(),
            },
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(started.runtime.terminal_id, "terminal-2");
        assert_eq!(
            started
                .runtime
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("session-2")
        );
    }

    #[tokio::test]
    async fn prompts_existing_agent_without_waiting_for_turn_completion() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["id"], "yard:command-1:prompt");
            assert_eq!(request["method"], "agent.prompt");
            assert_eq!(request["params"]["target"], "pane-1");
            assert_eq!(request["params"]["text"], "Continue.");
            assert!(request["params"]["wait"].is_null());
            writer
                .write_all(
                    b"{\"id\":\"yard:command-1:prompt\",\"result\":{\"type\":\"agent_prompted\",\"agent\":{\"agent_status\":\"working\"}}}\n",
                )
                .await
                .unwrap();
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let prompted = prompt_agent_at_socket(
            &config,
            &socket_path,
            PromptAgentRequest {
                command_id: "command-1".to_owned(),
                session: "default".to_owned(),
                pane_id: "pane-1".to_owned(),
                text: "Continue.".to_owned(),
            },
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(prompted.status, "working");
    }

    #[tokio::test]
    async fn reads_bounded_unwrapped_plain_text_output() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["id"], "yard:request-1:read");
            assert_eq!(request["method"], "pane.read");
            assert_eq!(request["params"]["pane_id"], "pane-1");
            assert_eq!(request["params"]["source"], "recent_unwrapped");
            assert_eq!(request["params"]["lines"], 120);
            assert_eq!(request["params"]["format"], "text");
            assert_eq!(request["params"]["strip_ansi"], true);
            writer
                .write_all(
                    b"{\"id\":\"yard:request-1:read\",\"result\":{\"type\":\"pane_read\",\"read\":{\"pane_id\":\"pane-1\",\"workspace_id\":\"workspace-1\",\"tab_id\":\"tab-1\",\"source\":\"recent_unwrapped\",\"format\":\"text\",\"text\":\"tests pass\",\"revision\":0,\"truncated\":false}}}\n",
                )
                .await
                .unwrap();
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let output = read_pane_at_socket(
            &config,
            &socket_path,
            ReadPaneRequest {
                request_id: "request-1".to_owned(),
                session: "default".to_owned(),
                pane_id: "pane-1".to_owned(),
                lines: 120,
            },
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(output.text, "tests pass");
        assert_eq!(output.revision, 0);
        assert!(!output.truncated);
    }

    #[tokio::test]
    async fn reads_extended_history_through_an_uncapped_selection() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..4 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_str().unwrap();
                let result = extended_history_result(step, &request);
                let response = serde_json::json!({
                    "id": id,
                    "result": result
                });
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let output = read_pane_at_socket(
            &config,
            &socket_path,
            ReadPaneRequest {
                request_id: "request-extended".to_owned(),
                session: "default".to_owned(),
                pane_id: "pane-1".to_owned(),
                lines: 10_000,
            },
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(output.text, "selected 10000 lines");
        assert!(output.truncated);
    }
}

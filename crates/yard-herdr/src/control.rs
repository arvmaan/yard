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
const AGENT_PROMPT_READY_TIMEOUT: Duration = Duration::from_secs(5);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetireRuntimeRequest {
    pub cleanup_id: String,
    pub session: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub owns_tab: bool,
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
    #[error("Herdr agent start failed: {start}; runtime rollback: {rollback}")]
    StartFailed {
        start: HerdrError,
        rollback: String,
        rollback_succeeded: bool,
    },
    #[error("Herdr created the worker but initial prompt delivery failed: {source}")]
    PromptDeliveryFailed {
        runtime: Box<WorkerRuntimeBinding>,
        source: HerdrError,
    },
}

pub(crate) async fn provision_agent(
    config: &HerdrConfig,
    request: ProvisionAgentRequest,
) -> Result<ProvisionedAgent, HerdrControlError> {
    let prepared = prepare_agent(
        config,
        PrepareAgentRequest {
            command_id: request.command_id.clone(),
            session: request.session,
            workspace_id: request.workspace_id,
            cwd: request.cwd,
            tab_label: request.tab_label,
        },
    )
    .await?;
    start_prepared_agent(
        config,
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
        ambiguous: workspace_create_outcome_ambiguous(&source),
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
    {
        return Err(HerdrControlError::WorkspaceCreateFailed {
            source: HerdrError::InvalidTopology(
                "created workspace, tab, and root pane have different workspace IDs".to_owned(),
            ),
            ambiguous: true,
        });
    }

    let prepared = prepared_runtime_binding(&request.session, &created.tab, &created.root_pane);
    start_agent_at_socket(
        config,
        socket_path,
        StartPreparedAgentRequest {
            command_id: request.command_id,
            prepared,
            agent_name: request.agent_name,
            kind: request.kind,
            args: request.args,
            prompt: request.prompt,
        },
        StartRollback::Workspace(created.workspace.workspace_id),
    )
    .await
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
    .await?;
    expect_result_type(&tab_result, "tab_created")?;
    let tab: TabCreated = serde_json::from_value(tab_result).map_err(HerdrError::CommandDecode)?;
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
    Tab(String),
    Workspace(String),
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
            return Err(start_failure_after_rollback(
                config,
                socket_path,
                &request.command_id,
                rollback,
                start,
                true,
            )
            .await);
        }
    };
    let runtime =
        validate_started_topology(config, socket_path, &request, rollback, started).await?;

    let prompt_result = prompt_when_ready(
        config,
        socket_path,
        &request.command_id,
        &runtime.pane_id,
        &request.prompt,
    )
    .await;
    match prompt_result {
        Ok(result) => match expect_result_type(&result, "agent_prompted") {
            Ok(()) => Ok(ProvisionedAgent { runtime }),
            Err(source) => Err(HerdrControlError::PromptDeliveryFailed {
                runtime: Box::new(runtime),
                source,
            }),
        },
        Err(source) if is_definitely_not_submitted(&source) => {
            match send_prompt_to_pane(
                config,
                socket_path,
                &request.command_id,
                &runtime.pane_id,
                &request.prompt,
            )
            .await
            {
                Ok(()) => Ok(ProvisionedAgent { runtime }),
                Err(source) => Err(HerdrControlError::PromptDeliveryFailed {
                    runtime: Box::new(runtime),
                    source,
                }),
            }
        }
        Err(source) => Err(HerdrControlError::PromptDeliveryFailed {
            runtime: Box::new(runtime),
            source,
        }),
    }
}

async fn validate_started_topology(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: &StartPreparedAgentRequest,
    rollback: StartRollback,
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
        )
        .await);
    }
    Ok(runtime)
}

async fn start_failure_after_rollback(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    rollback: StartRollback,
    start: HerdrError,
    rollback_covers_started_runtime: bool,
) -> HerdrControlError {
    let rollback_result = rollback_start(config, socket_path, command_id, rollback).await;
    let rollback_succeeded = rollback_covers_started_runtime && rollback_result.is_ok();
    let rollback = rollback_result.map_or_else(
        |error| error.to_string(),
        |()| {
            if rollback_covers_started_runtime {
                "succeeded".to_owned()
            } else {
                "prepared topology closed; mismatched started runtime remains unverified".to_owned()
            }
        },
    );
    HerdrControlError::StartFailed {
        start,
        rollback,
        rollback_succeeded,
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
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{}:read", request.request_id),
        "pane.read",
        json!({
            "pane_id": request.pane_id,
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
    Ok(result.read)
}

pub(crate) async fn retire_runtime(
    config: &HerdrConfig,
    request: RetireRuntimeRequest,
) -> Result<(), HerdrError> {
    validate_retirement_request(&request)?;
    let session = running_session(config, &request.session).await?;
    retire_runtime_at_socket(config, &session.socket_path, request).await
}

async fn retire_runtime_at_socket(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    request: RetireRuntimeRequest,
) -> Result<(), HerdrError> {
    let (request_suffix, method, params, expected_result, absence_code) = if request.owns_tab {
        let tab_id = retirement_tab_id(&request)?;
        (
            "retire-tab",
            "tab.close",
            json!({ "tab_id": tab_id }),
            "tab_closed",
            "tab_not_found",
        )
    } else {
        (
            "retire-pane",
            "pane.close",
            json!({ "pane_id": request.pane_id }),
            "pane_closed",
            "pane_not_found",
        )
    };
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{}:{request_suffix}", request.cleanup_id),
        method,
        params,
        config.request_timeout,
    )
    .await;
    match result {
        Ok(result) => expect_result_type(&result, expected_result),
        Err(HerdrError::Api { code, .. }) if code == absence_code => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_retirement_request(request: &RetireRuntimeRequest) -> Result<(), HerdrError> {
    if request.owns_tab {
        retirement_tab_id(request)?;
    }
    Ok(())
}

fn retirement_tab_id(request: &RetireRuntimeRequest) -> Result<&str, HerdrError> {
    request
        .tab_id
        .as_deref()
        .ok_or_else(|| HerdrError::UnexpectedResult {
            expected: "retirement tab_id",
            actual: "missing".to_owned(),
        })
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

async fn prompt_when_ready(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    pane_id: &str,
    prompt: &str,
) -> Result<serde_json::Value, HerdrError> {
    let deadline = Instant::now() + AGENT_PROMPT_READY_TIMEOUT;
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
                let retryable = is_definitely_not_submitted(&error);
                if !retryable || Instant::now() >= deadline {
                    return Err(error);
                }
            }
        }
        attempt = attempt.saturating_add(1);
        sleep(Duration::from_millis(100)).await;
    }
}

async fn send_prompt_to_pane(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    pane_id: &str,
    prompt: &str,
) -> Result<(), HerdrError> {
    let result = request_command(
        config,
        socket_path,
        &format!("yard:{command_id}:pane-prompt"),
        "pane.send_input",
        json!({
            "pane_id": pane_id,
            "text": prompt,
            "keys": ["enter"]
        }),
        config.request_timeout,
    )
    .await?;
    expect_result_type(&result, "ok")
}

fn is_definitely_not_submitted(error: &HerdrError) -> bool {
    matches!(
        error,
        HerdrError::Api { code, .. }
            if code == "agent_not_ready" || code == "agent_not_found"
    )
}

fn workspace_create_outcome_ambiguous(error: &HerdrError) -> bool {
    !matches!(
        error,
        HerdrError::SocketConnect { .. } | HerdrError::Api { .. }
    )
}

async fn rollback_start(
    config: &HerdrConfig,
    socket_path: &std::path::Path,
    command_id: &str,
    rollback: StartRollback,
) -> Result<(), HerdrError> {
    match rollback {
        StartRollback::Tab(tab_id) => close_tab(config, socket_path, command_id, &tab_id).await,
        StartRollback::Workspace(workspace_id) => {
            close_workspace(config, socket_path, command_id, &workspace_id).await
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
    expect_result_type(&result, "tab_closed")
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    use super::{
        BootstrapAgentRequest, HerdrControlError, PrepareAgentRequest, PromptAgentRequest,
        ReadPaneRequest, RetireRuntimeRequest, StartPreparedAgentRequest,
        bootstrap_agent_at_socket, prepare_agent_at_socket, prompt_agent_at_socket,
        read_pane_at_socket, retained_prepared_topology, retire_runtime, retire_runtime_at_socket,
        start_prepared_agent_at_socket, workspace_create_outcome_ambiguous,
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

    #[test]
    fn started_agent_must_retain_the_prepared_topology() {
        let prepared = runtime_topology("workspace-1", "tab-1", "pane-1", "terminal-1");

        assert!(retained_prepared_topology(&prepared, &prepared));
        assert!(!retained_prepared_topology(
            &prepared,
            &runtime_topology("workspace-2", "tab-1", "pane-1", "terminal-1"),
        ));
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
    fn classifies_workspace_creation_transport_ambiguity() {
        assert!(workspace_create_outcome_ambiguous(
            &HerdrError::SocketTimeout
        ));
        assert!(!workspace_create_outcome_ambiguous(&HerdrError::Api {
            code: "workspace_create_failed".to_owned(),
            message: "invalid directory".to_owned(),
        }));
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
    async fn selects_destructive_close_method_from_runtime_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for step in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let (id, method, param, result_type) = if step == 0 {
                    (
                        "yard:cleanup-1:retire-tab",
                        "tab.close",
                        ("tab_id", "tab-1"),
                        "tab_closed",
                    )
                } else {
                    (
                        "yard:cleanup-2:retire-pane",
                        "pane.close",
                        ("pane_id", "pane-2"),
                        "pane_closed",
                    )
                };
                assert_eq!(request["id"], id);
                assert_eq!(request["method"], method);
                assert_eq!(request["params"][param.0], param.1);
                writer
                    .write_all(
                        format!(
                            "{}\n",
                            serde_json::json!({
                                "id": id,
                                "result": { "type": result_type }
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        retire_runtime_at_socket(
            &config,
            &socket_path,
            RetireRuntimeRequest {
                cleanup_id: "cleanup-1".to_owned(),
                session: "default".to_owned(),
                tab_id: Some("tab-1".to_owned()),
                pane_id: "pane-1".to_owned(),
                owns_tab: true,
            },
        )
        .await
        .unwrap();
        retire_runtime_at_socket(
            &config,
            &socket_path,
            RetireRuntimeRequest {
                cleanup_id: "cleanup-2".to_owned(),
                session: "default".to_owned(),
                tab_id: Some("tab-2".to_owned()),
                pane_id: "pane-2".to_owned(),
                owns_tab: false,
            },
        )
        .await
        .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn treats_matching_absence_as_successful_retirement() {
        let temp = tempfile::tempdir().unwrap();
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
                let code = if step == 0 {
                    assert_eq!(request["method"], "tab.close");
                    "tab_not_found"
                } else {
                    assert_eq!(request["method"], "pane.close");
                    "pane_not_found"
                };
                writer
                    .write_all(
                        format!(
                            "{}\n",
                            serde_json::json!({
                                "id": id,
                                "error": {
                                    "code": code,
                                    "message": "already absent"
                                }
                            })
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        for (cleanup_id, tab_id, pane_id, owns_tab) in [
            ("cleanup-tab", Some("tab-1"), "pane-1", true),
            ("cleanup-pane", None, "pane-2", false),
        ] {
            retire_runtime_at_socket(
                &config,
                &socket_path,
                RetireRuntimeRequest {
                    cleanup_id: cleanup_id.to_owned(),
                    session: "default".to_owned(),
                    tab_id: tab_id.map(str::to_owned),
                    pane_id: pane_id.to_owned(),
                    owns_tab,
                },
            )
            .await
            .unwrap();
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_owned_runtime_without_tab_before_discovery_or_socket_io() {
        let config = HerdrConfig {
            binary: "herdr-must-not-run".into(),
            ..HerdrConfig::default()
        };

        let error = retire_runtime(
            &config,
            RetireRuntimeRequest {
                cleanup_id: "cleanup-1".to_owned(),
                session: "default".to_owned(),
                tab_id: None,
                pane_id: "pane-1".to_owned(),
                owns_tab: true,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            HerdrError::UnexpectedResult {
                expected: "retirement tab_id",
                actual
            } if actual == "missing"
        ));
    }

    #[tokio::test]
    async fn does_not_swallow_unrelated_close_errors() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            let id = request["id"].as_str().unwrap();
            writer
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "workspace_not_found",
                                "message": "unrelated failure"
                            }
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let error = retire_runtime_at_socket(
            &HerdrConfig::default(),
            &socket_path,
            RetireRuntimeRequest {
                cleanup_id: "cleanup-1".to_owned(),
                session: "default".to_owned(),
                tab_id: Some("tab-1".to_owned()),
                pane_id: "pane-1".to_owned(),
                owns_tab: true,
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrError::Api { code, .. } if code == "workspace_not_found"
        ));
    }

    #[tokio::test]
    async fn validates_destructive_close_result_type() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            let id = request["id"].as_str().unwrap();
            writer
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "id": id,
                            "result": { "type": "pane_closed" }
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let error = retire_runtime_at_socket(
            &HerdrConfig::default(),
            &socket_path,
            RetireRuntimeRequest {
                cleanup_id: "cleanup-1".to_owned(),
                session: "default".to_owned(),
                tab_id: Some("tab-1".to_owned()),
                pane_id: "pane-1".to_owned(),
                owns_tab: true,
            },
        )
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            HerdrError::UnexpectedResult {
                expected: "tab_closed",
                actual
            } if actual == "pane_closed"
        ));
    }
}

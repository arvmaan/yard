use std::collections::{HashMap, HashSet};

use yard_domain::{
    FocusObservation, ObservedStatus, ObservedWorker, PaneObservation, ProviderSessionRef,
    RuntimeInventory, TabObservation, WorkspaceObservation, WorktreeObservation,
};

use crate::{
    HerdrConfig, HerdrError,
    wire::{AgentSession, SessionSnapshot},
};

pub(crate) fn normalize(
    snapshot: SessionSnapshot,
    session_name: &str,
    observed_at_unix_ms: u64,
    config: &HerdrConfig,
) -> Result<RuntimeInventory, HerdrError> {
    if snapshot.protocol != config.expected_protocol {
        return Err(HerdrError::ProtocolMismatch {
            expected: config.expected_protocol,
            actual: snapshot.protocol,
        });
    }

    validate_topology(&snapshot)?;

    Ok(RuntimeInventory {
        adapter: "herdr".to_owned(),
        session: session_name.to_owned(),
        runtime_version: snapshot.version,
        protocol: snapshot.protocol,
        observed_at_unix_ms,
        focus: FocusObservation {
            workspace_id: snapshot.focused_workspace_id,
            tab_id: snapshot.focused_tab_id,
            pane_id: snapshot.focused_pane_id,
        },
        workspaces: snapshot
            .workspaces
            .into_iter()
            .map(|workspace| WorkspaceObservation {
                runtime_id: workspace.workspace_id,
                order: workspace.number,
                label: workspace.label,
                focused: workspace.focused,
                active_tab_id: workspace.active_tab_id,
                pane_count: workspace.pane_count,
                tab_count: workspace.tab_count,
                status: status(&workspace.agent_status),
                tokens: workspace.tokens,
                worktree: workspace.worktree.map(|worktree| WorktreeObservation {
                    repository_key: worktree.repo_key,
                    repository_name: worktree.repo_name,
                    repository_root: worktree.repo_root,
                    checkout_path: worktree.checkout_path,
                    is_linked: worktree.is_linked_worktree,
                }),
            })
            .collect(),
        tabs: snapshot
            .tabs
            .into_iter()
            .map(|tab| TabObservation {
                runtime_id: tab.tab_id,
                workspace_id: tab.workspace_id,
                order: tab.number,
                label: tab.label,
                focused: tab.focused,
                pane_count: tab.pane_count,
                status: status(&tab.agent_status),
            })
            .collect(),
        panes: snapshot
            .panes
            .into_iter()
            .map(|pane| PaneObservation {
                runtime_id: pane.pane_id,
                terminal_id: pane.terminal_id,
                workspace_id: pane.workspace_id,
                tab_id: pane.tab_id,
                focused: pane.focused,
                cwd: pane.cwd,
                foreground_cwd: pane.foreground_cwd,
                label: pane.label,
                provider: pane.agent,
                display_provider: pane.display_agent,
                status: status(&pane.agent_status),
                tokens: pane.tokens,
                provider_session: pane.agent_session.map(provider_session),
                revision: pane.revision,
            })
            .collect(),
        workers: snapshot
            .agents
            .into_iter()
            .map(|agent| ObservedWorker {
                runtime_id: agent.terminal_id.clone(),
                terminal_id: agent.terminal_id,
                workspace_id: agent.workspace_id,
                tab_id: agent.tab_id,
                pane_id: agent.pane_id,
                name: agent.name,
                provider: agent.agent,
                display_provider: agent.display_agent,
                status: status(&agent.agent_status),
                focused: agent.focused,
                launch_pending: agent.launch_pending,
                interactive_ready: agent.interactive_ready,
                state_change_sequence: agent.state_change_seq,
                cwd: agent.cwd,
                foreground_cwd: agent.foreground_cwd,
                tokens: agent.tokens,
                provider_session: agent.agent_session.map(provider_session),
                revision: agent.revision,
            })
            .collect(),
        child_agents: Vec::new(),
    })
}

fn provider_session(session: AgentSession) -> ProviderSessionRef {
    ProviderSessionRef {
        source: session.source,
        provider: session.agent,
        kind: session.kind,
        value: session.value,
    }
}

pub(crate) fn status(value: &str) -> ObservedStatus {
    match value {
        "idle" => ObservedStatus::Idle,
        "working" => ObservedStatus::Working,
        "blocked" => ObservedStatus::Blocked,
        "done" => ObservedStatus::Done,
        _ => ObservedStatus::Unknown,
    }
}

fn validate_topology(snapshot: &SessionSnapshot) -> Result<(), HerdrError> {
    let workspace_ids = unique(
        "workspace",
        snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.as_str()),
    )?;
    let tab_ids = unique("tab", snapshot.tabs.iter().map(|tab| tab.tab_id.as_str()))?;
    let pane_ids = unique(
        "pane",
        snapshot.panes.iter().map(|pane| pane.pane_id.as_str()),
    )?;
    unique(
        "terminal",
        snapshot.panes.iter().map(|pane| pane.terminal_id.as_str()),
    )?;

    let tabs: HashMap<_, _> = snapshot
        .tabs
        .iter()
        .map(|tab| (tab.tab_id.as_str(), tab))
        .collect();
    let panes: HashMap<_, _> = snapshot
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane))
        .collect();

    for workspace in &snapshot.workspaces {
        let active_tab = tabs.get(workspace.active_tab_id.as_str()).ok_or_else(|| {
            invalid(format!(
                "workspace '{}' references missing active tab '{}'",
                workspace.workspace_id, workspace.active_tab_id
            ))
        })?;
        if active_tab.workspace_id != workspace.workspace_id {
            return Err(invalid(format!(
                "workspace '{}' references tab '{}' owned by '{}'",
                workspace.workspace_id, workspace.active_tab_id, active_tab.workspace_id
            )));
        }
    }

    for tab in &snapshot.tabs {
        if !workspace_ids.contains(tab.workspace_id.as_str()) {
            return Err(invalid(format!(
                "tab '{}' references missing workspace '{}'",
                tab.tab_id, tab.workspace_id
            )));
        }
    }

    for pane in &snapshot.panes {
        if !workspace_ids.contains(pane.workspace_id.as_str()) {
            return Err(invalid(format!(
                "pane '{}' references missing workspace '{}'",
                pane.pane_id, pane.workspace_id
            )));
        }
        let tab = tabs.get(pane.tab_id.as_str()).ok_or_else(|| {
            invalid(format!(
                "pane '{}' references missing tab '{}'",
                pane.pane_id, pane.tab_id
            ))
        })?;
        if tab.workspace_id != pane.workspace_id {
            return Err(invalid(format!(
                "pane '{}' and tab '{}' disagree on workspace",
                pane.pane_id, pane.tab_id
            )));
        }
    }

    for agent in &snapshot.agents {
        let pane = panes.get(agent.pane_id.as_str()).ok_or_else(|| {
            invalid(format!(
                "agent terminal '{}' references missing pane '{}'",
                agent.terminal_id, agent.pane_id
            ))
        })?;
        if pane.workspace_id != agent.workspace_id
            || pane.tab_id != agent.tab_id
            || pane.terminal_id != agent.terminal_id
        {
            return Err(invalid(format!(
                "agent terminal '{}' ancestry does not match pane '{}'",
                agent.terminal_id, agent.pane_id
            )));
        }
    }

    validate_focus(
        "workspace",
        snapshot.focused_workspace_id.as_deref(),
        &workspace_ids,
    )?;
    validate_focus("tab", snapshot.focused_tab_id.as_deref(), &tab_ids)?;
    validate_focus("pane", snapshot.focused_pane_id.as_deref(), &pane_ids)
}

fn unique<'a>(
    entity: &str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<HashSet<&'a str>, HerdrError> {
    let mut unique = HashSet::new();
    for id in ids {
        if !unique.insert(id) {
            return Err(invalid(format!("duplicate {entity} ID '{id}'")));
        }
    }
    Ok(unique)
}

fn validate_focus(entity: &str, id: Option<&str>, ids: &HashSet<&str>) -> Result<(), HerdrError> {
    if let Some(id) = id
        && !ids.contains(id)
    {
        return Err(invalid(format!("focused {entity} '{id}' is missing")));
    }
    Ok(())
}

fn invalid(message: String) -> HerdrError {
    HerdrError::InvalidTopology(message)
}

#[cfg(test)]
mod tests {
    use crate::{HerdrConfig, HerdrError, socket::decode_snapshot};

    use super::normalize;

    #[test]
    fn normalizes_scrubbed_snapshot() {
        let snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json")).unwrap();
        let inventory = normalize(
            snapshot,
            "default",
            1_786_400_000_000,
            &HerdrConfig::default(),
        )
        .unwrap();

        assert_eq!(inventory.session, "default");
        assert_eq!(inventory.protocol, 19);
        assert_eq!(inventory.workspaces[0].runtime_id, "w1");
        assert_eq!(inventory.workers[0].provider.as_deref(), Some("codex"));
        assert_eq!(inventory.workers[0].pane_id, "w1:p1");
    }

    #[test]
    fn rejects_protocol_mismatch() {
        let mut snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json")).unwrap();
        snapshot.protocol = 20;

        let error = normalize(snapshot, "default", 1, &HerdrConfig::default()).unwrap_err();

        assert!(matches!(
            error,
            HerdrError::ProtocolMismatch {
                expected: 19,
                actual: 20
            }
        ));
    }

    #[test]
    fn rejects_agent_with_orphaned_pane() {
        let mut snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json")).unwrap();
        snapshot.agents[0].pane_id = "w1:p-missing".to_owned();

        let error = normalize(snapshot, "default", 1, &HerdrConfig::default()).unwrap_err();

        assert!(matches!(error, HerdrError::InvalidTopology(_)));
        assert!(error.to_string().contains("missing pane"));
    }
}

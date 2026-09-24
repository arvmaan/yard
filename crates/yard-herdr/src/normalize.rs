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
    validate_protocol(&snapshot, config)?;
    validate_topology(&snapshot)?;
    Ok(project(snapshot, session_name, observed_at_unix_ms))
}

pub(crate) fn normalize_fleet(
    snapshot: SessionSnapshot,
    session_name: &str,
    observed_at_unix_ms: u64,
    config: &HerdrConfig,
) -> Result<RuntimeInventory, HerdrError> {
    validate_protocol(&snapshot, config)?;
    Ok(project(snapshot, session_name, observed_at_unix_ms))
}

fn validate_protocol(snapshot: &SessionSnapshot, config: &HerdrConfig) -> Result<(), HerdrError> {
    if config.supported_protocols.contains(&snapshot.protocol) {
        Ok(())
    } else {
        Err(HerdrError::ProtocolMismatch {
            minimum: *config.supported_protocols.start(),
            maximum: *config.supported_protocols.end(),
            actual: snapshot.protocol,
        })
    }
}

fn project(
    snapshot: SessionSnapshot,
    session_name: &str,
    observed_at_unix_ms: u64,
) -> RuntimeInventory {
    RuntimeInventory {
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
                pane_instance_id: pane.pane_instance_id,
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
                pane_instance_id: agent.pane_instance_id,
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
    }
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
    use crate::{
        HerdrConfig, HerdrError,
        config::{MAX_HERDR_PROTOCOL, MIN_HERDR_PROTOCOL},
        socket::decode_snapshot,
    };
    use yard_domain::ObservedStatus;

    use super::{normalize, normalize_fleet};

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
    fn normalizes_herdr_0_8_2_protocol_20_snapshot() {
        let snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.2/snapshot.json")).unwrap();

        let inventory = normalize(
            snapshot,
            "default",
            1_788_500_000_000,
            &HerdrConfig::default(),
        )
        .unwrap();

        assert_eq!(inventory.runtime_version, "0.8.2");
        assert_eq!(inventory.protocol, 20);
        assert_eq!(inventory.focus.workspace_id.as_deref(), Some("w20"));
        assert_eq!(inventory.focus.tab_id.as_deref(), Some("w20:t1"));
        assert_eq!(inventory.focus.pane_id.as_deref(), Some("w20:p1"));
        assert_eq!(inventory.workspaces[0].runtime_id, "w20");
        assert_eq!(
            inventory.workspaces[0]
                .worktree
                .as_ref()
                .map(|worktree| worktree.repository_name.as_str()),
            Some("protocol-20-project")
        );
        assert_eq!(inventory.panes[0].provider.as_deref(), Some("claude"));
        assert_eq!(
            inventory.panes[0].display_provider.as_deref(),
            Some("Claude Code")
        );
        assert_eq!(
            inventory.panes[0]
                .tokens
                .get("yard_role")
                .map(String::as_str),
            Some("implementer")
        );
        assert_eq!(
            inventory.workers[0].name.as_deref(),
            Some("yard-protocol20")
        );
        assert_eq!(inventory.workers[0].status, ObservedStatus::Working);
        assert!(inventory.workers[0].interactive_ready);
        assert_eq!(inventory.workers[0].state_change_sequence, 73);
        assert_eq!(
            inventory.workers[0]
                .provider_session
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("session_protocol20")
        );
    }

    #[test]
    fn rejects_protocol_mismatch() {
        let mut snapshot =
            decode_snapshot(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json")).unwrap();
        let unsupported_protocol = MAX_HERDR_PROTOCOL + 1;
        snapshot.protocol = unsupported_protocol;

        let error = normalize(snapshot, "default", 1, &HerdrConfig::default()).unwrap_err();

        assert!(matches!(
            error,
            HerdrError::ProtocolMismatch {
                minimum: MIN_HERDR_PROTOCOL,
                maximum: MAX_HERDR_PROTOCOL,
                actual
            } if actual == unsupported_protocol
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

    #[test]
    fn fleet_preserves_duplicate_panes_while_strict_normalization_rejects_them() {
        let mut fixture: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json"))
                .unwrap();
        let panes = fixture["result"]["snapshot"]["panes"]
            .as_array_mut()
            .unwrap();
        let mut conflicting = panes[0].clone();
        conflicting["tab_id"] = "conflicting-tab".into();
        panes.push(conflicting);
        let fixture = serde_json::to_vec(&fixture).unwrap();

        let inventory = normalize_fleet(
            decode_snapshot(&fixture).unwrap(),
            "default",
            1,
            &HerdrConfig::default(),
        )
        .unwrap();
        assert_eq!(inventory.panes.len(), 2);
        assert_eq!(inventory.panes[0].runtime_id, inventory.panes[1].runtime_id);
        assert_ne!(inventory.panes[0].tab_id, inventory.panes[1].tab_id);

        let error = normalize(
            decode_snapshot(&fixture).unwrap(),
            "default",
            1,
            &HerdrConfig::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate pane ID"));
    }

    #[test]
    fn fleet_preserves_conflicting_agent_ancestry_while_strict_normalization_rejects_it() {
        let mut fixture: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/v0.8.0/snapshot.json"))
                .unwrap();
        fixture["result"]["snapshot"]["agents"][0]["tab_id"] = "conflicting-tab".into();
        let fixture = serde_json::to_vec(&fixture).unwrap();

        let inventory = normalize_fleet(
            decode_snapshot(&fixture).unwrap(),
            "default",
            1,
            &HerdrConfig::default(),
        )
        .unwrap();
        assert_ne!(inventory.panes[0].tab_id, inventory.workers[0].tab_id);

        let error = normalize(
            decode_snapshot(&fixture).unwrap(),
            "default",
            1,
            &HerdrConfig::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("ancestry does not match"));
    }
}

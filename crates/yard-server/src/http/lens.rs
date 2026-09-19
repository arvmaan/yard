use std::collections::{BTreeMap, HashMap};

use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use yard_domain::{
    ObservedStatus, RuntimeInventory, RuntimeObservationState, RuntimeTopology, WorkerAvailability,
    WorkerCandidate, WorkerCandidates,
};

use super::{ApiError, AppState, NoStoreJson, runtime_topology_store_error};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LensIdentity {
    session: String,
    workspace_id: String,
    tab_id: Option<String>,
    pane_id: String,
    terminal_id: String,
}

#[derive(Default)]
struct LensEvidence<'a> {
    panes: Vec<&'a yard_domain::PaneObservation>,
    workers: Vec<&'a yard_domain::ObservedWorker>,
    durable: Vec<&'a WorkerCandidate>,
    occupants: Vec<&'a yard_domain::ManagedRuntimeOccupant>,
}

#[derive(Debug, Serialize)]
pub(super) struct RuntimeLens {
    adapter: String,
    selected_session: String,
    snapshot_current: bool,
    observed_at_unix_ms: String,
    inventory: RuntimeInventory,
    topology: RuntimeTopology,
    workers: WorkerCandidates,
    entries: Vec<RuntimeLensEntry>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent serialized wire flags"
)]
#[derive(Debug, Serialize)]
struct RuntimeLensEntry {
    classification: &'static str,
    session: String,
    workspace_id: String,
    tab_id: Option<String>,
    pane_id: String,
    terminal_id: String,
    observed_at_unix_ms: String,
    binding_last_observed_at_unix_ms: Option<String>,
    snapshot_current: bool,
    reason: String,
    worker_id: Option<String>,
    profile_name: Option<String>,
    availability: Option<WorkerAvailability>,
    provider: Option<String>,
    display_provider: Option<String>,
    name: Option<String>,
    label: Option<String>,
    status: ObservedStatus,
    focused: bool,
    launch_pending: bool,
    interactive_ready: bool,
}

pub(super) async fn runtime_lens(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> Result<NoStoreJson<RuntimeLens>, ApiError> {
    let session = required_session(&session)?;
    let inventory = state.source.inventory(&session).await?;
    let topology = state
        .store
        .runtime_topology("herdr", &inventory.session)
        .await
        .map_err(|error| runtime_topology_store_error(&error))?;
    let worker_candidates = state
        .store
        .list_worker_candidates()
        .await
        .map_err(|error| lens_store_error(&error))?;
    let entries = project_entries(&inventory, &topology, &worker_candidates);
    Ok(NoStoreJson(RuntimeLens {
        adapter: inventory.adapter.clone(),
        selected_session: inventory.session.clone(),
        snapshot_current: true,
        observed_at_unix_ms: inventory.observed_at_unix_ms.to_string(),
        inventory,
        topology,
        workers: worker_candidates,
        entries,
    }))
}

fn project_entries(
    inventory: &RuntimeInventory,
    topology: &RuntimeTopology,
    candidates: &WorkerCandidates,
) -> Vec<RuntimeLensEntry> {
    let mut entries = BTreeMap::<LensIdentity, LensEvidence<'_>>::new();
    for pane in &inventory.panes {
        entries
            .entry(LensIdentity {
                session: inventory.session.clone(),
                workspace_id: pane.workspace_id.clone(),
                tab_id: Some(pane.tab_id.clone()),
                pane_id: pane.runtime_id.clone(),
                terminal_id: pane.terminal_id.clone(),
            })
            .or_default()
            .panes
            .push(pane);
    }
    for worker in &inventory.workers {
        entries
            .entry(LensIdentity {
                session: inventory.session.clone(),
                workspace_id: worker.workspace_id.clone(),
                tab_id: Some(worker.tab_id.clone()),
                pane_id: worker.pane_id.clone(),
                terminal_id: worker.terminal_id.clone(),
            })
            .or_default()
            .workers
            .push(worker);
    }
    for candidate in &candidates.workers {
        let Some(runtime) = candidate.worker.runtime.as_ref().filter(|runtime| {
            runtime.adapter == inventory.adapter && runtime.session == inventory.session
        }) else {
            continue;
        };
        entries
            .entry(LensIdentity {
                session: runtime.session.clone(),
                workspace_id: runtime.workspace_id.clone(),
                tab_id: runtime.tab_id.clone(),
                pane_id: runtime.pane_id.clone(),
                terminal_id: runtime.terminal_id.clone(),
            })
            .or_default()
            .durable
            .push(candidate);
    }
    for workspace in &topology.managed_workspaces {
        for occupant in &workspace.occupants {
            entries
                .entry(LensIdentity {
                    session: topology.session.clone(),
                    workspace_id: workspace.workspace_id.clone(),
                    tab_id: occupant.tab_id.clone(),
                    pane_id: occupant.pane_id.clone(),
                    terminal_id: occupant.terminal_id.clone(),
                })
                .or_default()
                .occupants
                .push(occupant);
        }
    }

    let mut terminal_claims = HashMap::<String, usize>::new();
    for identity in entries.keys() {
        *terminal_claims
            .entry(identity.terminal_id.clone())
            .or_default() += 1;
    }
    entries
        .into_iter()
        .map(|(identity, evidence)| {
            let terminal_ambiguous = terminal_claims.get(&identity.terminal_id) != Some(&1);
            entry(
                inventory.observed_at_unix_ms,
                identity,
                &evidence,
                terminal_ambiguous,
            )
        })
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "single fail-closed classification path"
)]
fn entry(
    observed_at_unix_ms: u64,
    identity: LensIdentity,
    evidence: &LensEvidence<'_>,
    terminal_ambiguous: bool,
) -> RuntimeLensEntry {
    let worker = evidence.workers.first().copied();
    let pane = evidence.panes.first().copied();
    let durable = evidence.durable.first().copied();
    let occupant = evidence.occupants.first().copied();
    let duplicate = evidence.workers.len() > 1
        || evidence.panes.len() > 1
        || evidence.durable.len() > 1
        || evidence.occupants.len() > 1;
    let provider_ambiguous = worker.zip(pane).is_some_and(|(worker, pane)| {
        worker.provider_session.is_some()
            && pane.provider_session.is_some()
            && worker.provider_session != pane.provider_session
    }) || worker.zip(durable).is_some_and(|(worker, durable)| {
        durable.worker.runtime.as_ref().is_none_or(|runtime| {
            runtime.provider_session.as_ref() != worker.provider_session.as_ref()
        })
    });
    let durable_ambiguous = durable
        .and_then(|candidate| candidate.worker.runtime.as_ref())
        .is_some_and(|runtime| runtime.observation_state == RuntimeObservationState::Ambiguous);
    let (classification, reason) =
        if terminal_ambiguous || duplicate || provider_ambiguous || durable_ambiguous {
            (
                "ambiguous_identity",
                if terminal_ambiguous {
                    "The terminal identity has conflicting pane claims."
                } else if duplicate {
                    "The runtime snapshot contains duplicate identity records."
                } else {
                    "The worker provider identity is incoherent."
                },
            )
        } else if (durable.is_some() || occupant.is_some()) && worker.is_none() && pane.is_none() {
            (
                "stale_missing_binding",
                if durable.is_some() {
                    "The durable worker binding is missing from the fresh runtime snapshot."
                } else {
                    "The topology entry is missing from the fresh runtime snapshot."
                },
            )
        } else if durable.is_some() && (worker.is_some() || pane.is_some()) {
            (
                "linked_yard_worker",
                if worker.is_some() {
                    "The observed Herdr agent matches one durable Yard worker."
                } else {
                    "The current pane matches one durable Yard worker; terminal access only."
                },
            )
        } else if worker.is_some() {
            (
                "unassigned_herdr_agent",
                "The observed Herdr agent has no durable Yard worker binding.",
            )
        } else {
            (
                "topology_only_shell_pane",
                if pane.is_some() {
                    "The observed pane has no Herdr agent or durable Yard worker."
                } else {
                    "The managed topology occupant is absent from the fresh runtime snapshot."
                },
            )
        };
    RuntimeLensEntry {
        classification,
        session: identity.session,
        workspace_id: identity.workspace_id,
        tab_id: identity.tab_id,
        pane_id: identity.pane_id,
        terminal_id: identity.terminal_id,
        observed_at_unix_ms: observed_at_unix_ms.to_string(),
        binding_last_observed_at_unix_ms: durable
            .and_then(|candidate| candidate.worker.runtime.as_ref())
            .map(|runtime| runtime.last_observed_at_unix_ms.to_string()),
        snapshot_current: worker.is_some() || pane.is_some(),
        reason: reason.to_owned(),
        worker_id: durable.map(|candidate| candidate.worker.id.clone()),
        profile_name: durable.and_then(|candidate| candidate.profile_name.clone()),
        availability: durable.map(|candidate| candidate.availability),
        provider: worker
            .and_then(|worker| worker.provider.clone())
            .or_else(|| pane.and_then(|pane| pane.provider.clone())),
        display_provider: worker
            .and_then(|worker| worker.display_provider.clone())
            .or_else(|| pane.and_then(|pane| pane.display_provider.clone())),
        name: worker.and_then(|worker| worker.name.clone()),
        label: pane
            .and_then(|pane| pane.label.clone())
            .or_else(|| occupant.map(|occupant| occupant.label.clone())),
        status: worker.map_or_else(
            || pane.map_or(ObservedStatus::Unknown, |pane| pane.status),
            |worker| worker.status,
        ),
        focused: worker.map_or_else(
            || pane.is_some_and(|pane| pane.focused),
            |worker| worker.focused,
        ),
        launch_pending: worker.is_some_and(|worker| worker.launch_pending),
        interactive_ready: worker.is_some_and(|worker| worker.interactive_ready),
    }
}

fn required_session(session: &str) -> Result<String, ApiError> {
    let session = session.trim();
    if session.is_empty() || session.len() > 256 {
        return Err(ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_runtime_session",
            message: "session must be non-empty and no longer than 256 bytes".to_owned(),
        });
    }
    Ok(session.to_owned())
}

fn lens_store_error(error: &yard_store::ProjectStoreError) -> ApiError {
    runtime_topology_store_error(error)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yard_domain::{
        FocusObservation, ManagedRuntimeOccupant, ManagedRuntimeOccupantKind,
        ManagedRuntimeWorkspace, ManagedRuntimeWorkspaceKind, ObservedStatus, ObservedWorker,
        PaneObservation, ProviderSessionRef, RuntimeInventory, RuntimeObservationState,
        RuntimeProcessState, RuntimeTopology, Worker, WorkerAvailability, WorkerCandidate,
        WorkerCandidates, WorkerRuntimeBinding,
    };

    use super::project_entries;

    fn provider(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "thread".to_owned(),
            value: value.to_owned(),
        }
    }

    fn pane(index: usize, terminal_id: &str) -> PaneObservation {
        PaneObservation {
            runtime_id: format!("pane-{index}"),
            terminal_id: terminal_id.to_owned(),
            workspace_id: format!("workspace-{}", index / 20),
            tab_id: format!("tab-{index}"),
            focused: false,
            cwd: Some(format!("/work/{index}")),
            foreground_cwd: Some(format!("/work/{index}")),
            label: Some(format!("Pane {index}")),
            provider: Some("shell".to_owned()),
            display_provider: Some("Shell".to_owned()),
            status: ObservedStatus::Idle,
            tokens: BTreeMap::new(),
            provider_session: None,
            revision: index as u64 + 1,
        }
    }

    fn observed(index: usize, terminal_id: &str) -> ObservedWorker {
        let pane = pane(index, terminal_id);
        ObservedWorker {
            runtime_id: terminal_id.to_owned(),
            terminal_id: terminal_id.to_owned(),
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.runtime_id,
            name: Some(format!("Agent {index}")),
            provider: Some("codex".to_owned()),
            display_provider: Some("Codex".to_owned()),
            status: ObservedStatus::Working,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_sequence: index as u64 + 1,
            cwd: pane.cwd,
            foreground_cwd: pane.foreground_cwd,
            tokens: BTreeMap::new(),
            provider_session: Some(provider(&format!("thread-{index}"))),
            revision: index as u64 + 1,
        }
    }

    fn inventory(panes: Vec<PaneObservation>, workers: Vec<ObservedWorker>) -> RuntimeInventory {
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "selected".to_owned(),
            runtime_version: "0.8.2".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 123,
            focus: FocusObservation::default(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes,
            workers,
            child_agents: Vec::new(),
        }
    }

    fn candidate_with_state(
        index: usize,
        terminal_id: &str,
        observation_state: RuntimeObservationState,
    ) -> WorkerCandidate {
        let observed = observed(index, terminal_id);
        WorkerCandidate {
            worker: Worker {
                id: format!("worker-{index}"),
                profile_id: None,
                profile_version: None,
                desired_state: yard_domain::WorkerDesiredState::Running,
                runtime: Some(WorkerRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "selected".to_owned(),
                    workspace_id: observed.workspace_id,
                    terminal_id: terminal_id.to_owned(),
                    tab_id: Some(observed.tab_id),
                    pane_id: observed.pane_id,
                    provider_session: observed.provider_session,
                    owns_tab: false,
                    observation_state,
                    process_state: RuntimeProcessState::Running,
                    status: ObservedStatus::Working,
                    state_change_sequence: 1,
                    revision: 1,
                    version: 1,
                    last_observed_at_unix_ms: 123,
                }),
                version: 1,
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            },
            profile_name: Some(format!("Profile {index}")),
            default_role: Some("implementer".to_owned()),
            availability: WorkerAvailability::Assigned,
            project_id: Some("project-1".to_owned()),
            assignment_id: Some("assignment-1".to_owned()),
            reason: None,
        }
    }

    fn candidate(index: usize, terminal_id: &str) -> WorkerCandidate {
        candidate_with_state(index, terminal_id, RuntimeObservationState::Observed)
    }
    fn topology() -> RuntimeTopology {
        RuntimeTopology {
            adapter: "herdr".to_owned(),
            session: "selected".to_owned(),
            managed_workspaces: Vec::new(),
        }
    }

    #[test]
    fn classifies_linked_unassigned_shell_stale_and_ambiguous_entries() {
        let linked_pane = pane(1, "linked");
        let linked_worker = observed(1, "linked");
        let unassigned_pane = pane(2, "unassigned");
        let unassigned_worker = observed(2, "unassigned");
        let shell = pane(3, "shell");
        let mut ambiguous_left = pane(4, "ambiguous");
        let mut ambiguous_right = pane(5, "ambiguous");
        ambiguous_left.workspace_id = "workspace-left".to_owned();
        ambiguous_right.workspace_id = "workspace-right".to_owned();
        let snapshot = inventory(
            vec![
                linked_pane,
                unassigned_pane,
                shell,
                ambiguous_left,
                ambiguous_right,
            ],
            vec![linked_worker, unassigned_worker],
        );
        let entries = project_entries(
            &snapshot,
            &topology(),
            &WorkerCandidates {
                workers: vec![candidate(1, "linked"), candidate(6, "stale")],
            },
        );
        let classifications = entries
            .iter()
            .map(|entry| (entry.terminal_id.as_str(), entry.classification))
            .collect::<Vec<_>>();

        assert!(classifications.contains(&("linked", "linked_yard_worker")));
        assert!(classifications.contains(&("unassigned", "unassigned_herdr_agent")));
        assert!(classifications.contains(&("shell", "topology_only_shell_pane")));
        assert!(classifications.contains(&("stale", "stale_missing_binding")));
        assert_eq!(
            classifications
                .iter()
                .filter(|(terminal, classification)| {
                    *terminal == "ambiguous" && *classification == "ambiguous_identity"
                })
                .count(),
            2
        );
    }

    #[test]
    fn includes_missing_topology_occupants_as_stale_entries() {
        let topology = RuntimeTopology {
            adapter: "herdr".to_owned(),
            session: "selected".to_owned(),
            managed_workspaces: vec![ManagedRuntimeWorkspace {
                workspace_id: "managed".to_owned(),
                kind: ManagedRuntimeWorkspaceKind::Provisioning,
                label: "Managed".to_owned(),
                occupants: vec![ManagedRuntimeOccupant {
                    kind: ManagedRuntimeOccupantKind::Provisioning,
                    terminal_id: "topology-only".to_owned(),
                    tab_id: Some("tab-topology".to_owned()),
                    pane_id: "pane-topology".to_owned(),
                    label: "Provisioning".to_owned(),
                    reason: "Launch pending".to_owned(),
                    project_name: None,
                }],
            }],
        };
        let entries = project_entries(
            &inventory(Vec::new(), Vec::new()),
            &topology,
            &WorkerCandidates {
                workers: Vec::new(),
            },
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].classification, "stale_missing_binding");
        assert!(!entries[0].snapshot_current);
    }

    #[test]
    fn fresh_runtime_evidence_overrides_missing_but_not_ambiguous_bindings() {
        let missing_worker = pane(10, "missing-worker");
        let observed_worker = observed(10, "missing-worker");
        let missing_pane = pane(11, "missing-pane");
        let ambiguous_pane = pane(12, "ambiguous-binding");
        let snapshot = inventory(
            vec![missing_worker, missing_pane, ambiguous_pane],
            vec![observed_worker],
        );
        let entries = project_entries(
            &snapshot,
            &topology(),
            &WorkerCandidates {
                workers: vec![
                    candidate_with_state(10, "missing-worker", RuntimeObservationState::Missing),
                    candidate_with_state(11, "missing-pane", RuntimeObservationState::Missing),
                    candidate_with_state(
                        12,
                        "ambiguous-binding",
                        RuntimeObservationState::Ambiguous,
                    ),
                    candidate_with_state(13, "stale", RuntimeObservationState::Missing),
                ],
            },
        );
        let find = |terminal: &str| {
            entries
                .iter()
                .find(|entry| entry.terminal_id == terminal)
                .unwrap()
        };

        assert_eq!(find("missing-worker").classification, "linked_yard_worker");
        assert!(find("missing-worker").interactive_ready);
        assert_eq!(find("missing-pane").classification, "linked_yard_worker");
        assert!(!find("missing-pane").interactive_ready);
        assert!(find("missing-pane").reason.contains("terminal access only"));
        assert_eq!(find("stale").classification, "stale_missing_binding");
        assert!(!find("stale").snapshot_current);
        assert_eq!(
            find("ambiguous-binding").classification,
            "ambiguous_identity"
        );
    }

    #[test]
    fn unions_three_hundred_fifty_non_overlapping_runtime_identities() {
        let panes = (0..200)
            .map(|index| pane(index, &format!("terminal-{index}")))
            .collect::<Vec<_>>();
        let workers = (200..250)
            .map(|index| observed(index, &format!("terminal-{index}")))
            .collect::<Vec<_>>();
        let bindings = (250..350)
            .map(|index| candidate(index, &format!("terminal-{index}")))
            .collect::<Vec<_>>();

        let entries = project_entries(
            &inventory(panes, workers),
            &topology(),
            &WorkerCandidates { workers: bindings },
        );
        let count = |classification: &str| {
            entries
                .iter()
                .filter(|entry| entry.classification == classification)
                .count()
        };

        assert_eq!(entries.len(), 350);
        assert_eq!(count("linked_yard_worker"), 0);
        assert_eq!(count("unassigned_herdr_agent"), 50);
        assert_eq!(count("topology_only_shell_pane"), 200);
        assert_eq!(count("stale_missing_binding"), 100);
        assert_eq!(count("ambiguous_identity"), 0);
    }
}

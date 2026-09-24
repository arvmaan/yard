use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tokio::task::JoinSet;
use yard_domain::{
    ObservedStatus, RuntimeInventory, RuntimeObservationState, RuntimeTopology, WorkerAvailability,
    WorkerCandidate, WorkerCandidates,
};

use super::{ApiError, AppState, NoStoreJson, runtime_topology_store_error};

const FLEET_SNAPSHOT_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
pub(super) struct HerdrFleetInventory {
    adapter: String,
    live_pane_count: usize,
    sessions: Vec<HerdrLiveSession>,
    failures: Vec<HerdrSessionFailure>,
}

#[derive(Debug, Serialize)]
struct HerdrLiveSession {
    id: String,
    name: String,
    is_default: bool,
    metadata_ambiguous: bool,
    metadata_reason: Option<&'static str>,
    observed_at_unix_ms: String,
    pane_count: usize,
    panes: Vec<HerdrLivePane>,
}

#[derive(Debug, Serialize)]
struct HerdrSessionFailure {
    session: String,
    code: &'static str,
    reason: &'static str,
}

#[derive(Debug)]
pub(super) struct HerdrFleetUnavailable {
    code: &'static str,
    message: &'static str,
    attempted_session_count: Option<usize>,
    failed_session_count: Option<usize>,
}

#[derive(Serialize)]
struct HerdrFleetErrorEnvelope {
    error: HerdrFleetErrorBody,
}

#[derive(Serialize)]
struct HerdrFleetErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempted_session_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_session_count: Option<usize>,
}

impl IntoResponse for HerdrFleetUnavailable {
    fn into_response(self) -> Response {
        let mut response = (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HerdrFleetErrorEnvelope {
                error: HerdrFleetErrorBody {
                    code: self.code,
                    message: self.message,
                    attempted_session_count: self.attempted_session_count,
                    failed_session_count: self.failed_session_count,
                },
            }),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

#[derive(Debug, Serialize)]
struct HerdrLivePane {
    identity_key: String,
    kind: &'static str,
    observation: &'static str,
    reason: String,
    metadata_reason: Option<String>,
    has_live_pane: bool,
    session: String,
    workspace_id: Option<String>,
    workspace_label: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
    pane_instance_id: Option<String>,
    terminal_id: Option<String>,
    label: Option<String>,
    name: Option<String>,
    provider: Option<String>,
    display_provider: Option<String>,
    status: ObservedStatus,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
}

struct LivePaneEvidence<'a> {
    identity_hint: String,
    panes: Vec<&'a yard_domain::PaneObservation>,
    workers: Vec<&'a yard_domain::ObservedWorker>,
    terminal_identity_conflict: bool,
    terminal_agent_signal: bool,
}

#[derive(Clone, Copy)]
enum PaneAncestryIssue {
    MissingWorkspace,
    MissingTab,
    MissingWorkspaceAndTab,
    Conflicting,
}

impl PaneAncestryIssue {
    fn reason(self) -> &'static str {
        match self {
            Self::MissingWorkspace => "Missing workspace ancestry.",
            Self::MissingTab => "Missing tab ancestry.",
            Self::MissingWorkspaceAndTab => "Missing workspace and tab ancestry.",
            Self::Conflicting => "Conflicting pane ancestry.",
        }
    }

    fn trusts_workspace(self) -> bool {
        matches!(self, Self::MissingTab)
    }
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

#[derive(Debug)]
struct FleetSessionRequest {
    id: String,
    name: String,
    is_default: bool,
    descriptor: crate::inventory_service::RuntimeSessionDescriptor,
}

struct FleetSessionGroup {
    descriptor: crate::inventory_service::RuntimeSessionDescriptor,
    running: bool,
    conflicting: bool,
}

struct GroupedFleetSessions {
    requests: Vec<FleetSessionRequest>,
    failures: Vec<HerdrSessionFailure>,
    running_session_count: usize,
}

fn group_fleet_sessions(
    session_descriptors: Vec<crate::inventory_service::RuntimeSessionDescriptor>,
) -> GroupedFleetSessions {
    let mut groups = Vec::<FleetSessionGroup>::new();
    let mut group_indexes = HashMap::<String, usize>::new();
    for descriptor in session_descriptors {
        if let Some(index) = group_indexes.get(descriptor.id()).copied() {
            let group = &mut groups[index];
            group.running |= descriptor.summary().running;
            group.conflicting |= !same_session_descriptor(&group.descriptor, &descriptor);
            continue;
        }
        group_indexes.insert(descriptor.id().to_owned(), groups.len());
        groups.push(FleetSessionGroup {
            running: descriptor.summary().running,
            descriptor,
            conflicting: false,
        });
    }

    let mut requests = Vec::new();
    let mut failures = Vec::new();
    let mut running_session_count = 0;
    for group in groups {
        if !group.running {
            continue;
        }
        running_session_count += 1;
        if group.conflicting {
            failures.push(HerdrSessionFailure {
                session: group.descriptor.id().to_owned(),
                code: "session_metadata_ambiguous",
                reason: "Herdr reported conflicting session metadata.",
            });
            continue;
        }
        requests.push(FleetSessionRequest {
            id: group.descriptor.id().to_owned(),
            name: group.descriptor.summary().name.clone(),
            is_default: group.descriptor.summary().is_default,
            descriptor: group.descriptor,
        });
    }
    GroupedFleetSessions {
        requests,
        failures,
        running_session_count,
    }
}

fn same_session_descriptor(
    left: &crate::inventory_service::RuntimeSessionDescriptor,
    right: &crate::inventory_service::RuntimeSessionDescriptor,
) -> bool {
    left.id() == right.id()
        && left.snapshot_key() == right.snapshot_key()
        && left.summary().name == right.summary().name
        && left.summary().is_default == right.summary().is_default
        && left.summary().running == right.summary().running
}

#[allow(
    clippy::too_many_lines,
    reason = "single read-only bounded fleet observation path"
)]
pub(super) async fn runtime_fleet_inventory(
    State(state): State<AppState>,
) -> Result<NoStoreJson<HerdrFleetInventory>, HerdrFleetUnavailable> {
    let (adapter, session_descriptors) =
        state.source.session_descriptors().await.map_err(|error| {
            tracing::warn!(%error, "Herdr fleet session discovery failed");
            herdr_fleet_discovery_unavailable()
        })?;
    let grouped_sessions = group_fleet_sessions(session_descriptors);
    if grouped_sessions.running_session_count == 0 {
        return Ok(NoStoreJson(HerdrFleetInventory {
            adapter,
            live_pane_count: 0,
            sessions: Vec::new(),
            failures: Vec::new(),
        }));
    }
    if grouped_sessions.requests.is_empty() {
        return Ok(NoStoreJson(HerdrFleetInventory {
            adapter,
            live_pane_count: 0,
            sessions: Vec::new(),
            failures: grouped_sessions.failures,
        }));
    }

    let session_count = grouped_sessions.running_session_count;
    let request_count = grouped_sessions.requests.len();
    let mut pending = grouped_sessions.requests.into_iter().enumerate();
    let mut tasks = JoinSet::new();
    for (index, runtime_session) in pending.by_ref().take(FLEET_SNAPSHOT_CONCURRENCY) {
        let source = state.source.clone();
        tasks.spawn(async move {
            let snapshot = source
                .fleet_inventory_for_descriptor(&runtime_session.descriptor)
                .await;
            (index, runtime_session, snapshot)
        });
    }
    let mut snapshots = (0..request_count).map(|_| None).collect::<Vec<_>>();
    while let Some(result) = tasks.join_next().await {
        let snapshot = result.map_err(|error| {
            tracing::warn!(%error, "Herdr fleet snapshot task failed");
            herdr_fleet_unavailable(session_count, session_count)
        })?;
        let index = snapshot.0;
        snapshots[index] = Some(snapshot);
        if let Some((index, runtime_session)) = pending.next() {
            let source = state.source.clone();
            tasks.spawn(async move {
                let snapshot = source
                    .fleet_inventory_for_descriptor(&runtime_session.descriptor)
                    .await;
                (index, runtime_session, snapshot)
            });
        }
    }
    let mut observed_sessions = Vec::new();
    let mut failures = grouped_sessions.failures;
    for (_, runtime_session, snapshot) in snapshots.into_iter().flatten() {
        match snapshot {
            Ok(inventory) if inventory.session == runtime_session.id => {
                observed_sessions.push((runtime_session, inventory));
            }
            Ok(inventory) => {
                tracing::warn!(
                    expected_session = %runtime_session.id,
                    observed_session = %inventory.session,
                    "Herdr returned a mismatched fleet snapshot"
                );
                failures.push(HerdrSessionFailure {
                    session: runtime_session.id,
                    code: "session_identity_mismatch",
                    reason: "Herdr returned a snapshot for a different session.",
                });
            }
            Err(error) => {
                tracing::warn!(
                    session = %runtime_session.id,
                    %error,
                    "Herdr fleet session snapshot failed"
                );
                failures.push(HerdrSessionFailure {
                    session: runtime_session.id,
                    code: "herdr_snapshot_failed",
                    reason: "Herdr snapshot was unavailable for this session.",
                });
            }
        }
    }
    if observed_sessions.is_empty() {
        return Err(herdr_fleet_unavailable(session_count, failures.len()));
    }

    let sessions = observed_sessions
        .into_iter()
        .map(|(runtime_session, inventory)| {
            project_live_session_with_metadata(
                runtime_session.id,
                runtime_session.name,
                runtime_session.is_default,
                false,
                &inventory,
            )
        })
        .collect::<Vec<_>>();
    let live_pane_count = sessions.iter().map(|session| session.pane_count).sum();
    Ok(NoStoreJson(HerdrFleetInventory {
        adapter,
        live_pane_count,
        sessions,
        failures,
    }))
}

fn herdr_fleet_unavailable(
    attempted_session_count: usize,
    failed_session_count: usize,
) -> HerdrFleetUnavailable {
    HerdrFleetUnavailable {
        code: "herdr_fleet_unavailable",
        message: "Live Herdr inventory is temporarily unavailable; retry the request.",
        attempted_session_count: Some(attempted_session_count),
        failed_session_count: Some(failed_session_count),
    }
}

fn herdr_fleet_discovery_unavailable() -> HerdrFleetUnavailable {
    HerdrFleetUnavailable {
        code: "herdr_session_discovery_unavailable",
        message: "Unable to discover running Herdr sessions; retry the request.",
        attempted_session_count: None,
        failed_session_count: None,
    }
}

fn project_live_session_with_metadata(
    session_id: String,
    name: String,
    is_default: bool,
    metadata_ambiguous: bool,
    inventory: &RuntimeInventory,
) -> HerdrLiveSession {
    let mut workspace_labels = HashMap::<&str, Option<&str>>::new();
    for workspace in &inventory.workspaces {
        workspace_labels
            .entry(workspace.runtime_id.as_str())
            .and_modify(|label| {
                if *label != Some(workspace.label.as_str()) {
                    *label = None;
                }
            })
            .or_insert(Some(workspace.label.as_str()));
    }
    let mut tab_workspaces = HashMap::<&str, Option<&str>>::new();
    for tab in &inventory.tabs {
        tab_workspaces
            .entry(tab.runtime_id.as_str())
            .and_modify(|workspace_id| {
                if *workspace_id != Some(tab.workspace_id.as_str()) {
                    *workspace_id = None;
                }
            })
            .or_insert(Some(tab.workspace_id.as_str()));
    }
    let panes = group_live_evidence(&session_id, inventory)
        .into_iter()
        .map(|evidence| {
            let ancestry_issue = pane_ancestry_issue(&evidence, &workspace_labels, &tab_workspaces);
            let workspace_id = common_evidence_field(
                &evidence,
                |pane| pane.workspace_id.as_str(),
                |worker| worker.workspace_id.as_str(),
            );
            let workspace_label = ancestry_issue
                .is_none_or(PaneAncestryIssue::trusts_workspace)
                .then_some(workspace_id.as_deref())
                .flatten()
                .and_then(|workspace_id| workspace_labels.get(workspace_id).copied().flatten());
            project_live_pane(&session_id, workspace_label, ancestry_issue, &evidence)
        })
        .collect::<Vec<_>>();
    HerdrLiveSession {
        id: session_id,
        name,
        is_default,
        metadata_ambiguous,
        metadata_reason: metadata_ambiguous.then_some("Conflicting session metadata."),
        observed_at_unix_ms: inventory.observed_at_unix_ms.to_string(),
        pane_count: panes.iter().filter(|pane| pane.has_live_pane).count(),
        panes,
    }
}

#[cfg(test)]
fn project_live_session(
    session: String,
    is_default: bool,
    inventory: &RuntimeInventory,
) -> HerdrLiveSession {
    project_live_session_with_metadata(session.clone(), session, is_default, false, inventory)
}

fn group_live_evidence<'a>(
    session: &str,
    inventory: &'a RuntimeInventory,
) -> Vec<LivePaneEvidence<'a>> {
    let mut groups = Vec::<LivePaneEvidence<'a>>::new();
    let mut group_indexes = HashMap::<String, usize>::new();
    let mut unattached_indexes = HashMap::<String, usize>::new();
    let mut terminal_owners = HashMap::<&str, Option<usize>>::new();
    let mut terminal_conflicts = HashSet::new();
    for pane in &inventory.panes {
        let group_key = pane_group_key(session, &pane.runtime_id);
        let identity_hint = group_key.clone();
        let index = *group_indexes.entry(group_key).or_insert_with(|| {
            let index = groups.len();
            groups.push(LivePaneEvidence {
                identity_hint,
                panes: Vec::new(),
                workers: Vec::new(),
                terminal_identity_conflict: false,
                terminal_agent_signal: false,
            });
            index
        });
        groups[index].panes.push(pane);
        record_terminal_claim(
            &mut terminal_owners,
            &mut terminal_conflicts,
            pane.terminal_id.as_str(),
            index,
        );
    }
    let pane_terminal_owners = terminal_owners.clone();

    let terminals_with_workers = inventory
        .workers
        .iter()
        .map(|worker| worker.terminal_id.as_str())
        .collect::<HashSet<_>>();
    for worker in &inventory.workers {
        let group_key = pane_group_key(session, &worker.pane_id);
        let index = group_indexes.get(&group_key).copied().or_else(|| {
            pane_terminal_owners
                .get(worker.terminal_id.as_str())
                .copied()
                .flatten()
        });
        let index = index.unwrap_or_else(|| {
            let identity_hint = unattached_agent_group_key(session, worker);
            *unattached_indexes
                .entry(identity_hint.clone())
                .or_insert_with(|| {
                    let index = groups.len();
                    groups.push(LivePaneEvidence {
                        identity_hint,
                        panes: Vec::new(),
                        workers: Vec::new(),
                        terminal_identity_conflict: false,
                        terminal_agent_signal: false,
                    });
                    index
                })
        });
        groups[index].workers.push(worker);
        groups[index].terminal_agent_signal = true;
        record_terminal_claim(
            &mut terminal_owners,
            &mut terminal_conflicts,
            worker.terminal_id.as_str(),
            index,
        );
    }
    for index in terminal_conflicts {
        groups[index].terminal_identity_conflict = true;
    }
    for group in &mut groups {
        group.terminal_agent_signal |= group
            .panes
            .iter()
            .any(|pane| terminals_with_workers.contains(pane.terminal_id.as_str()));
    }
    groups
}

fn unattached_agent_group_key(session: &str, worker: &yard_domain::ObservedWorker) -> String {
    if !worker.runtime_id.is_empty() && worker.runtime_id != worker.terminal_id {
        return serde_json::to_string(&("agent", session, worker.runtime_id.as_str()))
            .expect("Herdr agent evidence grouping serialization cannot fail");
    }
    serde_json::to_string(&(
        "location",
        session,
        (!worker.workspace_id.is_empty()).then_some(worker.workspace_id.as_str()),
        (!worker.terminal_id.is_empty()).then_some(worker.terminal_id.as_str()),
    ))
    .expect("Herdr agent evidence grouping serialization cannot fail")
}

fn record_terminal_claim<'a>(
    owners: &mut HashMap<&'a str, Option<usize>>,
    conflicts: &mut HashSet<usize>,
    terminal_id: &'a str,
    index: usize,
) {
    if terminal_id.is_empty() {
        conflicts.insert(index);
        return;
    }
    match owners.entry(terminal_id) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(Some(index));
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => match *entry.get() {
            Some(owner) if owner != index => {
                conflicts.insert(owner);
                conflicts.insert(index);
                entry.insert(None);
            }
            None => {
                conflicts.insert(index);
            }
            Some(_) => {}
        },
    }
}

fn pane_ancestry_issue(
    evidence: &LivePaneEvidence<'_>,
    workspace_labels: &HashMap<&str, Option<&str>>,
    tab_workspaces: &HashMap<&str, Option<&str>>,
) -> Option<PaneAncestryIssue> {
    let mut missing_workspace = false;
    let mut missing_tab = false;
    let mut conflicting = false;
    for pane in &evidence.panes {
        missing_workspace |= pane.workspace_id.is_empty()
            || !workspace_labels.contains_key(pane.workspace_id.as_str());
        match tab_workspaces.get(pane.tab_id.as_str()) {
            None => missing_tab = true,
            Some(Some(workspace_id)) if *workspace_id != pane.workspace_id => {
                conflicting = true;
            }
            Some(None) => conflicting = true,
            Some(Some(_)) => {}
        }
    }
    if conflicting {
        Some(PaneAncestryIssue::Conflicting)
    } else {
        match (missing_workspace, missing_tab) {
            (true, true) => Some(PaneAncestryIssue::MissingWorkspaceAndTab),
            (true, false) => Some(PaneAncestryIssue::MissingWorkspace),
            (false, true) => Some(PaneAncestryIssue::MissingTab),
            (false, false) => None,
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "single fail-closed live pane classification path"
)]
fn project_live_pane(
    session: &str,
    workspace_label: Option<&str>,
    ancestry_issue: Option<PaneAncestryIssue>,
    evidence: &LivePaneEvidence<'_>,
) -> HerdrLivePane {
    let panes = evidence.panes.as_slice();
    let workers = evidence.workers.as_slice();
    let pane = panes.first().copied();
    let worker = (workers.len() == 1).then(|| workers[0]);
    let mut workspace_id = common_evidence_field(
        evidence,
        |pane| pane.workspace_id.as_str(),
        |worker| worker.workspace_id.as_str(),
    );
    let pane_id = common_evidence_field(
        evidence,
        |pane| pane.runtime_id.as_str(),
        |worker| worker.pane_id.as_str(),
    );
    let pane_instance_id = common_optional_evidence_field(
        evidence,
        |pane| pane.pane_instance_id.as_deref(),
        |worker| worker.pane_instance_id.as_deref(),
    )
    .0;
    let terminal_id = common_evidence_field(
        evidence,
        |pane| pane.terminal_id.as_str(),
        |worker| worker.terminal_id.as_str(),
    );
    let mut tab_id = common_evidence_field(
        evidence,
        |pane| pane.tab_id.as_str(),
        |worker| worker.tab_id.as_str(),
    );
    let tab_conflict = tab_id.is_none();
    let identity_conflict = evidence.terminal_identity_conflict
        || workspace_id.is_none()
        || pane_id.is_none()
        || terminal_id.is_none();
    let agent_signal = evidence.terminal_agent_signal
        || !workers.is_empty()
        || panes.iter().any(|pane| pane_has_agent_signal(pane));
    let partial_agent_evidence = worker.is_some_and(|worker| {
        !worker_has_coherent_agent_record(worker)
            || panes
                .iter()
                .any(|pane| pane_worker_metadata_conflicts(pane, worker))
    }) || panes
        .iter()
        .any(|pane| pane_has_agent_signal(pane) && !pane_has_coherent_provider_identity(pane));
    let (kind, observation, reason) = if panes.is_empty() {
        ("agent", "ambiguous", "No live pane observed.")
    } else if let Some(issue) = ancestry_issue {
        (
            if agent_signal { "agent" } else { "runtime" },
            "ambiguous",
            issue.reason(),
        )
    } else if tab_conflict {
        (
            if agent_signal { "agent" } else { "runtime" },
            "ambiguous",
            "Conflicting tab identity.",
        )
    } else if identity_conflict {
        (
            if agent_signal { "agent" } else { "runtime" },
            "ambiguous",
            "Partial or conflicting agent evidence.",
        )
    } else if panes.len() > 1 {
        (
            if agent_signal { "agent" } else { "runtime" },
            "ambiguous",
            "Herdr reported this pane identity more than once.",
        )
    } else if workers.len() > 1 {
        (
            "agent",
            "ambiguous",
            "Multiple live agents report this pane identity.",
        )
    } else if partial_agent_evidence {
        (
            "agent",
            "ambiguous",
            "Partial or conflicting agent evidence.",
        )
    } else if worker.is_some() {
        (
            "agent",
            "observed",
            if worker.is_some_and(|worker| worker.provider_session.is_some()) {
                "Live agent observed."
            } else {
                "Live providerless agent observed."
            },
        )
    } else if agent_signal {
        ("agent", "ambiguous", "Partial agent evidence.")
    } else {
        ("runtime", "observed", "Live runtime pane observed.")
    };
    let trusted_worker = (observation == "observed" && kind == "agent")
        .then_some(worker)
        .flatten();
    let trusted_runtime = observation == "observed" && kind == "runtime";
    let ambiguous = observation == "ambiguous";
    let (common_title, title_conflict) = common_optional_evidence_field(
        evidence,
        |pane| pane.label.as_deref(),
        |worker| worker.name.as_deref(),
    );
    let (common_cwd, cwd_conflict) = common_optional_evidence_field(
        evidence,
        |pane| pane.foreground_cwd.as_deref().or(pane.cwd.as_deref()),
        |worker| worker.foreground_cwd.as_deref().or(worker.cwd.as_deref()),
    );
    let (common_status, status_conflict) = common_evidence_status(evidence);
    let metadata_reason = ambiguous
        .then(|| {
            let mut conflicts = Vec::new();
            if title_conflict {
                conflicts.push("title");
            }
            if cwd_conflict {
                conflicts.push("working directory");
            }
            if tab_conflict {
                conflicts.push("tab");
            }
            if status_conflict {
                conflicts.push("process status");
            }
            (!conflicts.is_empty()).then(|| {
                format!(
                    "Conflicting or incomplete metadata: {}.",
                    conflicts.join(", ")
                )
            })
        })
        .flatten();
    if let Some(issue) = ancestry_issue {
        if !issue.trusts_workspace() {
            workspace_id = None;
        }
        tab_id = None;
    }
    let identity_key = evidence_identity_key(
        session,
        !panes.is_empty(),
        (!panes.is_empty())
            .then_some(workspace_id.as_deref())
            .flatten(),
        (!panes.is_empty()).then_some(pane_id.as_deref()).flatten(),
        (!panes.is_empty())
            .then_some(terminal_id.as_deref())
            .flatten(),
        panes.is_empty().then_some(evidence.identity_hint.as_str()),
    );
    HerdrLivePane {
        identity_key,
        kind,
        observation,
        reason: reason.to_owned(),
        metadata_reason,
        has_live_pane: !panes.is_empty(),
        session: session.to_owned(),
        workspace_id,
        workspace_label: workspace_label.map(str::to_owned),
        tab_id,
        pane_id,
        pane_instance_id,
        terminal_id,
        label: if ambiguous {
            common_title
        } else {
            pane.and_then(|pane| pane.label.clone())
        },
        name: trusted_worker.and_then(|worker| worker.name.clone()),
        provider: trusted_worker.and_then(|worker| worker.provider.clone()),
        display_provider: trusted_worker.and_then(|worker| worker.display_provider.clone()),
        status: if ambiguous {
            common_status.unwrap_or(ObservedStatus::Unknown)
        } else {
            trusted_worker.map_or_else(
                || {
                    if trusted_runtime {
                        pane.map_or(ObservedStatus::Unknown, |pane| pane.status)
                    } else {
                        ObservedStatus::Unknown
                    }
                },
                |worker| worker.status,
            )
        },
        cwd: if ambiguous {
            None
        } else {
            trusted_worker
                .and_then(|worker| worker.cwd.clone())
                .or_else(|| pane.and_then(|pane| pane.cwd.clone()))
        },
        foreground_cwd: if ambiguous {
            common_cwd
        } else {
            trusted_worker
                .and_then(|worker| worker.foreground_cwd.clone())
                .or_else(|| pane.and_then(|pane| pane.foreground_cwd.clone()))
        },
    }
}

fn pane_has_agent_signal(pane: &yard_domain::PaneObservation) -> bool {
    pane.provider.is_some() || pane.display_provider.is_some() || pane.provider_session.is_some()
}

fn pane_has_coherent_provider_identity(pane: &yard_domain::PaneObservation) -> bool {
    coherent_provider_identity(
        pane.provider.as_deref(),
        pane.display_provider.as_deref(),
        pane.provider_session.as_ref(),
    )
}

fn worker_has_coherent_agent_record(worker: &yard_domain::ObservedWorker) -> bool {
    coherent_provider_identity(
        worker.provider.as_deref(),
        worker.display_provider.as_deref(),
        worker.provider_session.as_ref(),
    )
}

fn coherent_provider_identity(
    provider: Option<&str>,
    display_provider: Option<&str>,
    provider_session: Option<&yard_domain::ProviderSessionRef>,
) -> bool {
    match (provider, display_provider, provider_session) {
        (None, None, None) => true,
        (Some(provider), display_provider, Some(session)) => {
            !provider.is_empty()
                && display_provider.is_none_or(|display| !display.is_empty())
                && session.provider == provider
                && !session.source.is_empty()
                && !session.provider.is_empty()
                && !session.kind.is_empty()
                && !session.value.is_empty()
        }
        _ => false,
    }
}

fn pane_worker_metadata_conflicts(
    pane: &yard_domain::PaneObservation,
    worker: &yard_domain::ObservedWorker,
) -> bool {
    pane_has_agent_signal(pane)
        && (pane.provider != worker.provider
            || pane.display_provider != worker.display_provider
            || pane.provider_session != worker.provider_session)
}

fn common_optional_evidence_field<'a>(
    evidence: &LivePaneEvidence<'a>,
    pane_field: fn(&'a yard_domain::PaneObservation) -> Option<&'a str>,
    worker_field: fn(&'a yard_domain::ObservedWorker) -> Option<&'a str>,
) -> (Option<String>, bool) {
    let mut values = evidence
        .panes
        .iter()
        .map(|pane| pane_field(pane))
        .chain(evidence.workers.iter().map(|worker| worker_field(worker)));
    let Some(Some(first)) = values.next() else {
        return (None, true);
    };
    if first.is_empty() || !values.all(|value| value == Some(first)) {
        (None, true)
    } else {
        (Some(first.to_owned()), false)
    }
}

fn common_evidence_status(evidence: &LivePaneEvidence<'_>) -> (Option<ObservedStatus>, bool) {
    let evidence_count = evidence.panes.len() + evidence.workers.len();
    let mut values = evidence
        .panes
        .iter()
        .map(|pane| pane.status)
        .chain(evidence.workers.iter().map(|worker| worker.status));
    let Some(first) = values.next() else {
        return (None, true);
    };
    if evidence_count == 1 {
        return (None, true);
    }
    if first == ObservedStatus::Unknown || !values.all(|value| value == first) {
        (None, true)
    } else {
        (Some(first), false)
    }
}

fn common_evidence_field<'a>(
    evidence: &LivePaneEvidence<'a>,
    pane_field: fn(&'a yard_domain::PaneObservation) -> &'a str,
    worker_field: fn(&'a yard_domain::ObservedWorker) -> &'a str,
) -> Option<String> {
    let mut values = evidence
        .panes
        .iter()
        .map(|pane| pane_field(pane))
        .chain(evidence.workers.iter().map(|worker| worker_field(worker)));
    let first = values.next()?;
    (!first.is_empty() && values.all(|value| value == first)).then(|| first.to_owned())
}

fn pane_group_key(session: &str, pane_id: &str) -> String {
    serde_json::to_string(&(session, pane_id))
        .expect("Herdr pane grouping serialization cannot fail")
}

fn evidence_identity_key(
    session: &str,
    has_live_pane: bool,
    workspace_id: Option<&str>,
    pane_id: Option<&str>,
    terminal_id: Option<&str>,
    evidence_identity: Option<&str>,
) -> String {
    serde_json::to_string(&(
        session,
        has_live_pane,
        workspace_id,
        pane_id,
        terminal_id,
        evidence_identity,
    ))
    .expect("Herdr pane identity serialization cannot fail")
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
    use std::collections::{BTreeMap, BTreeSet, HashSet};

    use yard_domain::{
        FocusObservation, ManagedRuntimeOccupant, ManagedRuntimeOccupantKind,
        ManagedRuntimeWorkspace, ManagedRuntimeWorkspaceKind, ObservedStatus, ObservedWorker,
        PaneObservation, ProviderSessionRef, RuntimeInventory, RuntimeObservationState,
        RuntimeProcessState, RuntimeSession, RuntimeTopology, TabObservation, Worker,
        WorkerAvailability, WorkerCandidate, WorkerCandidates, WorkerRuntimeBinding,
        WorkspaceObservation,
    };

    use crate::inventory_service::RuntimeSessionDescriptor;

    use super::{group_fleet_sessions, group_live_evidence, project_entries, project_live_session};

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
            pane_instance_id: None,
            terminal_id: terminal_id.to_owned(),
            workspace_id: format!("workspace-{}", index / 20),
            tab_id: format!("tab-{index}"),
            focused: false,
            cwd: Some(format!("/work/{index}")),
            foreground_cwd: Some(format!("/work/{index}")),
            label: Some(format!("Pane {index}")),
            provider: None,
            display_provider: None,
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
            pane_instance_id: None,
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

    fn agent_pane(index: usize, terminal_id: &str) -> PaneObservation {
        let worker = observed(index, terminal_id);
        let mut pane = pane(index, terminal_id);
        pane.provider = worker.provider;
        pane.display_provider = worker.display_provider;
        pane.provider_session = worker.provider_session;
        pane
    }

    fn inventory(panes: Vec<PaneObservation>, workers: Vec<ObservedWorker>) -> RuntimeInventory {
        let mut workspace_ids = HashSet::new();
        let mut tab_ids = HashSet::new();
        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        for pane in &panes {
            if workspace_ids.insert(pane.workspace_id.clone()) {
                workspaces.push(WorkspaceObservation {
                    runtime_id: pane.workspace_id.clone(),
                    order: workspaces.len(),
                    label: pane.workspace_id.clone(),
                    focused: false,
                    active_tab_id: pane.tab_id.clone(),
                    pane_count: 1,
                    tab_count: 1,
                    status: ObservedStatus::Unknown,
                    tokens: BTreeMap::new(),
                    worktree: None,
                });
            }
            if tab_ids.insert(pane.tab_id.clone()) {
                tabs.push(TabObservation {
                    runtime_id: pane.tab_id.clone(),
                    workspace_id: pane.workspace_id.clone(),
                    order: tabs.len(),
                    label: pane.tab_id.clone(),
                    focused: false,
                    pane_count: 1,
                    status: ObservedStatus::Unknown,
                });
            }
        }
        RuntimeInventory {
            adapter: "herdr".to_owned(),
            session: "selected".to_owned(),
            runtime_version: "0.8.2".to_owned(),
            protocol: 19,
            observed_at_unix_ms: 123,
            focus: FocusObservation::default(),
            workspaces,
            tabs,
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
    fn projects_every_live_pane_in_linear_passes() {
        let mut panes = (0..501)
            .map(|index| pane(index, &format!("terminal-{index}")))
            .collect::<Vec<_>>();
        panes[2] = agent_pane(2, "terminal-2");
        let mut unnamed = observed(2, "terminal-2");
        unnamed.name = None;
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(panes, vec![unnamed]),
        );

        assert_eq!(projected.pane_count, 501);
        assert_eq!(
            projected
                .panes
                .iter()
                .filter(|pane| pane.kind == "agent")
                .count(),
            1
        );
        assert_eq!(
            projected
                .panes
                .iter()
                .filter(|pane| pane.kind == "runtime")
                .count(),
            500
        );
        let unnamed = projected
            .panes
            .iter()
            .find(|pane| pane.terminal_id.as_deref() == Some("terminal-2"))
            .unwrap();
        assert_eq!(unnamed.observation, "observed");
        assert!(unnamed.name.is_none());
    }

    #[test]
    fn provider_pane_without_agent_record_is_ambiguous_agent() {
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![agent_pane(1, "provider-pane")], Vec::new()),
        );

        let pane = &projected.panes[0];
        assert_eq!(pane.kind, "agent");
        assert_eq!(pane.observation, "ambiguous");
        assert_eq!(pane.reason, "Partial agent evidence.");
        assert!(pane.name.is_none());
        assert!(pane.provider.is_none());
        assert!(pane.display_provider.is_none());
        assert_eq!(pane.status, ObservedStatus::Unknown);
    }

    #[test]
    fn conflicting_pane_and_agent_provider_identity_is_ambiguous() {
        let pane = agent_pane(1, "provider-conflict");
        let mut worker = observed(1, "provider-conflict");
        worker.provider = Some("claude".to_owned());
        worker.display_provider = Some("Claude".to_owned());
        worker.provider_session = Some(ProviderSessionRef {
            source: "herdr:claude".to_owned(),
            provider: "claude".to_owned(),
            kind: "session".to_owned(),
            value: "session-conflict".to_owned(),
        });
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![pane], vec![worker]),
        );

        let pane = &projected.panes[0];
        assert_eq!(pane.kind, "agent");
        assert_eq!(pane.observation, "ambiguous");
        assert!(pane.name.is_none());
        assert!(pane.provider.is_none());
        assert!(pane.display_provider.is_none());
        assert_eq!(pane.status, ObservedStatus::Unknown);
    }

    #[test]
    fn live_pane_classification_matrix_fails_closed() {
        let runtime = inventory(vec![pane(1, "runtime")], Vec::new());
        let provider_agent = inventory(
            vec![agent_pane(2, "provider-agent")],
            vec![observed(2, "provider-agent")],
        );
        let mut providerless_worker = observed(3, "providerless-agent");
        providerless_worker.provider = None;
        providerless_worker.display_provider = None;
        providerless_worker.provider_session = None;
        let providerless_agent = inventory(
            vec![pane(3, "providerless-agent")],
            vec![providerless_worker],
        );
        let provider_only = inventory(vec![agent_pane(4, "provider-only")], Vec::new());
        let mut partial_worker = observed(5, "partial-worker");
        partial_worker.provider_session = None;
        let partial_worker = inventory(vec![agent_pane(5, "partial-worker")], vec![partial_worker]);
        let mut conflicting_worker = observed(6, "conflicting-provider");
        conflicting_worker.display_provider = Some("Different provider".to_owned());
        let conflicting_provider = inventory(
            vec![agent_pane(6, "conflicting-provider")],
            vec![conflicting_worker],
        );
        let mut missing_identity_worker = observed(7, "missing-identity");
        missing_identity_worker.pane_id.clear();
        let missing_identity = inventory(
            vec![agent_pane(7, "missing-identity")],
            vec![missing_identity_worker],
        );

        let cases = [
            ("runtime", runtime, "runtime", "observed", None),
            (
                "provider agent",
                provider_agent,
                "agent",
                "observed",
                Some("codex"),
            ),
            (
                "providerless agent",
                providerless_agent,
                "agent",
                "observed",
                None,
            ),
            (
                "provider-only pane",
                provider_only,
                "agent",
                "ambiguous",
                None,
            ),
            ("partial worker", partial_worker, "agent", "ambiguous", None),
            (
                "conflicting provider",
                conflicting_provider,
                "agent",
                "ambiguous",
                None,
            ),
            (
                "missing agent identity",
                missing_identity,
                "agent",
                "ambiguous",
                None,
            ),
        ];

        for (case, inventory, expected_kind, expected_observation, expected_provider) in cases {
            let projected = project_live_session("selected".to_owned(), true, &inventory);
            let pane = &projected.panes[0];
            assert_eq!(pane.kind, expected_kind, "{case}");
            assert_eq!(pane.observation, expected_observation, "{case}");
            assert_eq!(pane.provider.as_deref(), expected_provider, "{case}");
            if expected_observation == "ambiguous" {
                assert!(pane.name.is_none(), "{case}");
                assert_eq!(pane.status, ObservedStatus::Unknown, "{case}");
            }
        }
    }

    #[test]
    fn missing_or_conflicting_pane_ancestry_is_ambiguous() {
        let coherent = || {
            inventory(
                vec![agent_pane(1, "ancestry")],
                vec![observed(1, "ancestry")],
            )
        };
        let mut missing_workspace = coherent();
        missing_workspace.workspaces.clear();
        let mut missing_tab = coherent();
        missing_tab.tabs.clear();
        let mut conflicting = coherent();
        conflicting.tabs[0].workspace_id = "other-workspace".to_owned();

        let cases = [
            (
                "missing workspace",
                missing_workspace,
                "Missing workspace ancestry.",
                None,
            ),
            (
                "missing tab",
                missing_tab,
                "Missing tab ancestry.",
                Some("workspace-0"),
            ),
            (
                "conflicting ancestry",
                conflicting,
                "Conflicting pane ancestry.",
                None,
            ),
        ];

        for (case, inventory, reason, workspace_id) in cases {
            let projected = project_live_session("selected".to_owned(), true, &inventory);
            let pane = &projected.panes[0];
            assert_eq!(pane.kind, "agent", "{case}");
            assert_eq!(pane.observation, "ambiguous", "{case}");
            assert_eq!(pane.reason, reason, "{case}");
            assert_eq!(pane.workspace_id.as_deref(), workspace_id, "{case}");
            assert!(pane.tab_id.is_none(), "{case}");
            assert!(pane.name.is_none(), "{case}");
            assert!(pane.provider.is_none(), "{case}");
            assert!(pane.display_provider.is_none(), "{case}");
            assert_eq!(pane.status, ObservedStatus::Unknown, "{case}");
        }
    }

    #[test]
    fn conflicting_tab_identity_is_optional_and_ambiguous() {
        let pane = agent_pane(1, "tab-conflict");
        let mut worker = observed(1, "tab-conflict");
        worker.tab_id = "different-tab".to_owned();
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![pane], vec![worker]),
        );

        let pane = &projected.panes[0];
        assert_eq!(pane.kind, "agent");
        assert_eq!(pane.observation, "ambiguous");
        assert_eq!(pane.reason, "Conflicting tab identity.");
        assert!(pane.tab_id.is_none());
        assert!(pane.provider.is_none());
        assert!(pane.name.is_none());
        assert_eq!(pane.status, ObservedStatus::Unknown);
    }

    #[test]
    fn preserves_duplicate_agent_evidence_until_ambiguity_classification() {
        let pane = agent_pane(1, "linked");
        let first = observed(1, "linked");
        let mut conflicting = first.clone();
        conflicting.runtime_id = "conflicting-agent".to_owned();
        conflicting.name = Some("Conflicting agent".to_owned());
        conflicting.provider = Some("claude".to_owned());
        conflicting.display_provider = Some("Claude".to_owned());
        conflicting.provider_session = Some(ProviderSessionRef {
            source: "herdr:claude".to_owned(),
            provider: "claude".to_owned(),
            kind: "session".to_owned(),
            value: "session-2".to_owned(),
        });
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![pane], vec![first, conflicting]),
        );

        assert_eq!(projected.panes.len(), 1);
        let pane = &projected.panes[0];
        assert_eq!(pane.kind, "agent");
        assert_eq!(pane.observation, "ambiguous");
        assert_eq!(
            pane.reason,
            "Multiple live agents report this pane identity."
        );
        assert!(pane.name.is_none());
        assert!(pane.provider.is_none());
        assert!(pane.display_provider.is_none());
        assert!(pane.label.is_none());
        assert_eq!(pane.status, ObservedStatus::Unknown);
        assert_eq!(pane.foreground_cwd.as_deref(), Some("/work/1"));
    }

    #[test]
    fn duplicate_pane_evidence_is_classified_before_deduplication() {
        let original = pane(1, "duplicate");
        let mut conflicting = original.clone();
        conflicting.label = Some("Conflicting pane".to_owned());
        conflicting.status = ObservedStatus::Blocked;
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![original, conflicting], Vec::new()),
        );

        assert_eq!(projected.panes.len(), 1);
        assert_eq!(projected.panes[0].kind, "runtime");
        assert_eq!(projected.panes[0].observation, "ambiguous");
        assert_eq!(
            projected.panes[0].reason,
            "Herdr reported this pane identity more than once."
        );
        assert!(projected.panes[0].label.is_none());
        assert_eq!(projected.panes[0].status, ObservedStatus::Unknown);
    }

    #[test]
    fn conflicting_duplicate_identity_is_order_independent_and_never_trusted() {
        let original_pane = agent_pane(1, "shared-terminal");
        let original_worker = observed(1, "shared-terminal");
        let mut conflicting_pane = original_pane.clone();
        conflicting_pane.workspace_id = "conflicting-workspace".to_owned();
        conflicting_pane.terminal_id = "conflicting-terminal".to_owned();
        conflicting_pane.label = Some("Conflicting pane".to_owned());
        conflicting_pane.provider = Some("claude".to_owned());
        conflicting_pane.display_provider = Some("Claude".to_owned());
        conflicting_pane.provider_session = Some(ProviderSessionRef {
            source: "herdr:claude".to_owned(),
            provider: "claude".to_owned(),
            kind: "session".to_owned(),
            value: "conflicting-session".to_owned(),
        });
        let mut conflicting_worker = original_worker.clone();
        conflicting_worker.workspace_id = conflicting_pane.workspace_id.clone();
        conflicting_worker.terminal_id = conflicting_pane.terminal_id.clone();
        conflicting_worker.runtime_id = conflicting_pane.terminal_id.clone();
        conflicting_worker.name = Some("Conflicting agent".to_owned());
        conflicting_worker.provider = conflicting_pane.provider.clone();
        conflicting_worker.display_provider = conflicting_pane.display_provider.clone();
        conflicting_worker.provider_session = conflicting_pane.provider_session.clone();

        let first = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(
                vec![original_pane.clone(), conflicting_pane.clone()],
                vec![original_worker.clone(), conflicting_worker.clone()],
            ),
        );
        let reversed = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(
                vec![conflicting_pane, original_pane],
                vec![conflicting_worker, original_worker],
            ),
        );

        assert_eq!(first.panes.len(), 1);
        assert_eq!(reversed.panes.len(), 1);
        for pane in [&first.panes[0], &reversed.panes[0]] {
            assert_eq!(pane.kind, "agent");
            assert_eq!(pane.observation, "ambiguous");
            assert!(pane.workspace_id.is_none());
            assert_eq!(pane.pane_id.as_deref(), Some("pane-1"));
            assert!(pane.terminal_id.is_none());
            assert!(pane.label.is_none());
            assert!(pane.name.is_none());
            assert!(pane.provider.is_none());
            assert!(pane.display_provider.is_none());
            assert_eq!(pane.status, ObservedStatus::Unknown);
        }
        assert_eq!(first.panes[0].identity_key, reversed.panes[0].identity_key);
    }

    #[test]
    fn malformed_shared_terminal_evidence_has_a_linear_storage_bound() {
        let panes = (0..1_000)
            .map(|index| agent_pane(index, "shared-terminal"))
            .collect::<Vec<_>>();
        let workers = (0..1_000)
            .map(|index| observed(index, "shared-terminal"))
            .collect::<Vec<_>>();
        let inventory = inventory(panes, workers);
        let groups = group_live_evidence("selected", &inventory);

        assert_eq!(groups.len(), 1_000);
        assert_eq!(
            groups
                .iter()
                .map(|group| group.panes.len() + group.workers.len())
                .sum::<usize>(),
            inventory.panes.len() + inventory.workers.len()
        );
        assert!(groups.iter().all(|group| group.terminal_identity_conflict));

        let projected = project_live_session("selected".to_owned(), true, &inventory);
        assert_eq!(projected.panes.len(), 1_000);
        assert!(projected.panes.iter().all(|pane| {
            pane.observation == "ambiguous"
                && pane.provider.is_none()
                && pane.name.is_none()
                && pane.status == ObservedStatus::Unknown
        }));
    }

    #[test]
    fn incomplete_provider_session_identity_is_always_ambiguous() {
        for field in ["source", "provider", "kind", "value"] {
            let mut pane = agent_pane(1, "partial-session");
            let mut worker = observed(1, "partial-session");
            let pane_session = pane.provider_session.as_mut().unwrap();
            let worker_session = worker.provider_session.as_mut().unwrap();
            match field {
                "source" => {
                    pane_session.source.clear();
                    worker_session.source.clear();
                }
                "provider" => {
                    pane_session.provider.clear();
                    worker_session.provider.clear();
                }
                "kind" => {
                    pane_session.kind.clear();
                    worker_session.kind.clear();
                }
                "value" => {
                    pane_session.value.clear();
                    worker_session.value.clear();
                }
                _ => unreachable!(),
            }

            let projected = project_live_session(
                "selected".to_owned(),
                true,
                &inventory(vec![pane], vec![worker]),
            );
            let pane = &projected.panes[0];
            assert_eq!(pane.kind, "agent", "{field}");
            assert_eq!(pane.observation, "ambiguous", "{field}");
            assert!(pane.provider.is_none(), "{field}");
            assert!(pane.name.is_none(), "{field}");
            assert_eq!(pane.status, ObservedStatus::Unknown, "{field}");
        }
    }

    #[test]
    fn groups_sessions_linearly_and_fails_closed_on_conflicting_duplicate_ids() {
        let descriptor = |id: &str, snapshot: &str, name: &str, is_default: bool, running: bool| {
            RuntimeSessionDescriptor::identified(
                id,
                snapshot,
                RuntimeSession {
                    name: name.to_owned(),
                    is_default,
                    running,
                },
            )
        };
        let descriptors = vec![
            descriptor("session-a", "socket-a", "shared", true, true),
            descriptor("session-a", "socket-a", "shared", true, true),
            descriptor("session-b", "socket-b", "shared", false, true),
            descriptor("session-c", "socket-c-1", "first", true, true),
            descriptor("session-c", "socket-c-2", "second", false, false),
        ];
        let summarize = |groups: super::GroupedFleetSessions| {
            (
                groups
                    .requests
                    .iter()
                    .map(|request| request.id.clone())
                    .collect::<Vec<_>>(),
                groups
                    .requests
                    .into_iter()
                    .map(|request| {
                        (
                            request.id,
                            (
                                request.name,
                                request.is_default,
                                request.descriptor.snapshot_key().to_owned(),
                            ),
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
                groups
                    .failures
                    .into_iter()
                    .map(|failure| (failure.session, (failure.code, failure.reason)))
                    .collect::<BTreeMap<_, _>>(),
                groups.running_session_count,
            )
        };
        let first = summarize(group_fleet_sessions(descriptors.clone()));
        let reversed = summarize(group_fleet_sessions(
            descriptors.into_iter().rev().collect(),
        ));

        assert_eq!(first.0, ["session-a", "session-b"]);
        assert_eq!(reversed.0, ["session-b", "session-a"]);
        assert_eq!(first.1, reversed.1);
        assert_eq!(first.2, reversed.2);
        assert_eq!(first.3, 3);
        assert_eq!(reversed.3, 3);
        assert!(!first.1.contains_key("session-c"));
        assert_eq!(
            first.2["session-c"],
            (
                "session_metadata_ambiguous",
                "Herdr reported conflicting session metadata.",
            )
        );
    }

    #[test]
    fn fleet_session_grouping_preserves_first_discovery_order_at_load() {
        let descriptors = (0..5_000)
            .flat_map(|index| {
                let descriptor = RuntimeSessionDescriptor::identified(
                    &format!("session-{index}"),
                    &format!("socket-{index}"),
                    RuntimeSession {
                        name: format!("display-{index}"),
                        is_default: index == 0,
                        running: true,
                    },
                );
                [descriptor.clone(), descriptor]
            })
            .collect::<Vec<_>>();

        let grouped = group_fleet_sessions(descriptors);

        assert_eq!(grouped.running_session_count, 5_000);
        assert!(grouped.failures.is_empty());
        assert_eq!(grouped.requests.len(), 5_000);
        assert!(
            grouped
                .requests
                .iter()
                .enumerate()
                .all(|(index, request)| request.id == format!("session-{index}"))
        );
    }

    #[test]
    fn unattached_agent_keys_are_stable_and_location_duplicates_dedupe() {
        let mut first = observed(7, "orphan-terminal");
        first.pane_id = "missing-pane-a".to_owned();
        let mut second = observed(8, "orphan-terminal");
        second.workspace_id = first.workspace_id.clone();
        second.pane_id = "missing-pane-b".to_owned();
        let projected = project_live_session(
            "session-a".to_owned(),
            true,
            &inventory(Vec::new(), vec![first.clone(), second.clone()]),
        );
        let reversed = project_live_session(
            "session-a".to_owned(),
            true,
            &inventory(Vec::new(), vec![second, first]),
        );

        assert_eq!(projected.pane_count, 0);
        assert_eq!(projected.panes.len(), 1);
        assert_eq!(reversed.panes.len(), 1);
        assert_eq!(
            projected.panes[0].identity_key,
            reversed.panes[0].identity_key
        );
        let evidence = &projected.panes[0];
        assert!(!evidence.has_live_pane);
        assert_eq!(evidence.kind, "agent");
        assert_eq!(evidence.observation, "ambiguous");
        assert_eq!(evidence.reason, "No live pane observed.");
        assert_eq!(evidence.session, "session-a");
        assert_eq!(evidence.terminal_id.as_deref(), Some("orphan-terminal"));
        assert!(evidence.pane_id.is_none());
        assert!(evidence.provider.is_none());
        assert_eq!(evidence.status, ObservedStatus::Working);
    }

    #[test]
    fn unattached_agent_ids_win_over_colliding_locations() {
        let mut first = observed(7, "orphan-terminal");
        first.runtime_id = "agent-a".to_owned();
        first.pane_id = "missing-pane-a".to_owned();
        let mut second = observed(8, "orphan-terminal");
        second.runtime_id = "agent-b".to_owned();
        second.workspace_id = first.workspace_id.clone();
        second.pane_id = "missing-pane-b".to_owned();
        let projected = project_live_session(
            "session-a".to_owned(),
            true,
            &inventory(Vec::new(), vec![first.clone(), second.clone()]),
        );
        let reversed = project_live_session(
            "session-a".to_owned(),
            true,
            &inventory(Vec::new(), vec![second, first]),
        );

        assert_eq!(projected.panes.len(), 2);
        assert_eq!(
            projected
                .panes
                .iter()
                .map(|pane| pane.identity_key.as_str())
                .collect::<BTreeSet<_>>(),
            reversed
                .panes
                .iter()
                .map(|pane| pane.identity_key.as_str())
                .collect::<BTreeSet<_>>()
        );
        assert_ne!(
            projected.panes[0].identity_key,
            projected.panes[1].identity_key
        );
    }

    #[test]
    fn duplicate_unattached_agent_ids_collapse_independently_of_mutable_metadata() {
        let mut first = observed(7, "first-terminal");
        first.runtime_id = "agent-a".to_owned();
        first.pane_id = "missing-pane-a".to_owned();
        let mut second = observed(8, "second-terminal");
        second.runtime_id = "agent-a".to_owned();
        second.workspace_id = "other-workspace".to_owned();
        second.pane_id = "missing-pane-b".to_owned();
        second.name = Some("Renamed agent".to_owned());
        second.status = ObservedStatus::Blocked;
        let projected = project_live_session(
            "session-a".to_owned(),
            true,
            &inventory(Vec::new(), vec![first.clone(), second.clone()]),
        );
        let reversed = project_live_session(
            "session-a".to_owned(),
            true,
            &inventory(Vec::new(), vec![second, first]),
        );

        assert_eq!(projected.panes.len(), 1);
        assert_eq!(reversed.panes.len(), 1);
        assert_eq!(
            projected.panes[0].identity_key,
            reversed.panes[0].identity_key
        );
        assert!(projected.panes[0].workspace_id.is_none());
        assert!(projected.panes[0].terminal_id.is_none());
        assert!(projected.panes[0].pane_id.is_none());
        assert_eq!(projected.panes[0].reason, "No live pane observed.");
    }

    #[test]
    fn ambiguous_metadata_is_common_across_every_evidence_item_or_omitted() {
        let mut pane = agent_pane(1, "shared");
        pane.label = Some("Shared title".to_owned());
        pane.status = ObservedStatus::Idle;
        let mut first = observed(1, "shared");
        first.name = Some("Shared title".to_owned());
        first.status = ObservedStatus::Idle;
        let mut second = first.clone();
        second.runtime_id = "duplicate-agent".to_owned();
        let common = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![pane.clone()], vec![first.clone(), second.clone()]),
        );
        let common = &common.panes[0];
        assert_eq!(common.observation, "ambiguous");
        assert_eq!(common.label.as_deref(), Some("Shared title"));
        assert_eq!(common.foreground_cwd.as_deref(), Some("/work/1"));
        assert_eq!(common.tab_id.as_deref(), Some("tab-1"));
        assert_eq!(common.status, ObservedStatus::Idle);
        assert!(common.metadata_reason.is_none());

        for field in ["title", "working directory", "tab", "process status"] {
            let mut conflicting = second.clone();
            match field {
                "title" => conflicting.name = Some("Different title".to_owned()),
                "working directory" => {
                    conflicting.cwd = Some("/different".to_owned());
                    conflicting.foreground_cwd = Some("/different".to_owned());
                }
                "tab" => conflicting.tab_id = "different-tab".to_owned(),
                "process status" => conflicting.status = ObservedStatus::Blocked,
                _ => unreachable!(),
            }
            let projected = project_live_session(
                "selected".to_owned(),
                true,
                &inventory(vec![pane.clone()], vec![first.clone(), conflicting]),
            );
            let projected = &projected.panes[0];
            assert!(
                projected
                    .metadata_reason
                    .as_deref()
                    .unwrap()
                    .contains(field),
                "{field}"
            );
            match field {
                "title" => assert!(projected.label.is_none()),
                "working directory" => assert!(projected.foreground_cwd.is_none()),
                "tab" => assert!(projected.tab_id.is_none()),
                "process status" => assert_eq!(projected.status, ObservedStatus::Unknown),
                _ => unreachable!(),
            }
        }

        let mut missing = second;
        missing.name = None;
        missing.cwd = None;
        missing.foreground_cwd = None;
        let projected = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![pane], vec![first, missing]),
        );
        assert!(projected.panes[0].label.is_none());
        assert!(projected.panes[0].foreground_cwd.is_none());
        assert!(
            projected.panes[0]
                .metadata_reason
                .as_deref()
                .unwrap()
                .contains("title, working directory")
        );
    }

    #[test]
    fn stable_live_identity_ignores_order_and_mutable_metadata() {
        let original_pane = agent_pane(1, "linked");
        let original_worker = observed(1, "linked");
        let original = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(
                vec![original_pane.clone(), pane(2, "runtime")],
                vec![original_worker.clone()],
            ),
        );

        let mut changed_pane = original_pane;
        changed_pane.label = Some("renamed pane".to_owned());
        changed_pane.display_provider = Some("Claude CLI".to_owned());
        changed_pane.provider = Some("claude".to_owned());
        changed_pane.provider_session = Some(ProviderSessionRef {
            source: "herdr:claude".to_owned(),
            provider: "claude".to_owned(),
            kind: "thread".to_owned(),
            value: "replacement".to_owned(),
        });
        changed_pane.status = ObservedStatus::Blocked;
        let mut changed_worker = original_worker;
        changed_worker.name = Some("renamed agent".to_owned());
        changed_worker.display_provider = Some("Claude CLI".to_owned());
        changed_worker.provider = Some("claude".to_owned());
        changed_worker.provider_session = changed_pane.provider_session.clone();
        changed_worker.status = ObservedStatus::Blocked;
        let reordered = project_live_session(
            "selected".to_owned(),
            true,
            &inventory(vec![pane(2, "runtime"), changed_pane], vec![changed_worker]),
        );

        let original_keys = original
            .panes
            .iter()
            .map(|pane| pane.identity_key.as_str())
            .collect::<std::collections::HashSet<_>>();
        let reordered_keys = reordered
            .panes
            .iter()
            .map(|pane| pane.identity_key.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(original_keys, reordered_keys);
        let original_linked = original
            .panes
            .iter()
            .find(|pane| pane.terminal_id.as_deref() == Some("linked"))
            .unwrap();
        let changed_linked = reordered
            .panes
            .iter()
            .find(|pane| pane.terminal_id.as_deref() == Some("linked"))
            .unwrap();
        assert_eq!(original_linked.identity_key, changed_linked.identity_key);
        assert_eq!(changed_linked.observation, "observed");
        assert_eq!(changed_linked.provider.as_deref(), Some("claude"));
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

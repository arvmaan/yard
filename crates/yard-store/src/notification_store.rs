//! Read-only attention records for outbound notifications (Slack phase 1).
//!
//! The notifier derives attention on the server from durable state, the same
//! rules the web applies in `RuntimeCanvas`/`projectUpdates`: Herdr's
//! `observed_status` on bound runtimes, and command acknowledgements that
//! ended `failed` or `ambiguous`. Nothing here writes, and nothing selects
//! prompt text, terminal output or error messages: a notification carries
//! titles and states only.
//!
//! Archived or deleted projects, ended or deleted workers, and archived or
//! deleted workstreams never produce records.

use std::collections::HashSet;

use rusqlite::{Connection, params};
use yard_domain::{ObservedStatus, RuntimeObservationState, RuntimeProcessState};

use super::{
    ProjectStoreError, SqliteProjectStore, observed_status, row_u64, runtime_observation_state,
    runtime_process_state, to_i64,
};

/// Which Yard seat a bound runtime occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttentionRuntimeRole {
    /// A worker running an open assignment.
    Assignment,
    /// A project's orchestrator.
    ProjectOrchestrator,
    /// The Yard-level orchestrator (superintendent).
    YardOrchestrator,
    /// A workstream coordination node's agent.
    CoordinationNode,
}

/// One bound runtime and the titles a notification may show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionRuntime {
    pub role: AttentionRuntimeRole,
    pub worker_id: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub assignment_id: Option<String>,
    /// The assignment objective (a title; the notifier truncates it).
    pub objective: Option<String>,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
    /// The pinned worker profile name, when the worker has one.
    pub profile_name: Option<String>,
    pub status: ObservedStatus,
    pub process_state: RuntimeProcessState,
    pub observation_state: RuntimeObservationState,
    pub state_change_sequence: u64,
    /// Provider session value, compared only to detect a different agent.
    pub provider_session: Option<String>,
    /// Assignment workers only: whether the latest assignment prompt that did
    /// not fail came from Yard's automatic summary requests (`yard:auto:*`),
    /// and when it was created. The notifier uses it to leave automatic
    /// summary turns out of "ready for review".
    pub last_prompt_automatic: bool,
    pub last_prompt_at_unix_ms: Option<u64>,
}

/// A terminal command outcome that needs the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttentionCommandStatus {
    Failed,
    Ambiguous,
}

/// A command acknowledgement that ended `failed` or `ambiguous`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionCommand {
    pub command_id: String,
    pub command_type: String,
    pub status: AttentionCommandStatus,
    pub updated_at_unix_ms: u64,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub assignment_id: Option<String>,
    pub objective: Option<String>,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttentionRecords {
    pub runtimes: Vec<AttentionRuntime>,
    pub commands: Vec<AttentionCommand>,
    /// Projects that are neither archived nor deleted, so queued command
    /// notifications can be re-checked at send time.
    pub visible_project_ids: HashSet<String>,
    /// Coordination nodes that are neither archived nor deleted.
    pub visible_node_ids: HashSet<String>,
}

/// Command types whose failure or ambiguity is worth a notification: prompts,
/// routes, allocations, handoffs and dispositions.
pub const ATTENTION_COMMAND_TYPES: &[&str] = &[
    "assignment_prompt",
    "orchestrator_prompt",
    "yard_orchestrator_prompt",
    "yard_orchestrator_route",
    "coordination_node_prompt",
    "coordination_node_route",
    "profile_allocation",
    "worker_allocation",
    "worker_handoff",
    "assignment_disposition",
];

/// At most this many command rows per read; the notifier advances its
/// watermark and reads the rest on the next tick.
pub const ATTENTION_COMMAND_BATCH_LIMIT: usize = 200;

pub(super) async fn attention_records(
    store: &SqliteProjectStore,
    commands_updated_after_unix_ms: u64,
) -> Result<AttentionRecords, ProjectStoreError> {
    let after = to_i64(commands_updated_after_unix_ms)?;
    store
        .run(move |connection| {
            Ok(AttentionRecords {
                runtimes: select_runtimes(connection)?,
                commands: select_commands(connection, after)?,
                visible_project_ids: select_ids(
                    connection,
                    &format!("SELECT project.id FROM projects project WHERE {VISIBLE_PROJECT}"),
                )?,
                visible_node_ids: select_ids(
                    connection,
                    "SELECT node.id FROM coordination_nodes node
                      WHERE NOT EXISTS (SELECT 1 FROM archived_coordination_nodes archived
                                         WHERE archived.node_id = node.id)
                        AND NOT EXISTS (SELECT 1 FROM deleted_coordination_nodes deleted
                                         WHERE deleted.node_id = node.id)",
                )?,
            })
        })
        .await
}

const VISIBLE_PROJECT: &str = "NOT EXISTS (SELECT 1 FROM archived_projects archived
                                   WHERE archived.project_id = project.id)
         AND NOT EXISTS (SELECT 1 FROM deleted_projects deleted
                          WHERE deleted.project_id = project.id)";

/// A live, user-visible worker. System-ephemeral workers (isolated summary
/// workers and worker-cleanup advisors) are Yard's own plumbing and never
/// notify; a worker retired by worker cleanup is ended and unbound.
const LIVE_WORKER: &str = "worker.ended_at_unix_ms IS NULL
         AND worker.ownership_kind <> 'system_ephemeral'
         AND NOT EXISTS (SELECT 1 FROM deleted_workers deleted
                          WHERE deleted.worker_id = worker.id)";

/// Assignments that an isolated summary worker runs for Yard. Their turns are
/// not the owner's work, so they are never "ready for review" and their
/// failed commands are not reported.
const SUMMARY_ASSIGNMENT: &str = "SELECT 1 FROM summary_worker_commands summary
                                    WHERE summary.result_assignment_id = ";

fn select_ids(connection: &Connection, sql: &str) -> Result<HashSet<String>, ProjectStoreError> {
    let mut statement = connection.prepare(sql)?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(Into::into)
}

/// Actor prefix of Yard's automatic summary requests
/// (`automation_service::request_worker_summaries`).
const AUTOMATIC_PROMPT_ACTOR: &str = "yard:auto:%";

fn select_runtimes(connection: &Connection) -> Result<Vec<AttentionRuntime>, ProjectStoreError> {
    let binding_columns = "binding.observed_status, binding.process_state,
                binding.observation_state, binding.state_change_sequence,
                binding.provider_session_value";
    // The latest assignment prompt that did not fail, per open assignment
    // (SQLite takes the bare columns from the MAX row).
    let sql = format!(
        "WITH latest_prompt AS (
             SELECT prompt.assignment_id,
                    MAX(ack.created_at_unix_ms) AS created_at_unix_ms,
                    ack.actor LIKE '{AUTOMATIC_PROMPT_ACTOR}' AS automatic
               FROM assignment_prompt_commands prompt
               JOIN command_acknowledgements ack ON ack.id = prompt.command_id
              WHERE ack.status <> 'failed'
                AND prompt.assignment_id IN (
                    SELECT id FROM assignments
                     WHERE lifecycle IN ('allocating', 'active', 'handing_off'))
              GROUP BY prompt.assignment_id
         )
         SELECT 'assignment', worker.id, project.id, project.name, assignment.id,
                assignment.objective, NULL, NULL, revision.name, {binding_columns},
                COALESCE(latest_prompt.automatic, 0), latest_prompt.created_at_unix_ms
           FROM assignments assignment
           JOIN projects project ON project.id = assignment.project_id
           JOIN workers worker ON worker.id = assignment.worker_id
           JOIN worker_runtime_bindings binding ON binding.worker_id = worker.id
           LEFT JOIN worker_profile_revisions revision
             ON revision.profile_id = assignment.profile_id
            AND revision.version = assignment.profile_version
           LEFT JOIN latest_prompt ON latest_prompt.assignment_id = assignment.id
          WHERE assignment.lifecycle IN ('allocating', 'active', 'handing_off')
            AND {VISIBLE_PROJECT} AND {LIVE_WORKER}
            AND NOT EXISTS ({SUMMARY_ASSIGNMENT}assignment.id)
         UNION ALL
         SELECT 'project_orchestrator', worker.id, project.id, project.name, NULL,
                NULL, NULL, NULL, revision.name, {binding_columns}, 0, NULL
           FROM projects project
           JOIN workers worker ON worker.id = project.orchestrator_worker_id
           JOIN worker_runtime_bindings binding ON binding.worker_id = worker.id
           LEFT JOIN worker_profile_revisions revision
             ON revision.profile_id = worker.profile_id
            AND revision.version = worker.profile_version
          WHERE {VISIBLE_PROJECT} AND {LIVE_WORKER}
         UNION ALL
         SELECT 'yard_orchestrator', worker.id, NULL, NULL, NULL,
                NULL, NULL, NULL, revision.name, {binding_columns}, 0, NULL
           FROM yard_orchestrator yard
           JOIN workers worker ON worker.id = yard.worker_id
           JOIN worker_runtime_bindings binding ON binding.worker_id = worker.id
           LEFT JOIN worker_profile_revisions revision
             ON revision.profile_id = worker.profile_id
            AND revision.version = worker.profile_version
          WHERE {LIVE_WORKER}
         UNION ALL
         SELECT 'coordination_node', worker.id, NULL, NULL, NULL,
                NULL, node.id, node.name, revision.name, {binding_columns}, 0, NULL
           FROM coordination_nodes node
           JOIN workers worker ON worker.id = node.worker_id
           JOIN worker_runtime_bindings binding ON binding.worker_id = worker.id
           LEFT JOIN worker_profile_revisions revision
             ON revision.profile_id = worker.profile_id
            AND revision.version = worker.profile_version
          WHERE NOT EXISTS (SELECT 1 FROM archived_coordination_nodes archived
                             WHERE archived.node_id = node.id)
            AND NOT EXISTS (SELECT 1 FROM deleted_coordination_nodes deleted
                             WHERE deleted.node_id = node.id)
            AND {LIVE_WORKER}
          ORDER BY 2, 1"
    );
    let mut statement = connection.prepare(&sql)?;
    statement
        .query_map([], |row| {
            let role = match row.get::<_, String>(0)?.as_str() {
                "assignment" => AttentionRuntimeRole::Assignment,
                "project_orchestrator" => AttentionRuntimeRole::ProjectOrchestrator,
                "yard_orchestrator" => AttentionRuntimeRole::YardOrchestrator,
                _ => AttentionRuntimeRole::CoordinationNode,
            };
            Ok(AttentionRuntime {
                role,
                worker_id: row.get(1)?,
                project_id: row.get(2)?,
                project_name: row.get(3)?,
                assignment_id: row.get(4)?,
                objective: row.get(5)?,
                node_id: row.get(6)?,
                node_name: row.get(7)?,
                profile_name: row.get(8)?,
                status: observed_status(row, 9)?,
                process_state: runtime_process_state(row, 10)?,
                observation_state: runtime_observation_state(row, 11)?,
                state_change_sequence: row_u64(row, 12)?,
                provider_session: row.get(13)?,
                last_prompt_automatic: row.get::<_, i64>(14)? != 0,
                last_prompt_at_unix_ms: row
                    .get::<_, Option<i64>>(15)?
                    .map(|value| u64::try_from(value).unwrap_or(0)),
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn select_commands(
    connection: &Connection,
    updated_after_unix_ms: i64,
) -> Result<Vec<AttentionCommand>, ProjectStoreError> {
    let types = ATTENTION_COMMAND_TYPES
        .iter()
        .map(|kind| format!("'{kind}'"))
        .collect::<Vec<_>>()
        .join(", ");
    // Each command table names its project (and, for assignment commands,
    // the assignment); the first non-null wins. Only titles are selected.
    let sql = format!(
        "WITH attention AS (
             SELECT ack.id, ack.command_type, ack.status, ack.updated_at_unix_ms,
                    COALESCE(assignment_prompt.project_id, orchestrator_prompt.project_id,
                             yard_route.target_project_id, worker_allocation.project_id,
                             profile_allocation.project_id, handoff.target_project_id,
                             disposition.project_id, node_route.target_project_id)
                        AS project_id,
                    COALESCE(assignment_prompt.assignment_id, disposition.assignment_id,
                             handoff.source_assignment_id)
                        AS assignment_id,
                    COALESCE(node_prompt.node_id, node_route.node_id) AS node_id
               FROM command_acknowledgements ack
               LEFT JOIN assignment_prompt_commands assignment_prompt
                 ON assignment_prompt.command_id = ack.id
               LEFT JOIN orchestrator_prompt_commands orchestrator_prompt
                 ON orchestrator_prompt.command_id = ack.id
               LEFT JOIN yard_orchestrator_route_commands yard_route
                 ON yard_route.command_id = ack.id
               LEFT JOIN worker_allocation_commands worker_allocation
                 ON worker_allocation.command_id = ack.id
               LEFT JOIN profile_allocation_commands profile_allocation
                 ON profile_allocation.command_id = ack.id
               LEFT JOIN worker_handoff_commands handoff
                 ON handoff.command_id = ack.id
               LEFT JOIN assignment_disposition_commands disposition
                 ON disposition.command_id = ack.id
               LEFT JOIN coordination_node_prompt_commands node_prompt
                 ON node_prompt.command_id = ack.id
               LEFT JOIN coordination_node_route_commands node_route
                 ON node_route.command_id = ack.id
              WHERE ack.status IN ('failed', 'ambiguous')
                AND ack.command_type IN ({types})
                AND ack.updated_at_unix_ms > ?1
         )
         SELECT attention.id, attention.command_type, attention.status,
                attention.updated_at_unix_ms, attention.project_id, project.name,
                attention.assignment_id, assignment.objective, attention.node_id, node.name
           FROM attention
           LEFT JOIN projects project ON project.id = attention.project_id
           LEFT JOIN assignments assignment ON assignment.id = attention.assignment_id
           LEFT JOIN coordination_nodes node ON node.id = attention.node_id
          WHERE (attention.project_id IS NULL OR ({VISIBLE_PROJECT}))
            AND (attention.assignment_id IS NULL
                 OR NOT EXISTS ({SUMMARY_ASSIGNMENT}attention.assignment_id))
            AND (attention.node_id IS NULL OR (
                    NOT EXISTS (SELECT 1 FROM archived_coordination_nodes archived
                                 WHERE archived.node_id = attention.node_id)
                AND NOT EXISTS (SELECT 1 FROM deleted_coordination_nodes deleted
                                 WHERE deleted.node_id = attention.node_id)))
          ORDER BY attention.updated_at_unix_ms, attention.id
          LIMIT ?2"
    );
    let mut statement = connection.prepare(&sql)?;
    statement
        .query_map(
            params![
                updated_after_unix_ms,
                i64::try_from(ATTENTION_COMMAND_BATCH_LIMIT).unwrap_or(i64::MAX)
            ],
            |row| {
                let status = match row.get::<_, String>(2)?.as_str() {
                    "ambiguous" => AttentionCommandStatus::Ambiguous,
                    _ => AttentionCommandStatus::Failed,
                };
                Ok(AttentionCommand {
                    command_id: row.get(0)?,
                    command_type: row.get(1)?,
                    status,
                    updated_at_unix_ms: row_u64(row, 3)?,
                    project_id: row.get(4)?,
                    project_name: row.get(5)?,
                    assignment_id: row.get(6)?,
                    objective: row.get(7)?,
                    node_id: row.get(8)?,
                    node_name: row.get(9)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;
    use yard_domain::{
        CanvasPlacement, ConfirmProfileAllocation, CreateProject, CreateWorkerProfile,
        IsolationPolicy, ObservedStatus, ProjectRuntimeBinding, RuntimeObservationState,
        RuntimeProcessState, SendAssignmentPrompt, WorkerProfileSpec, WorkerRuntimeBinding,
    };

    use super::{AttentionCommandStatus, AttentionRuntimeRole};
    use crate::{SqliteProjectStore, TokenSpendCommandSource, YardStore};

    fn runtime(terminal_id: &str, pane_id: &str) -> WorkerRuntimeBinding {
        WorkerRuntimeBinding {
            adapter: "herdr".to_owned(),
            session: "default".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            terminal_id: terminal_id.to_owned(),
            tab_id: Some(format!("tab-{terminal_id}")),
            pane_id: pane_id.to_owned(),
            provider_session: None,
            owns_tab: false,
            observation_state: RuntimeObservationState::Observed,
            process_state: RuntimeProcessState::Running,
            status: ObservedStatus::Working,
            state_change_sequence: 1,
            revision: 1,
            version: 1,
            last_observed_at_unix_ms: 1,
        }
    }

    /// A project whose orchestrator is bound, plus one active assignment.
    async fn fixture(temp: &TempDir) -> (SqliteProjectStore, String, yard_domain::Assignment) {
        let store = SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(
                CreateProject {
                    name: "Checkout <api>".to_owned(),
                    runtime: ProjectRuntimeBinding {
                        adapter: "herdr".to_owned(),
                        session: "default".to_owned(),
                        workspace_id: "workspace-1".to_owned(),
                    },
                    orchestrator_observed_worker_id: "terminal-1".to_owned(),
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                },
                runtime("terminal-1", "pane-1"),
            )
            .await
            .unwrap();
        let profile = store
            .create_worker_profile(CreateWorkerProfile {
                spec: WorkerProfileSpec {
                    name: "Implementer".to_owned(),
                    runtime_adapter: "herdr".to_owned(),
                    provider: "codex".to_owned(),
                    model: None,
                    default_role: "implementer".to_owned(),
                    instructions_ref: None,
                    tools: Vec::new(),
                    skills: Vec::new(),
                    mcp_servers: Vec::new(),
                    sandbox_policy: "runtime_default".to_owned(),
                    worktree_policy: "project_workspace".to_owned(),
                    permission_policy: "runtime_default".to_owned(),
                    completion_contract: "manual_receipt".to_owned(),
                },
            })
            .await
            .unwrap();
        let allocation = ConfirmProfileAllocation {
            command_id: "attention-allocation".to_owned(),
            actor: "local-user".to_owned(),
            profile_id: profile.id,
            expected_profile_version: profile.version,
            expected_project_version: project.version,
            objective: "Ship the retry banner".to_owned(),
            role: "implementer".to_owned(),
            isolation_policy: IsolationPolicy::ProjectWorkspace,
        };
        store
            .begin_profile_allocation(&project.id, allocation.clone())
            .await
            .unwrap();
        let mut worker_runtime = runtime("terminal-2", "pane-2");
        worker_runtime.owns_tab = true;
        store
            .claim_provisioning_runtime(&allocation.command_id, worker_runtime.clone())
            .await
            .unwrap();
        store
            .persist_runtime_allocation(&allocation.command_id, worker_runtime)
            .await
            .unwrap();
        let assignment = store
            .activate_profile_allocation(&allocation.command_id)
            .await
            .unwrap()
            .assignment;
        (store, project.id, assignment)
    }

    fn connection(temp: &TempDir) -> Connection {
        Connection::open(temp.path().join("yard.sqlite3")).unwrap()
    }

    #[tokio::test]
    async fn lists_bound_runtimes_with_titles_and_status() {
        let temp = TempDir::new().unwrap();
        let (store, project_id, assignment) = fixture(&temp).await;
        connection(&temp)
            .execute(
                "UPDATE worker_runtime_bindings
                    SET observed_status = 'blocked', state_change_sequence = 9
                  WHERE worker_id = ?1",
                [&assignment.worker.id],
            )
            .unwrap();

        let records = store.attention_records(0).await.unwrap();

        assert_eq!(records.runtimes.len(), 2, "{:?}", records.runtimes);
        let worker = records
            .runtimes
            .iter()
            .find(|runtime| runtime.role == AttentionRuntimeRole::Assignment)
            .unwrap();
        assert_eq!(worker.worker_id, assignment.worker.id);
        assert_eq!(worker.project_id.as_deref(), Some(project_id.as_str()));
        assert_eq!(worker.project_name.as_deref(), Some("Checkout <api>"));
        assert_eq!(
            worker.assignment_id.as_deref(),
            Some(assignment.id.as_str())
        );
        assert_eq!(worker.objective.as_deref(), Some("Ship the retry banner"));
        assert_eq!(worker.profile_name.as_deref(), Some("Implementer"));
        assert_eq!(worker.status, ObservedStatus::Blocked);
        assert_eq!(worker.state_change_sequence, 9);
        let orchestrator = records
            .runtimes
            .iter()
            .find(|runtime| runtime.role == AttentionRuntimeRole::ProjectOrchestrator)
            .unwrap();
        assert_eq!(orchestrator.status, ObservedStatus::Working);
        assert!(records.commands.is_empty());
        assert!(!worker.last_prompt_automatic);
        assert_eq!(worker.last_prompt_at_unix_ms, None);
        assert!(records.visible_project_ids.contains(&project_id));
    }

    #[tokio::test]
    async fn the_latest_unfailed_prompt_tells_automatic_summary_turns_apart() {
        let temp = TempDir::new().unwrap();
        let (store, project_id, assignment) = fixture(&temp).await;
        let prompt = |command_id: &str, actor: &str| SendAssignmentPrompt {
            command_id: command_id.to_owned(),
            actor: actor.to_owned(),
            attempt_id: assignment.attempt.id.clone(),
            expected_assignment_version: assignment.version,
            expected_attempt_version: assignment.attempt.version,
            text: "summary please".to_owned(),
        };
        let worker = |records: &super::AttentionRecords| {
            records
                .runtimes
                .iter()
                .find(|runtime| runtime.role == AttentionRuntimeRole::Assignment)
                .cloned()
                .unwrap()
        };
        let created_at = |command_id: &str, at: i64| {
            connection(&temp)
                .execute(
                    "UPDATE command_acknowledgements SET created_at_unix_ms = ?2 WHERE id = ?1",
                    rusqlite::params![command_id, at],
                )
                .unwrap();
        };
        store
            .begin_assignment_prompt(
                &project_id,
                &assignment.id,
                prompt(
                    "auto-summary",
                    &format!("yard:auto:project-worker-summary:{project_id}"),
                ),
                // The actor marks it automatic; the spend source is not read.
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        store
            .succeed_assignment_prompt("auto-summary", "working")
            .await
            .unwrap();
        created_at("auto-summary", 1_000);
        let automatic = worker(&store.attention_records(0).await.unwrap());
        assert!(automatic.last_prompt_automatic);
        assert_eq!(automatic.last_prompt_at_unix_ms, Some(1_000));

        // A failed owner prompt started no turn and changes nothing.
        store
            .begin_assignment_prompt(
                &project_id,
                &assignment.id,
                prompt("owner-failed", "local-user"),
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        store
            .fail_assignment_prompt("owner-failed", "herdr refused", false)
            .await
            .unwrap();
        created_at("owner-failed", 2_000);
        assert!(worker(&store.attention_records(0).await.unwrap()).last_prompt_automatic);

        store
            .begin_assignment_prompt(
                &project_id,
                &assignment.id,
                prompt("owner", "local-user"),
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        created_at("owner", 3_000);
        let manual = worker(&store.attention_records(0).await.unwrap());
        assert!(!manual.last_prompt_automatic);
        assert_eq!(manual.last_prompt_at_unix_ms, Some(3_000));
    }

    #[tokio::test]
    async fn archived_deleted_and_ended_work_produces_no_records() {
        let temp = TempDir::new().unwrap();
        let (store, project_id, assignment) = fixture(&temp).await;
        let connection = connection(&temp);
        connection
            .execute(
                "UPDATE workers SET ended_at_unix_ms = 5 WHERE id = ?1",
                [&assignment.worker.id],
            )
            .unwrap();
        let records = store.attention_records(0).await.unwrap();
        assert_eq!(
            records
                .runtimes
                .iter()
                .map(|runtime| runtime.role)
                .collect::<Vec<_>>(),
            vec![AttentionRuntimeRole::ProjectOrchestrator]
        );

        // An archived project drops out of runtimes, commands and the
        // visible set.
        connection
            .execute(
                "UPDATE workers SET ended_at_unix_ms = NULL WHERE id = ?1",
                [&assignment.worker.id],
            )
            .unwrap();
        store
            .begin_assignment_prompt(
                &project_id,
                &assignment.id,
                SendAssignmentPrompt {
                    command_id: "archived-prompt".to_owned(),
                    actor: "local-user".to_owned(),
                    attempt_id: assignment.attempt.id.clone(),
                    expected_assignment_version: assignment.version,
                    expected_attempt_version: assignment.attempt.version,
                    text: "hello".to_owned(),
                },
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        store
            .fail_assignment_prompt("archived-prompt", "herdr refused", false)
            .await
            .unwrap();
        let before = store.attention_records(0).await.unwrap();
        assert_eq!(before.runtimes.len(), 2);
        assert_eq!(before.commands.len(), 1);
        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO archived_projects
                        (project_id, command_id, expected_project_version,
                         expected_orchestrator_worker_id, expected_orchestrator_worker_version,
                         expected_orchestrator_runtime_version, result_project_version,
                         result_orchestrator_worker_version, runtime_adapter, runtime_session,
                         runtime_workspace_id, archived_at_unix_ms)
                 SELECT id, 'archive-command', 1, orchestrator_worker_id, 1, NULL, 2, 2,
                        'herdr', 'default', 'workspace-1', 6
                   FROM projects WHERE id = '{project_id}';"
            ))
            .unwrap();
        let archived = store.attention_records(0).await.unwrap();
        assert!(archived.runtimes.is_empty(), "{:?}", archived.runtimes);
        assert!(archived.commands.is_empty(), "{:?}", archived.commands);
        assert!(!archived.visible_project_ids.contains(&project_id));
        connection
            .execute("DELETE FROM archived_projects", [])
            .unwrap();

        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO deleted_projects
                        (project_id, command_id, orchestrator_worker_id, deleted_at_unix_ms)
                 SELECT id, 'delete-command', orchestrator_worker_id, 6
                   FROM projects WHERE id = '{project_id}';"
            ))
            .unwrap();
        assert!(
            store
                .attention_records(0)
                .await
                .unwrap()
                .runtimes
                .is_empty()
        );
    }

    fn roles(records: &super::AttentionRecords) -> Vec<AttentionRuntimeRole> {
        records
            .runtimes
            .iter()
            .map(|runtime| runtime.role)
            .collect()
    }

    /// Begin and fail one prompt on the fixture assignment.
    async fn failed_prompt(
        store: &SqliteProjectStore,
        project_id: &str,
        assignment: &yard_domain::Assignment,
        command_id: &str,
    ) {
        store
            .begin_assignment_prompt(
                project_id,
                &assignment.id,
                SendAssignmentPrompt {
                    command_id: command_id.to_owned(),
                    actor: "local-user".to_owned(),
                    attempt_id: assignment.attempt.id.clone(),
                    expected_assignment_version: assignment.version,
                    expected_attempt_version: assignment.attempt.version,
                    text: "hello".to_owned(),
                },
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        store
            .fail_assignment_prompt(command_id, "herdr refused", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn system_ephemeral_workers_produce_no_records() {
        let temp = TempDir::new().unwrap();
        let (store, _project_id, assignment) = fixture(&temp).await;
        assert_eq!(roles(&store.attention_records(0).await.unwrap()).len(), 2);
        // The same row shape mainline gives isolated summary workers and
        // worker-cleanup advisors.
        connection(&temp)
            .execute(
                "UPDATE workers SET ownership_kind = 'system_ephemeral' WHERE id = ?1",
                [&assignment.worker.id],
            )
            .unwrap();
        assert_eq!(
            roles(&store.attention_records(0).await.unwrap()),
            vec![AttentionRuntimeRole::ProjectOrchestrator]
        );
        // Yard-owned (and external) workers still notify.
        connection(&temp)
            .execute(
                "UPDATE workers SET ownership_kind = 'yard_owned' WHERE id = ?1",
                [&assignment.worker.id],
            )
            .unwrap();
        assert_eq!(roles(&store.attention_records(0).await.unwrap()).len(), 2);
    }

    #[tokio::test]
    async fn summary_worker_assignments_produce_no_records() {
        let temp = TempDir::new().unwrap();
        let (store, project_id, assignment) = fixture(&temp).await;
        failed_prompt(&store, &project_id, &assignment, "summary-prompt").await;
        let before = store.attention_records(0).await.unwrap();
        assert_eq!(before.runtimes.len(), 2);
        assert_eq!(before.commands.len(), 1);

        // The assignment is the result of an isolated summary worker command
        // (the worker keeps its ownership kind here, so only the summary
        // reference excludes it).
        connection(&temp)
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO summary_worker_commands (
                        command_id, actor, project_id, parent_worker_id,
                        expected_parent_worker_version, expected_project_version,
                        profile_id, profile_version, expected_artifact_id, objective,
                        captured_adapter, captured_runtime_session, captured_workspace_id,
                        captured_terminal_id, captured_tab_id, captured_pane_id,
                        status, result_worker_id, result_assignment_id,
                        created_at_unix_ms, updated_at_unix_ms)
                 SELECT 'summary-command', 'yard:summary', project_id, worker_id, 1, 1,
                        profile_id, profile_version, 'summary-artifact', 'Summarize',
                        'herdr', 'default', 'workspace-1', 'terminal-1', 'tab-terminal-1',
                        'pane-1', 'active', worker_id, id, 1, 1
                   FROM assignments WHERE id = '{}';",
                assignment.id
            ))
            .unwrap();

        let after = store.attention_records(0).await.unwrap();
        assert_eq!(
            roles(&after),
            vec![AttentionRuntimeRole::ProjectOrchestrator]
        );
        assert!(after.commands.is_empty(), "{:?}", after.commands);
        assert!(after.visible_project_ids.contains(&project_id));
    }

    #[tokio::test]
    async fn a_worker_retired_by_worker_cleanup_produces_no_records() {
        let temp = TempDir::new().unwrap();
        let (store, _project_id, assignment) = fixture(&temp).await;
        connection(&temp)
            .execute(
                "UPDATE worker_runtime_bindings SET observed_status = 'done'
                  WHERE worker_id = ?1",
                [&assignment.worker.id],
            )
            .unwrap();
        assert_eq!(roles(&store.attention_records(0).await.unwrap()).len(), 2);
        // What `worker_cleanup_store` does when it retires a worker: drop the
        // runtime binding and end the worker in one transaction.
        connection(&temp)
            .execute_batch(&format!(
                "DELETE FROM worker_runtime_bindings WHERE worker_id = '{id}';
                 UPDATE workers SET ended_at_unix_ms = 7, version = version + 1,
                                    updated_at_unix_ms = 7
                  WHERE id = '{id}' AND ended_at_unix_ms IS NULL;",
                id = assignment.worker.id
            ))
            .unwrap();
        assert_eq!(
            roles(&store.attention_records(0).await.unwrap()),
            vec![AttentionRuntimeRole::ProjectOrchestrator]
        );
    }

    #[tokio::test]
    async fn archived_coordination_nodes_leave_the_visible_set() {
        let temp = TempDir::new().unwrap();
        let (store, project_id, _) = fixture(&temp).await;
        let node_id = uuid::Uuid::now_v7().to_string();
        let node = store
            .create_coordination_node(
                &node_id,
                Some(temp.path().to_string_lossy().into_owned()),
                None,
                yard_domain::CreateCoordinationNode {
                    command_id: "create-node".to_owned(),
                    actor: "local-user".to_owned(),
                    name: "Research".to_owned(),
                    kind: yard_domain::CoordinationNodeKind::Workstream,
                    placement: CanvasPlacement {
                        x: 0.0,
                        y: 0.0,
                        width: 322.0,
                        height: 240.0,
                    },
                    attached_project_ids: vec![project_id],
                },
            )
            .await
            .unwrap()
            .node;
        let visible = store.attention_records(0).await.unwrap().visible_node_ids;
        assert!(visible.contains(&node_id), "{visible:?}");
        store
            .archive_coordination_node(
                &node_id,
                yard_domain::ArchiveCoordinationNode {
                    command_id: "archive-node".to_owned(),
                    actor: "local-user".to_owned(),
                    expected_node_version: node.version,
                    expected_worker_version: node.worker.as_ref().map(|worker| worker.version),
                },
            )
            .await
            .unwrap();
        let visible = store.attention_records(0).await.unwrap().visible_node_ids;
        assert!(!visible.contains(&node_id), "{visible:?}");
    }

    #[tokio::test]
    async fn failed_and_ambiguous_commands_after_the_watermark_carry_their_project() {
        let temp = TempDir::new().unwrap();
        let (store, project_id, assignment) = fixture(&temp).await;
        let prompt = |command_id: &str| SendAssignmentPrompt {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            attempt_id: assignment.attempt.id.clone(),
            expected_assignment_version: assignment.version,
            expected_attempt_version: assignment.attempt.version,
            text: "secret prompt text".to_owned(),
        };
        store
            .begin_assignment_prompt(
                &project_id,
                &assignment.id,
                prompt("prompt-ambiguous"),
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        store
            .fail_assignment_prompt("prompt-ambiguous", "binding changed", true)
            .await
            .unwrap();
        store
            .begin_assignment_prompt(
                &project_id,
                &assignment.id,
                prompt("prompt-failed"),
                TokenSpendCommandSource::Manual,
            )
            .await
            .unwrap();
        store
            .fail_assignment_prompt("prompt-failed", "herdr refused", false)
            .await
            .unwrap();
        let connection = connection(&temp);
        connection
            .execute(
                "UPDATE command_acknowledgements SET updated_at_unix_ms = 1000
                  WHERE id = 'prompt-ambiguous'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE command_acknowledgements SET updated_at_unix_ms = 2000
                  WHERE id = 'prompt-failed'",
                [],
            )
            .unwrap();

        let all = store.attention_records(0).await.unwrap().commands;
        assert_eq!(
            all.iter()
                .map(|command| (command.command_id.as_str(), command.status))
                .collect::<Vec<_>>(),
            vec![
                ("prompt-ambiguous", AttentionCommandStatus::Ambiguous),
                ("prompt-failed", AttentionCommandStatus::Failed),
            ]
        );
        assert_eq!(all[0].command_type, "assignment_prompt");
        assert_eq!(all[0].project_name.as_deref(), Some("Checkout <api>"));
        assert_eq!(
            all[0].assignment_id.as_deref(),
            Some(assignment.id.as_str())
        );
        assert_eq!(all[0].objective.as_deref(), Some("Ship the retry banner"));
        let later = store.attention_records(1000).await.unwrap().commands;
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].command_id, "prompt-failed");
        // The succeeded allocation command is never a record.
        assert!(
            all.iter()
                .all(|command| command.command_id != "attention-allocation")
        );
    }
}

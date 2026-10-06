//! Reconciliation keeps a worker bound across a provider session continuation
//! on its own terminal (Rule A of 7219078, re-implemented on the mainline
//! reconcile), and follows a unique provider session to a new terminal only
//! when the old topology is gone.

use super::*;

/// An active assignment whose worker (terminal-2, workspace-1) is bound to
/// `session`, beside the providerless project orchestrator (terminal-1).
async fn assigned_worker_with_provider_session(
    store: &SqliteProjectStore,
    session: &str,
) -> (String, String) {
    let (project_id, assignment) = create_active_assignment(store).await;
    store
        .reconcile_runtime_inventory(inventory(
            20,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-2",
                    "workspace-1",
                    "tab-2",
                    "pane-2",
                    Some(provider_session(session)),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();
    let runtime = worker_candidate(store, &assignment.worker.id)
        .await
        .worker
        .runtime
        .unwrap();
    assert_eq!(runtime.provider_session, Some(provider_session(session)));
    (project_id, assignment.worker.id)
}

async fn worker_candidate(
    store: &SqliteProjectStore,
    worker_id: &str,
) -> yard_domain::WorkerCandidate {
    store
        .list_worker_candidates()
        .await
        .unwrap()
        .workers
        .into_iter()
        .find(|candidate| candidate.worker.id == worker_id)
        .unwrap()
}

async fn runtime_of(store: &SqliteProjectStore, worker_id: &str) -> WorkerRuntimeBinding {
    worker_candidate(store, worker_id)
        .await
        .worker
        .runtime
        .unwrap()
}

fn orchestrator_session_terminal() -> ObservedWorker {
    observed_worker("terminal-1", "workspace-1", "tab-1", "pane-1", None)
}

fn worker_event_count(temp: &TempDir, worker_id: &str, event_type: &str) -> i64 {
    Connection::open(temp.path().join("yard.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM lifecycle_events
              WHERE aggregate_type = 'worker' AND aggregate_id = ?1
                AND event_type = ?2",
            params![worker_id, event_type],
            |row| row.get(0),
        )
        .unwrap()
}

fn worker_ids_bound_to(temp: &TempDir, terminal_id: &str) -> Vec<String> {
    let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
    let mut statement = connection
        .prepare("SELECT worker_id FROM worker_runtime_bindings WHERE terminal_id = ?1")
        .unwrap();
    statement
        .query_map([terminal_id], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn live_worker_count(temp: &TempDir) -> i64 {
    Connection::open(temp.path().join("yard.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM workers WHERE ended_at_unix_ms IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn bound_terminal_reports(observed_at_unix_ms: u64, session: Option<&str>) -> RuntimeInventory {
    inventory(
        observed_at_unix_ms,
        vec![
            orchestrator_session_terminal(),
            observed_worker(
                "terminal-2",
                "workspace-1",
                "tab-2",
                "pane-2",
                session.map(provider_session),
            ),
        ],
        Vec::new(),
    )
}

fn assert_bound_session_kept_ambiguous(runtime: &WorkerRuntimeBinding, session: &str) {
    assert_eq!(runtime.terminal_id, "terminal-2");
    assert_eq!(runtime.provider_session, Some(provider_session(session)));
    assert_eq!(
        runtime.observation_state,
        RuntimeObservationState::Ambiguous
    );
}

async fn exit_bound_agent(store: &SqliteProjectStore, observed_at_unix_ms: u64) {
    store
        .reconcile_runtime_inventory(inventory(
            observed_at_unix_ms,
            vec![orchestrator_session_terminal()],
            vec![observed_pane(
                "terminal-2",
                "workspace-1",
                "tab-2",
                "pane-2",
                None,
            )],
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn reconciliation_continues_compacted_provider_session_on_the_bound_terminal() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    let workers_before = live_worker_count(&temp);

    let result = store
        .reconcile_runtime_inventory(bound_terminal_reports(
            30,
            Some("generalist-compacted-session"),
        ))
        .await
        .unwrap();
    let candidate = worker_candidate(&store, &worker_id).await;
    let runtime = candidate.worker.runtime.unwrap();

    assert_eq!(result.ambiguous_bindings, 0);
    assert_eq!(result.adopted_workers, 0);
    assert_eq!(result.updated_bindings, 1);
    assert_eq!(candidate.availability, WorkerAvailability::Assigned);
    assert_eq!(runtime.terminal_id, "terminal-2");
    assert_eq!(runtime.pane_id, "pane-2");
    assert_eq!(
        runtime.provider_session,
        Some(provider_session("generalist-compacted-session"))
    );
    assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
    assert_eq!(runtime.process_state, RuntimeProcessState::Running);
    assert_eq!(
        worker_event_count(&temp, &worker_id, "runtime_provider_session_continued"),
        1
    );
    assert_eq!(live_worker_count(&temp), workers_before);
    assert_eq!(worker_ids_bound_to(&temp, "terminal-2"), vec![worker_id]);
}

#[tokio::test]
async fn reconciliation_does_not_follow_a_different_session_on_a_different_terminal() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;

    let result = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-3",
                    "workspace-1",
                    "tab-3",
                    "pane-3",
                    Some(provider_session("unrelated-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &worker_id).await;

    assert_eq!(result.missing_bindings, 1);
    assert_eq!(runtime.terminal_id, "terminal-2");
    assert_eq!(
        runtime.provider_session,
        Some(provider_session("generalist-session"))
    );
    assert_eq!(runtime.observation_state, RuntimeObservationState::Missing);
    assert_eq!(worker_ids_bound_to(&temp, "terminal-2"), vec![worker_id]);
    assert!(worker_ids_bound_to(&temp, "terminal-3").is_empty());
}

#[tokio::test]
async fn reconciliation_treats_provider_kind_change_on_the_bound_terminal_as_conflict() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    let mut claude = observed_worker(
        "terminal-2",
        "workspace-1",
        "tab-2",
        "pane-2",
        Some(ProviderSessionRef {
            source: "herdr:claude".to_owned(),
            provider: "claude".to_owned(),
            kind: "id".to_owned(),
            value: "claude-session".to_owned(),
        }),
    );
    claude.provider = Some("claude".to_owned());

    let result = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![orchestrator_session_terminal(), claude],
            Vec::new(),
        ))
        .await
        .unwrap();

    assert_eq!(result.ambiguous_bindings, 1);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
    assert_eq!(
        worker_event_count(&temp, &worker_id, "runtime_provider_session_continued"),
        0
    );
}

#[tokio::test]
async fn reconciliation_treats_workspace_change_with_new_session_as_conflict() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;

    let result = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-2",
                    "workspace-2",
                    "tab-2",
                    "pane-2",
                    Some(provider_session("generalist-compacted-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &worker_id).await;

    assert_eq!(result.ambiguous_bindings, 1);
    assert_eq!(runtime.workspace_id, "workspace-1");
    assert_bound_session_kept_ambiguous(&runtime, "generalist-session");
}

#[tokio::test]
async fn reconciliation_treats_new_session_with_tab_and_pane_change_as_conflict() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;

    let result = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-2",
                    "workspace-1",
                    "tab-7",
                    "pane-7",
                    Some(provider_session("generalist-compacted-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &worker_id).await;

    assert_eq!(result.ambiguous_bindings, 1);
    assert_bound_session_kept_ambiguous(&runtime, "generalist-session");
    assert_eq!(runtime.pane_id, "pane-2");
}

#[tokio::test]
async fn reconciliation_rejects_continuation_into_a_session_bound_to_another_worker() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    let orchestrator_id = store
        .get_project(&project_id)
        .await
        .unwrap()
        .orchestrator
        .id;
    store
        .reconcile_runtime_inventory(inventory(
            25,
            vec![
                observed_worker(
                    "terminal-1",
                    "workspace-1",
                    "tab-1",
                    "pane-1",
                    Some(provider_session("orchestrator-session")),
                ),
                observed_worker(
                    "terminal-2",
                    "workspace-1",
                    "tab-2",
                    "pane-2",
                    Some(provider_session("generalist-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();

    // The orchestrator's conversation is resumed inside the worker's pane.
    let result = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![observed_worker(
                "terminal-2",
                "workspace-1",
                "tab-2",
                "pane-2",
                Some(provider_session("orchestrator-session")),
            )],
            Vec::new(),
        ))
        .await
        .unwrap();
    let orchestrator_runtime = runtime_of(&store, &orchestrator_id).await;

    assert_eq!(result.ambiguous_bindings, 2);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
    assert_eq!(orchestrator_runtime.terminal_id, "terminal-1");
    assert_eq!(
        orchestrator_runtime.observation_state,
        RuntimeObservationState::Ambiguous
    );
}

#[tokio::test]
async fn reconciliation_leaves_continuation_ambiguous_when_the_terminal_is_quarantined() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO command_acknowledgements (
                id, command_type, actor, status, error_message,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES (
                'quarantine-command', 'profile_allocation', 'local-user',
                'failed', 'quarantined for test', 1, 1
             )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO quarantined_provisioning_runtime_bindings (
                command_id, adapter, runtime_session, runtime_workspace_id,
                terminal_id, tab_id, pane_id, owns_tab, observation_state,
                process_state, observed_status, state_change_sequence,
                runtime_revision, runtime_version, last_observed_at_unix_ms,
                captured_at_unix_ms
             ) VALUES (
                'quarantine-command', 'herdr', 'default', 'workspace-1',
                'terminal-2', 'tab-2', 'pane-2', 1, 'observed', 'running',
                'idle', 1, 1, 1, 1, 1
             )",
            [],
        )
        .unwrap();
    drop(connection);

    let result = store
        .reconcile_runtime_inventory(bound_terminal_reports(
            30,
            Some("generalist-compacted-session"),
        ))
        .await
        .unwrap();

    assert_eq!(result.ambiguous_bindings, 1);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
}

#[tokio::test]
async fn reconciliation_does_not_continue_into_a_new_agent_after_observed_exit() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    exit_bound_agent(&store, 30).await;
    assert_eq!(
        runtime_of(&store, &worker_id).await.process_state,
        RuntimeProcessState::Exited
    );

    // The user starts an unrelated conversation in the same pane.
    for observed_at in [40, 50] {
        let result = store
            .reconcile_runtime_inventory(bound_terminal_reports(
                observed_at,
                Some("unrelated-fresh-session"),
            ))
            .await
            .unwrap();
        assert_eq!(result.ambiguous_bindings, 1);
        let candidate = worker_candidate(&store, &worker_id).await;
        assert_eq!(candidate.availability, WorkerAvailability::Assigned);
        assert_bound_session_kept_ambiguous(
            &candidate.worker.runtime.unwrap(),
            "generalist-session",
        );
    }
    assert_eq!(
        worker_event_count(&temp, &worker_id, "runtime_provider_session_continued"),
        0
    );

    // Resuming the bound conversation in that pane restores the binding.
    store
        .reconcile_runtime_inventory(bound_terminal_reports(60, Some("generalist-session")))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &worker_id).await;
    assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
    assert_eq!(runtime.process_state, RuntimeProcessState::Running);
}

#[tokio::test]
async fn reconciliation_does_not_continue_after_exit_then_sessionless_relaunch() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    exit_bound_agent(&store, 30).await;

    // A relaunched agent appears before its hook reports any session.
    store
        .reconcile_runtime_inventory(bound_terminal_reports(40, None))
        .await
        .unwrap();
    let relaunched = runtime_of(&store, &worker_id).await;
    assert_bound_session_kept_ambiguous(&relaunched, "generalist-session");
    assert_eq!(relaunched.process_state, RuntimeProcessState::Exited);

    let result = store
        .reconcile_runtime_inventory(bound_terminal_reports(50, Some("unrelated-fresh-session")))
        .await
        .unwrap();
    assert_eq!(result.ambiguous_bindings, 1);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
}

#[tokio::test]
async fn reconciliation_does_not_continue_a_session_first_reported_on_an_agentless_pane() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    // A fresh agent's hook reports its session before Herdr detects the
    // agent, so the bound pane briefly shows no agent with a new session.
    store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![orchestrator_session_terminal()],
            vec![observed_pane(
                "terminal-2",
                "workspace-1",
                "tab-2",
                "pane-2",
                Some(provider_session("unrelated-fresh-session")),
            )],
        ))
        .await
        .unwrap();

    let result = store
        .reconcile_runtime_inventory(bound_terminal_reports(40, Some("unrelated-fresh-session")))
        .await
        .unwrap();
    assert_eq!(result.ambiguous_bindings, 1);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
}

#[tokio::test]
async fn reconciliation_does_not_continue_into_a_session_reported_on_two_terminals() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;

    let result = store
        .reconcile_runtime_inventory(inventory(
            40,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-2",
                    "workspace-1",
                    "tab-2",
                    "pane-2",
                    Some(provider_session("shared-new-session")),
                ),
                observed_worker(
                    "terminal-9",
                    "workspace-1",
                    "tab-9",
                    "pane-9",
                    Some(provider_session("shared-new-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();

    assert_eq!(result.ambiguous_bindings, 1);
    assert_eq!(result.adopted_workers, 0);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
    assert!(worker_ids_bound_to(&temp, "terminal-9").is_empty());
}

#[tokio::test]
async fn reconciliation_does_not_continue_while_the_old_session_is_on_another_terminal() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;

    let result = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-2",
                    "workspace-1",
                    "tab-2",
                    "pane-2",
                    Some(provider_session("generalist-forked-session")),
                ),
                observed_worker(
                    "terminal-9",
                    "workspace-1",
                    "tab-9",
                    "pane-9",
                    Some(provider_session("generalist-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();

    assert_eq!(result.ambiguous_bindings, 1);
    assert_eq!(result.adopted_workers, 0);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );
    assert!(worker_ids_bound_to(&temp, "terminal-9").is_empty());
}

/// The incident sequence: the Generalist compacts on its pane, then the host
/// reboots and Herdr restores the conversation in a new pane. The claimed
/// worker follows its (continued) session; no second worker appears.
#[tokio::test]
async fn reconciliation_rebinds_a_restored_conversation_after_a_continuation() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    store
        .reconcile_runtime_inventory(bound_terminal_reports(
            30,
            Some("generalist-compacted-session"),
        ))
        .await
        .unwrap();
    let workers_before = live_worker_count(&temp);

    let result = store
        .reconcile_runtime_inventory(inventory(
            40,
            vec![
                orchestrator_session_terminal(),
                observed_worker(
                    "terminal-9",
                    "workspace-1",
                    "tab-9",
                    "pane-9",
                    Some(provider_session("generalist-compacted-session")),
                ),
            ],
            Vec::new(),
        ))
        .await
        .unwrap();
    let candidate = worker_candidate(&store, &worker_id).await;
    let runtime = candidate.worker.runtime.unwrap();

    assert_eq!(result.ambiguous_bindings, 0);
    assert_eq!(result.adopted_workers, 0);
    assert_eq!(candidate.availability, WorkerAvailability::Assigned);
    assert_eq!(runtime.terminal_id, "terminal-9");
    assert_eq!(runtime.pane_id, "pane-9");
    assert_eq!(
        runtime.provider_session,
        Some(provider_session("generalist-compacted-session"))
    );
    assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
    assert_eq!(live_worker_count(&temp), workers_before);
    assert_eq!(worker_ids_bound_to(&temp, "terminal-9"), vec![worker_id]);
}

/// A live worker with no project allocation (nothing pins its workspace),
/// bound to `terminal-old` in workspace-1 with `moving-session`.
fn insert_unallocated_bound_worker(temp: &TempDir) -> String {
    let worker_id = "unallocated-worker".to_owned();
    let mut connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO workers (
                id, profile_id, profile_version, desired_state, version,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES (?1, NULL, NULL, 'running', 1, 1, 1)",
            [&worker_id],
        )
        .unwrap();
    let (_, mut runtime) = draft("workspace-1", "terminal-old");
    runtime.tab_id = Some("tab-old".to_owned());
    runtime.pane_id = "pane-old".to_owned();
    runtime.provider_session = Some(provider_session("moving-session"));
    runtime.last_observed_at_unix_ms = 20;
    insert_worker_runtime_binding_unchecked(&transaction, &worker_id, &runtime, 20).unwrap();
    transaction.commit().unwrap();
    worker_id
}

#[tokio::test]
async fn reconciliation_moves_provider_session_across_workspaces_only_after_old_pane_is_gone() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let worker_id = insert_unallocated_bound_worker(&temp);
    let moved = observed_worker(
        "terminal-new",
        "workspace-2",
        "tab-new",
        "pane-new",
        Some(provider_session("moving-session")),
    );

    let blocked = store
        .reconcile_runtime_inventory(inventory(
            30,
            vec![moved.clone()],
            vec![observed_pane(
                "terminal-old",
                "workspace-1",
                "tab-old",
                "pane-old",
                None,
            )],
        ))
        .await
        .unwrap();
    let blocked_runtime = runtime_of(&store, &worker_id).await;
    assert_eq!(blocked.ambiguous_bindings, 1);
    assert_eq!(blocked.adopted_workers, 0);
    assert_eq!(blocked_runtime.terminal_id, "terminal-old");
    assert_eq!(
        blocked_runtime.observation_state,
        RuntimeObservationState::Ambiguous
    );

    let moved_result = store
        .reconcile_runtime_inventory(inventory(40, vec![moved], Vec::new()))
        .await
        .unwrap();
    let moved_runtime = runtime_of(&store, &worker_id).await;
    assert_eq!(moved_result.ambiguous_bindings, 0);
    assert_eq!(moved_result.adopted_workers, 0);
    assert_eq!(moved_runtime.terminal_id, "terminal-new");
    assert_eq!(moved_runtime.workspace_id, "workspace-2");
    assert_eq!(
        moved_runtime.observation_state,
        RuntimeObservationState::Observed
    );
}

/// Gives the assigned worker a pane management lease on `pane-2` holding
/// Herdr pane instance `instance-a`.
fn lease_bound_pane(temp: &TempDir, project_id: &str, worker_id: &str) {
    let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT OR IGNORE INTO yard_installation (
                singleton_id, installation_uuid, created_at_unix_ms
             ) VALUES (1, 'installation-test', 1)",
            [],
        )
        .unwrap();
    let installation: String = connection
        .query_row(
            "SELECT installation_uuid FROM yard_installation WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO pane_management_leases (
                worker_id, project_id, allocation_id, installation_uuid,
                herdr_session, pane_id, pane_instance_id, owner_id,
                lease_token, expires_at_unix_ms, acquisition_request_id,
                status, renewal_failures, renew_after_unix_ms,
                last_error_code, created_at_unix_ms, updated_at_unix_ms
             )
             SELECT ?1, ?2, allocation.id, ?3, 'default', 'pane-2', 'instance-a',
                    'owner-test', 'token-test', 9999999999999, 'acquire-test',
                    'active', 0, 9999999999999, NULL, 1, 1
               FROM worker_allocations allocation
              WHERE allocation.worker_id = ?1 AND allocation.ended_at_unix_ms IS NULL",
            params![worker_id, project_id, installation],
        )
        .unwrap();
}

#[tokio::test]
async fn reconciliation_does_not_continue_when_the_managed_pane_instance_changed() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (project_id, worker_id) =
        assigned_worker_with_provider_session(&store, "generalist-session").await;
    lease_bound_pane(&temp, &project_id, &worker_id);
    let reported = |observed_at_unix_ms, instance: &str| {
        let mut worker = observed_worker(
            "terminal-2",
            "workspace-1",
            "tab-2",
            "pane-2",
            Some(provider_session("generalist-compacted-session")),
        );
        worker.pane_instance_id = Some(instance.to_owned());
        inventory(
            observed_at_unix_ms,
            vec![orchestrator_session_terminal(), worker],
            Vec::new(),
        )
    };

    let result = store
        .reconcile_runtime_inventory(reported(30, "instance-b"))
        .await
        .unwrap();
    assert_eq!(result.ambiguous_bindings, 1);
    assert_bound_session_kept_ambiguous(
        &runtime_of(&store, &worker_id).await,
        "generalist-session",
    );

    // The leased pane instance itself continues normally.
    let result = store
        .reconcile_runtime_inventory(reported(40, "instance-a"))
        .await
        .unwrap();
    assert_eq!(result.ambiguous_bindings, 0);
    let runtime = runtime_of(&store, &worker_id).await;
    assert_eq!(
        runtime.provider_session,
        Some(provider_session("generalist-compacted-session"))
    );
    assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
}

/// A project orchestrator replacement that has started its new runtime
/// (`session-replacement` on `terminal-replacement`) and is pending cutover,
/// while the displaced orchestrator (`session-displaced` on
/// `terminal-displaced`, tab-1, pane-1) is still bound.
async fn pending_replacement(
    store: &SqliteProjectStore,
) -> (Project, ReplaceProjectOrchestrator, WorkerRuntimeBinding) {
    let (project, _profile, command, prepared, started) = create_orchestrator_replacement_fixture(
        store,
        "replace-during-continuation",
        OldSessionDisposition::RetainForInspection,
    )
    .await;
    let begun = store
        .begin_project_orchestrator_replacement(&project.id, command.clone())
        .await
        .unwrap();
    assert!(matches!(
        begun,
        BeginProjectOrchestratorReplacement::Started(_)
    ));
    store
        .claim_project_orchestrator_replacement_runtime(&command.command_id, prepared)
        .await
        .unwrap();
    store
        .record_project_orchestrator_replacement_started_runtime(
            &command.command_id,
            started.clone(),
            OrchestratorReplacementStartEvidence::Confirmed,
        )
        .await
        .unwrap();
    (project, command, started)
}

fn displaced_terminal_reports(session: &str) -> ObservedWorker {
    observed_worker(
        "terminal-displaced",
        "workspace-replacement",
        "tab-1",
        "pane-1",
        Some(provider_session(session)),
    )
}

#[tokio::test]
async fn reconciliation_does_not_continue_into_a_pending_replacement_session() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (project, _command, _started) = pending_replacement(&store).await;

    // The replacement's session is reported on the displaced terminal only.
    let result = store
        .reconcile_runtime_inventory(inventory(
            60,
            vec![displaced_terminal_reports("session-replacement")],
            Vec::new(),
        ))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &project.orchestrator.id).await;

    assert_eq!(result.ambiguous_bindings, 1);
    assert_eq!(runtime.terminal_id, "terminal-displaced");
    assert_eq!(
        runtime.provider_session,
        Some(provider_session("session-displaced"))
    );
    assert_eq!(
        runtime.observation_state,
        RuntimeObservationState::Ambiguous
    );
}

/// A compaction of the displaced orchestrator while its replacement is
/// pending continues the live binding, but the replacement still compares
/// against the runtime it captured when it began, so cutover onto the changed
/// orchestrator is refused instead of treating the conversation as gone.
#[tokio::test]
async fn compaction_during_a_pending_orchestrator_replacement_keeps_the_capture() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (project, command, started) = pending_replacement(&store).await;
    let mut replacement = observed_worker(
        "terminal-replacement",
        "workspace-replacement",
        "tab-terminal-replacement",
        "pane-terminal-replacement",
        Some(provider_session("session-replacement")),
    );
    replacement.interactive_ready = true;

    let result = store
        .reconcile_runtime_inventory(inventory(
            60,
            vec![
                displaced_terminal_reports("session-displaced-compacted"),
                replacement,
            ],
            Vec::new(),
        ))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &project.orchestrator.id).await;

    assert_eq!(result.ambiguous_bindings, 0);
    assert_eq!(result.adopted_workers, 0);
    assert_eq!(runtime.terminal_id, "terminal-displaced");
    assert_eq!(
        runtime.provider_session,
        Some(provider_session("session-displaced-compacted"))
    );
    assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
    assert_eq!(
        command.expected_orchestrator_runtime.provider_session,
        Some(provider_session("session-displaced"))
    );
    assert!(worker_ids_bound_to(&temp, "terminal-replacement").is_empty());

    let mut observed_started = started;
    observed_started.last_observed_at_unix_ms = 60;
    let error = store
        .finalize_project_orchestrator_replacement(&command.command_id, observed_started)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            ProjectStoreError::OrchestratorReplacementTargetChanged
        ),
        "{error:?}"
    );
    assert_eq!(
        store
            .get_project(&project.id)
            .await
            .unwrap()
            .orchestrator
            .id,
        project.orchestrator.id
    );
}

// Session ids taken from pane command lines (2fd4e60). `HerdrInventorySource`
// enriches the inventory before reconciliation; these tests apply the same
// precedence and check that the store's rules then bind the right worker.

const COMMAND_LINE_Y: &str = "01a0b10f-4a75-7841-8e1d-aa6f12919c45";
const COMMAND_LINE_X: &str = "01a0eb18-71ea-7c2b-9d3e-0123456789ab";

/// What `HerdrInventorySource` does before reconciliation: parse each pane's
/// foreground argv and merge the ids with the command-line precedence (Herdr
/// missing -> command line; Herdr disagrees -> command line for Codex unless
/// the same id also runs on another terminal). `lines` are
/// `(terminal_id, pane_id, foreground argv)`.
fn with_foreground_command_lines(
    mut inventory: RuntimeInventory,
    lines: &[(&str, &str, &str)],
) -> RuntimeInventory {
    let derived: Vec<_> = lines
        .iter()
        .filter_map(|(terminal_id, pane_id, line)| {
            let job = yard_domain::PaneForegroundJob {
                pane_id: (*pane_id).to_owned(),
                process_group_id: Some(4242),
                processes: vec![yard_domain::ForegroundProcess {
                    pid: 4242,
                    name: "codex".to_owned(),
                    argv: Some(line.split_whitespace().map(str::to_owned).collect()),
                }],
            };
            Some(yard_domain::CommandLineProviderSession {
                terminal_id: (*terminal_id).to_owned(),
                pane_id: (*pane_id).to_owned(),
                provider: "codex".to_owned(),
                value: yard_domain::provider_session_from_foreground("codex", &job)?,
            })
        })
        .collect();
    assert_eq!(derived.len(), lines.len(), "every test line must parse");
    yard_domain::apply_command_line_provider_sessions(&mut inventory, &derived);
    inventory
}

#[tokio::test]
async fn reconciliation_reattaches_an_agent_relaunched_in_a_new_pane_from_its_command_line() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, COMMAND_LINE_Y).await;
    // After a restart the old terminal is gone and `codex resume Y` runs in a
    // new pane that Herdr did not launch, so it reports no session.
    let relaunched = inventory(
        30,
        vec![
            orchestrator_session_terminal(),
            observed_worker("terminal-5", "workspace-1", "tab-5", "pane-5", None),
        ],
        Vec::new(),
    );

    let result = store
        .reconcile_runtime_inventory(with_foreground_command_lines(
            relaunched,
            &[(
                "terminal-5",
                "pane-5",
                &format!("/usr/local/bin/codex resume --yolo {COMMAND_LINE_Y}"),
            )],
        ))
        .await
        .unwrap();
    let candidate = worker_candidate(&store, &worker_id).await;
    let runtime = candidate.worker.runtime.unwrap();

    assert_eq!(result.adopted_workers, 0);
    assert_eq!(result.ambiguous_bindings, 0);
    assert_eq!(candidate.availability, WorkerAvailability::Assigned);
    assert_eq!(runtime.terminal_id, "terminal-5");
    assert_eq!(runtime.pane_id, "pane-5");
    assert_eq!(
        runtime.provider_session,
        Some(provider_session(COMMAND_LINE_Y))
    );
    assert_eq!(runtime.observation_state, RuntimeObservationState::Observed);
    assert_eq!(worker_ids_bound_to(&temp, "terminal-5"), vec![worker_id]);
}

/// A live worker with no allocation, bound to `terminal-3` (workspace-1) with
/// session X. Mainline no longer auto-adopts, so it is inserted directly.
fn insert_worker_bound_to_x(temp: &TempDir) -> String {
    let worker_id = "worker-x".to_owned();
    let mut connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO workers (
                id, profile_id, profile_version, desired_state, version,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES (?1, NULL, NULL, 'running', 1, 25, 25)",
            [&worker_id],
        )
        .unwrap();
    let (_, mut runtime) = draft("workspace-1", "terminal-3");
    runtime.tab_id = Some("tab-3".to_owned());
    runtime.pane_id = "pane-3".to_owned();
    runtime.provider_session = Some(provider_session(COMMAND_LINE_X));
    runtime.last_observed_at_unix_ms = 25;
    insert_worker_runtime_binding_unchecked(&transaction, &worker_id, &runtime, 25).unwrap();
    transaction.commit().unwrap();
    worker_id
}

/// The assigned worker holds Y on terminal-2; a second worker holds X on
/// terminal-3. Returns `(worker_y, worker_x)`.
async fn workers_bound_to_y_and_x(store: &SqliteProjectStore, temp: &TempDir) -> (String, String) {
    let (_project_id, worker_y) =
        assigned_worker_with_provider_session(store, COMMAND_LINE_Y).await;
    let worker_x = insert_worker_bound_to_x(temp);
    assert_eq!(
        worker_ids_bound_to(temp, "terminal-3"),
        vec![worker_x.clone()]
    );
    (worker_y, worker_x)
}

#[tokio::test]
async fn reconciliation_prefers_command_lines_when_herdr_reports_a_session_on_the_wrong_pane() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (worker_y, worker_x) = workers_bound_to_y_and_x(&store, &temp).await;
    // The shared Codex daemon reported X's session start for pane-2, so
    // Herdr now shows X on pane-2 and nothing on pane-3.
    let misreported = inventory(
        30,
        vec![
            orchestrator_session_terminal(),
            observed_worker(
                "terminal-2",
                "workspace-1",
                "tab-2",
                "pane-2",
                Some(provider_session(COMMAND_LINE_X)),
            ),
            observed_worker("terminal-3", "workspace-1", "tab-3", "pane-3", None),
        ],
        Vec::new(),
    );

    let result = store
        .reconcile_runtime_inventory(with_foreground_command_lines(
            misreported,
            &[
                (
                    "terminal-2",
                    "pane-2",
                    &format!("codex resume {COMMAND_LINE_Y}"),
                ),
                (
                    "terminal-3",
                    "pane-3",
                    &format!("codex resume {COMMAND_LINE_X}"),
                ),
            ],
        ))
        .await
        .unwrap();
    let runtime_y = runtime_of(&store, &worker_y).await;
    let runtime_x = runtime_of(&store, &worker_x).await;

    assert_eq!(result.ambiguous_bindings, 0);
    assert_eq!(result.adopted_workers, 0);
    assert_eq!(
        (runtime_y.terminal_id.as_str(), runtime_y.provider_session),
        ("terminal-2", Some(provider_session(COMMAND_LINE_Y)))
    );
    assert_eq!(
        runtime_y.observation_state,
        RuntimeObservationState::Observed
    );
    assert_eq!(
        (runtime_x.terminal_id.as_str(), runtime_x.provider_session),
        ("terminal-3", Some(provider_session(COMMAND_LINE_X)))
    );
    assert_eq!(
        runtime_x.observation_state,
        RuntimeObservationState::Observed
    );
    assert_eq!(
        worker_event_count(&temp, &worker_y, "runtime_provider_session_continued"),
        0
    );
}

#[tokio::test]
async fn reconciliation_rebinds_the_worker_named_by_the_command_line_not_by_herdr() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (worker_y, worker_x) = workers_bound_to_y_and_x(&store, &temp).await;
    // After a restart both old terminals are gone. `codex resume Y` runs in
    // a new pane, and Herdr misattributes X to it.
    let restarted = inventory(
        30,
        vec![
            orchestrator_session_terminal(),
            observed_worker(
                "terminal-9",
                "workspace-1",
                "tab-9",
                "pane-9",
                Some(provider_session(COMMAND_LINE_X)),
            ),
        ],
        Vec::new(),
    );

    let result = store
        .reconcile_runtime_inventory(with_foreground_command_lines(
            restarted,
            &[(
                "terminal-9",
                "pane-9",
                &format!("codex resume {COMMAND_LINE_Y}"),
            )],
        ))
        .await
        .unwrap();
    let runtime_y = runtime_of(&store, &worker_y).await;
    let runtime_x = runtime_of(&store, &worker_x).await;

    assert_eq!(result.adopted_workers, 0);
    assert_eq!(runtime_y.terminal_id, "terminal-9");
    assert_eq!(
        runtime_y.provider_session,
        Some(provider_session(COMMAND_LINE_Y))
    );
    assert_eq!(
        runtime_y.observation_state,
        RuntimeObservationState::Observed
    );
    assert_eq!(runtime_x.terminal_id, "terminal-3");
    assert_eq!(
        runtime_x.observation_state,
        RuntimeObservationState::Missing
    );
    assert_eq!(worker_ids_bound_to(&temp, "terminal-9"), vec![worker_y]);
}

#[tokio::test]
async fn reconciliation_does_not_let_a_stale_command_line_orphan_the_pane_running_it() {
    const COMMAND_LINE_Z: &str = "01a0c2d4-5b6e-7f80-9a1b-2c3d4e5f6071";
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_project_id, worker_id) =
        assigned_worker_with_provider_session(&store, COMMAND_LINE_Y).await;
    let stale_line = format!("codex resume {COMMAND_LINE_Y}");
    let switched = || {
        observed_worker(
            "terminal-2",
            "workspace-1",
            "tab-2",
            "pane-2",
            Some(provider_session(COMMAND_LINE_Z)),
        )
    };
    // `codex resume Y` on terminal-2 switched in-process to Z; Herdr says Z.
    store
        .reconcile_runtime_inventory(with_foreground_command_lines(
            inventory(
                30,
                vec![orchestrator_session_terminal(), switched()],
                Vec::new(),
            ),
            &[("terminal-2", "pane-2", &stale_line)],
        ))
        .await
        .unwrap();
    // Then `codex resume Y` legitimately runs in a new pane terminal-6.
    let result = store
        .reconcile_runtime_inventory(with_foreground_command_lines(
            inventory(
                31,
                vec![
                    orchestrator_session_terminal(),
                    switched(),
                    observed_worker("terminal-6", "workspace-1", "tab-6", "pane-6", None),
                ],
                Vec::new(),
            ),
            &[
                ("terminal-2", "pane-2", &stale_line),
                ("terminal-6", "pane-6", &stale_line),
            ],
        ))
        .await
        .unwrap();
    let runtime = runtime_of(&store, &worker_id).await;

    // Codex argv on terminal-2 is stale (Y now runs on terminal-6), so
    // Herdr's Z is kept there. The binding cannot silently stay Observed
    // on a pane that runs Z; the conflict is flagged instead.
    assert_eq!(result.adopted_workers, 0);
    assert_eq!(result.ambiguous_bindings, 1);
    assert_eq!(runtime.terminal_id, "terminal-2");
    assert_eq!(
        runtime.observation_state,
        RuntimeObservationState::Ambiguous
    );
}

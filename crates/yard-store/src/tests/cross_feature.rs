//! Mainline worker cleanup and summary workers meet our archive, delete,
//! disposition and transcript features (S18 of the mainline rebase).

use super::*;

fn cleanup_listed_workers(dashboard: &yard_domain::WorkerCleanupDashboard) -> Vec<String> {
    dashboard
        .preview
        .candidates
        .iter()
        .map(|candidate| candidate.worker_id.clone())
        .collect()
}

async fn cleanup_lists(store: &SqliteProjectStore, worker_id: &str) -> bool {
    let dashboard = store.get_worker_cleanup_dashboard(50, 20).await.unwrap();
    cleanup_listed_workers(&dashboard).contains(&worker_id.to_owned())
}

async fn preview_lists(store: &SqliteProjectStore, worker_id: &str) -> bool {
    store
        .preview_completed_runtime_cleanup(100)
        .await
        .unwrap()
        .candidates
        .iter()
        .any(|candidate| candidate.worker_id == worker_id)
}

async fn execute(store: &SqliteProjectStore, sql: &'static str, values: Vec<String>) {
    store
        .run(move |connection| {
            connection.execute(sql, rusqlite::params_from_iter(values))?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn start_cleanup_run(store: &SqliteProjectStore, command_id: &str) -> usize {
    store
        .start_worker_cleanup_run(
            WorkerCleanupRunTrigger::Manual,
            StartWorkerCleanupRun {
                command_id: command_id.to_owned(),
                actor: "cleanup-test".to_owned(),
                preview: false,
            },
        )
        .await
        .unwrap()
        .candidate_count
}

#[tokio::test]
async fn worker_cleanup_waits_for_a_pending_transcript_capture() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_, assignment, _) = create_eligible_cleanup_candidate(&store, "capture").await;
    let worker_id = assignment.worker.id.clone();
    assert!(cleanup_lists(&store, &worker_id).await);

    // The completion's capture has not been read yet: closing the pane now
    // would lose the transcript, so cleanup does not consider the worker.
    execute(
        &store,
        "UPDATE transcript_capture_jobs
            SET status = 'pending', completed_at_unix_ms = NULL
          WHERE worker_id = ?1",
        vec![worker_id.clone()],
    )
    .await;
    assert!(!cleanup_lists(&store, &worker_id).await);
    assert_eq!(start_cleanup_run(&store, "capture-pending-run").await, 0);

    execute(
        &store,
        "UPDATE transcript_capture_jobs
            SET status = 'succeeded', completed_at_unix_ms = 1
          WHERE worker_id = ?1",
        vec![worker_id.clone()],
    )
    .await;
    assert!(cleanup_lists(&store, &worker_id).await);
    assert_eq!(start_cleanup_run(&store, "capture-settled-run").await, 1);
}

#[tokio::test]
async fn worker_cleanup_and_preview_skip_workers_of_archived_projects() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (project_id, assignment, _) = create_eligible_cleanup_candidate(&store, "archived").await;
    let worker_id = assignment.worker.id.clone();
    assert!(cleanup_lists(&store, &worker_id).await);
    assert!(preview_lists(&store, &worker_id).await);

    let preview = store
        .project_disposition_preview(&project_id)
        .await
        .unwrap();
    store
        .archive_project(
            &project_id,
            cancel_archive_command(&preview, "archive-cleanup-project"),
        )
        .await
        .unwrap();
    assert!(!worker_ended(&temp, &worker_id));
    assert!(!cleanup_lists(&store, &worker_id).await);
    assert!(!preview_lists(&store, &worker_id).await);
    assert_eq!(start_cleanup_run(&store, "archived-run").await, 0);
}

#[tokio::test]
async fn worker_cleanup_and_preview_skip_tombstoned_workers() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (_, assignment, _) = create_eligible_cleanup_candidate(&store, "tombstone").await;
    let worker_id = assignment.worker.id.clone();
    assert!(cleanup_lists(&store, &worker_id).await);

    // A tombstone is final even if the worker row still looks live.
    execute(
        &store,
        "INSERT INTO command_acknowledgements (
            id, command_type, actor, status, error_message,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES ('tombstone-delete', 'worker_delete', 'local-user', 'succeeded', NULL, 1, 1)",
        Vec::new(),
    )
    .await;
    execute(
        &store,
        "INSERT INTO deleted_workers (
            worker_id, command_id, expected_worker_version, deleted_at_unix_ms
         ) VALUES (?1, 'tombstone-delete', 1, 1)",
        vec![worker_id.clone()],
    )
    .await;
    assert!(!cleanup_lists(&store, &worker_id).await);
    assert!(!preview_lists(&store, &worker_id).await);
    assert_eq!(start_cleanup_run(&store, "tombstone-run").await, 0);
}

#[tokio::test]
async fn worker_cleanup_and_preview_skip_workers_of_archived_coordination_nodes() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let (project_id, assignment, _) = create_eligible_cleanup_candidate(&store, "node").await;
    let worker_id = assignment.worker.id.clone();
    let node_id = Uuid::now_v7().to_string();
    store
        .create_coordination_node(
            &node_id,
            Some(
                temp.path()
                    .join("coordination")
                    .join(&node_id)
                    .to_string_lossy()
                    .into_owned(),
            ),
            None,
            create_node_command(
                "cleanup-node-create",
                CoordinationNodeKind::Workstream,
                vec![project_id],
            ),
        )
        .await
        .unwrap();
    execute(
        &store,
        "UPDATE coordination_nodes SET worker_id = ?1 WHERE id = ?2",
        vec![worker_id.clone(), node_id.clone()],
    )
    .await;
    // A live node's worker is still listed, retained as protected.
    assert!(cleanup_lists(&store, &worker_id).await);
    assert!(preview_lists(&store, &worker_id).await);

    execute(
        &store,
        "INSERT INTO command_acknowledgements (
            id, command_type, actor, status, error_message,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES ('cleanup-node-archive', 'coordination_node_archive', 'local-user',
                   'succeeded', NULL, 1, 1)",
        Vec::new(),
    )
    .await;
    execute(
        &store,
        "INSERT INTO archived_coordination_nodes (
            command_id, node_id, actor, expected_node_version,
            expected_worker_version, result_node_version, worker_id,
            result_worker_version, archived_at_unix_ms
         ) VALUES ('cleanup-node-archive', ?1, 'local-user', 1, 1, 2, ?2, 2, 1)",
        vec![node_id, worker_id.clone()],
    )
    .await;
    assert!(!cleanup_lists(&store, &worker_id).await);
    assert!(!preview_lists(&store, &worker_id).await);
}

/// Our disposition guard (S3) refuses to complete or cancel summary work, and
/// our archive (S7) ends it: together the summary worker leaves through the
/// archive alone, its command fails, and mainline cleanup never sees it.
#[tokio::test]
async fn archive_ends_running_summary_work_that_disposition_refuses() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let summary = create_summary_fixture(&store, "cross-archive").await;
    let project_id = summary.project.id.clone();
    let worker_id = summary.assignment.worker.id.clone();
    let assignment = listed_assignment(&store, &project_id, &summary.assignment.id).await;
    assert!(matches!(
        store
            .dispose_assignment(
                &project_id,
                &assignment.id,
                disposition_command(
                    "cross-summary-cancel",
                    &assignment,
                    yard_domain::DispositionOutcome::Cancelled,
                    true,
                ),
                yard_domain::RequestOrigin::Browser,
            )
            .await,
        Err(ProjectStoreError::SystemEphemeralWorker)
    ));
    assert_eq!(
        summary_command_state(&temp, &summary.command.command_id),
        ("active".to_owned(), None)
    );

    let preview = store
        .project_disposition_preview(&project_id)
        .await
        .unwrap();
    assert!(preview.active_assignments.is_empty());
    assert_eq!(preview.summary_worker_assignments.len(), 1);
    store
        .archive_project(
            &project_id,
            cancel_archive_command(&preview, "cross-summary-archive"),
        )
        .await
        .unwrap();
    assert_eq!(
        summary_command_state(&temp, &summary.command.command_id),
        ("failed".to_owned(), Some("project_archived".to_owned()))
    );
    assert!(worker_ended(&temp, &worker_id));
    assert!(!cleanup_lists(&store, &worker_id).await);
    assert!(!preview_lists(&store, &worker_id).await);
    assert_eq!(start_cleanup_run(&store, "cross-summary-run").await, 0);
    // The summary assignment ends cancelled, not completed.
    let cancelled = listed_assignment(&store, &project_id, &summary.assignment.id).await;
    assert_eq!(cancelled.lifecycle, AssignmentLifecycle::Cancelled);
}

/// A summary worker's guarded cleanup item is revalidated right before its
/// pane closes: a tombstone, a node archive or a project archive that lands
/// in between stops it.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn worker_cleanup_close_revalidates_tombstones_and_archives() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let fixture = create_summary_fixture(&store, "revalidate").await;
    register_summary_artifact(
        &store,
        &fixture,
        &fixture.command.artifact_id,
        ArtifactKind::Markdown,
    )
    .await;
    store
        .record_completion_receipt(
            &fixture.project.id,
            &fixture.assignment.id,
            summary_receipt(
                &fixture,
                "revalidate-receipt",
                vec![fixture.command.artifact_id.clone()],
            ),
        )
        .await
        .unwrap();
    let handoff = store
        .start_summary_worker_handoff(
            &fixture.project.id,
            &fixture.project.orchestrator.id,
            &fixture.assignment.id,
            receive_summary(&fixture, "revalidate-handoff"),
            Some("child-instance".to_owned()),
        )
        .await
        .unwrap();
    let item = store
        .claim_worker_cleanup_items(handoff.summary.cleanup_run_id.as_deref(), 1, 10_000)
        .await
        .unwrap()
        .pop()
        .unwrap();
    store.authorize_worker_cleanup_close(&item).await.unwrap();

    let worker_id = fixture.assignment.worker.id.clone();
    execute(
        &store,
        "INSERT INTO command_acknowledgements (
            id, command_type, actor, status, error_message,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES ('revalidate-delete', 'worker_delete', 'local-user', 'succeeded', NULL, 1, 1)",
        Vec::new(),
    )
    .await;
    execute(
        &store,
        "INSERT INTO deleted_workers (
            worker_id, command_id, expected_worker_version, deleted_at_unix_ms
         ) VALUES (?1, 'revalidate-delete', 1, 1)",
        vec![worker_id.clone()],
    )
    .await;
    assert!(matches!(
        store.authorize_worker_cleanup_close(&item).await,
        Err(ProjectStoreError::WorkerCleanupRevalidationFailed)
    ));
    execute(
        &store,
        "DELETE FROM deleted_workers WHERE worker_id = ?1",
        vec![worker_id.clone()],
    )
    .await;
    store.authorize_worker_cleanup_close(&item).await.unwrap();

    let node_id = Uuid::now_v7().to_string();
    store
        .create_coordination_node(
            &node_id,
            Some(
                temp.path()
                    .join("coordination")
                    .join(&node_id)
                    .to_string_lossy()
                    .into_owned(),
            ),
            None,
            create_node_command(
                "revalidate-node-create",
                CoordinationNodeKind::Workstream,
                vec![fixture.project.id.clone()],
            ),
        )
        .await
        .unwrap();
    execute(
        &store,
        "INSERT INTO command_acknowledgements (
            id, command_type, actor, status, error_message,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES ('revalidate-node-archive', 'coordination_node_archive', 'local-user',
                   'succeeded', NULL, 1, 1)",
        Vec::new(),
    )
    .await;
    execute(
        &store,
        "INSERT INTO archived_coordination_nodes (
            command_id, node_id, actor, expected_node_version,
            expected_worker_version, result_node_version, worker_id,
            result_worker_version, archived_at_unix_ms
         ) VALUES ('revalidate-node-archive', ?1, 'local-user', 1, 1, 2, ?2, 2, 1)",
        vec![node_id, worker_id],
    )
    .await;
    assert!(matches!(
        store.authorize_worker_cleanup_close(&item).await,
        Err(ProjectStoreError::WorkerCleanupRevalidationFailed)
    ));
    execute(
        &store,
        "DELETE FROM archived_coordination_nodes WHERE command_id = 'revalidate-node-archive'",
        Vec::new(),
    )
    .await;
    store.authorize_worker_cleanup_close(&item).await.unwrap();

    // A real archive also stops it (it ends the worker as well).
    let preview = store
        .project_disposition_preview(&fixture.project.id)
        .await
        .unwrap();
    store
        .archive_project(
            &fixture.project.id,
            cancel_archive_command(&preview, "revalidate-archive"),
        )
        .await
        .unwrap();
    assert!(matches!(
        store.authorize_worker_cleanup_close(&item).await,
        Err(ProjectStoreError::WorkerCleanupRevalidationFailed)
    ));
}

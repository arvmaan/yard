use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;
use yard_domain::{
    Artifact, ArtifactKind, AssignmentLifecycle, ConfirmedAllocation, ReceiveSummaryWorker,
    RequestSummaryWorker, RuntimeObservationState, RuntimeProcessState,
    SummaryParentRuntimeCapture, SummaryWorker, SummaryWorkerState, SummaryWorkers,
};

use super::{
    BeginSummaryWorker, PreparedSummaryWorkerHandoff, ProjectStoreError, SqliteProjectStore,
    command_id_exists, insert_lifecycle_event, select_assignment_record, select_completion_receipt,
    select_current_profile_version, select_project, select_worker_profile_revision, to_i64,
    unix_time_ms,
};

#[allow(clippy::too_many_lines)]
pub(super) async fn begin(
    store: &SqliteProjectStore,
    project_id: &str,
    command: RequestSummaryWorker,
    capture: SummaryParentRuntimeCapture,
) -> Result<BeginSummaryWorker, ProjectStoreError> {
    let project_id = project_id.trim().to_owned();
    let command = command.normalize()?;
    validate_capture(&capture)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_by_command(&transaction, &command.command_id)? {
                if existing.project_id == project_id
                    && existing.parent_worker_id == command.parent_worker_id
                    && existing.profile_id == command.profile_id
                    && existing.profile_version == command.expected_profile_version
                    && existing.expected_artifact_id == command.artifact_id
                    && existing.objective == command.objective
                {
                    return Ok(BeginSummaryWorker::Replayed(Box::new(existing)));
                }
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            let project = select_project(&transaction, &project_id)?;
            if project.version != command.expected_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: project.version,
                });
            }
            if project.orchestrator.id != command.parent_worker_id {
                return Err(ProjectStoreError::SummaryParentNotCurrent);
            }
            if project.orchestrator.version != command.expected_parent_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: project.orchestrator.version,
                });
            }
            let runtime = project
                .orchestrator
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::SummaryParentRuntimeUnverified)?;
            if runtime.observation_state != RuntimeObservationState::Observed
                || runtime.process_state != RuntimeProcessState::Running
                || runtime.adapter != capture.adapter
                || runtime.session != capture.session
                || runtime.workspace_id != capture.workspace_id
                || runtime.terminal_id != capture.terminal_id
                || runtime.tab_id.as_deref() != Some(capture.tab_id.as_str())
                || runtime.pane_id != capture.pane_id
                || runtime.provider_session != capture.provider_session
            {
                return Err(ProjectStoreError::SummaryParentRuntimeUnverified);
            }
            let current_profile =
                select_current_profile_version(&transaction, &command.profile_id)?;
            if current_profile != command.expected_profile_version {
                return Err(ProjectStoreError::ProfileVersionConflict {
                    current_version: current_profile,
                });
            }
            let profile = select_worker_profile_revision(
                &transaction,
                &command.profile_id,
                command.expected_profile_version,
            )?;
            if profile.spec.runtime_adapter != capture.adapter {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO summary_worker_commands (
                    command_id, actor, project_id, parent_worker_id,
                    expected_parent_worker_version, expected_project_version,
                    profile_id, profile_version, expected_artifact_id, objective, captured_adapter,
                    captured_runtime_session, captured_workspace_id, captured_terminal_id,
                    captured_tab_id, captured_pane_id, captured_pane_instance_id,
                    captured_provider_session_json, status, created_at_unix_ms,
                    updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, 'allocating', ?19, ?19
                 )",
                params![
                    command.command_id,
                    command.actor,
                    project_id,
                    command.parent_worker_id,
                    to_i64(command.expected_parent_worker_version)?,
                    to_i64(command.expected_project_version)?,
                    command.profile_id,
                    to_i64(command.expected_profile_version)?,
                    command.artifact_id,
                    command.objective,
                    capture.adapter,
                    capture.session,
                    capture.workspace_id,
                    capture.terminal_id,
                    capture.tab_id,
                    capture.pane_id,
                    capture.pane_instance_id,
                    capture
                        .provider_session
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    to_i64(now)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "summary_worker",
                &command.command_id,
                1,
                "summary_worker_requested",
                &command.actor,
                now,
            )?;
            transaction.commit()?;
            Ok(BeginSummaryWorker::Started)
        })
        .await
}

pub(super) async fn complete(
    store: &SqliteProjectStore,
    command_id: &str,
    allocation: ConfirmedAllocation,
) -> Result<SummaryWorker, ProjectStoreError> {
    let command_id = command_id.trim().to_owned();
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let summary = select_by_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::SummaryWorkerNotFound)?;
            if summary.assignment.is_some() {
                if summary.assignment.as_ref().map(|value| value.id.as_str())
                    == Some(allocation.assignment.id.as_str())
                {
                    return Ok(summary);
                }
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            let runtime = allocation
                .assignment
                .worker
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            if allocation.command_id != command_id
                || allocation.assignment.project_id != summary.project_id
                || allocation.assignment.profile_id != summary.profile_id
                || allocation.assignment.profile_version != summary.profile_version
                || allocation.assignment.role != "summary_worker"
                || runtime.workspace_id != summary.captured_workspace_id
                || runtime.tab_id.is_none()
            {
                return Err(ProjectStoreError::SummaryWorkerAllocationMismatch);
            }
            let parent_worker_id = summary.parent_worker_id.clone();
            let worker_rows = transaction.execute(
                "UPDATE workers
                    SET ownership_kind = 'system_ephemeral', parent_worker_id = ?1
                  WHERE id = ?2 AND ownership_kind = 'yard_owned'
                    AND parent_worker_id IS NULL AND ended_at_unix_ms IS NULL",
                params![parent_worker_id, allocation.assignment.worker.id],
            )?;
            if worker_rows != 1 {
                return Err(ProjectStoreError::SummaryWorkerAllocationMismatch);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE summary_worker_commands
                    SET status = 'active', result_worker_id = ?1,
                        result_assignment_id = ?2, updated_at_unix_ms = ?3
                  WHERE command_id = ?4 AND status = 'allocating'",
                params![
                    allocation.assignment.worker.id,
                    allocation.assignment.id,
                    to_i64(now)?,
                    command_id
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "summary_worker",
                &command_id,
                2,
                "summary_worker_started",
                "yard",
                now,
            )?;
            let result = select_by_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::SummaryWorkerNotFound)?;
            transaction.commit()?;
            Ok(result)
        })
        .await
}

pub(super) async fn fail(
    store: &SqliteProjectStore,
    command_id: &str,
    message: &str,
) -> Result<(), ProjectStoreError> {
    let command_id = command_id.trim().to_owned();
    let message = message.trim().to_owned();
    if message.is_empty() {
        return Err(ProjectStoreError::CommandFailureMessageRequired);
    }
    store
        .run(move |connection| {
            let now = unix_time_ms()?;
            let changed = connection.execute(
                "UPDATE summary_worker_commands
                    SET status = CASE WHEN result_assignment_id IS NULL THEN 'failed' ELSE status END,
                        error_message = ?1, updated_at_unix_ms = ?2
                  WHERE command_id = ?3",
                params![message, to_i64(now)?, command_id],
            )?;
            if changed == 0 {
                return Err(ProjectStoreError::SummaryWorkerNotFound);
            }
            Ok(())
        })
        .await
}

pub(super) async fn list(
    store: &SqliteProjectStore,
    project_id: &str,
    parent_worker_id: &str,
) -> Result<SummaryWorkers, ProjectStoreError> {
    let project_id = project_id.trim().to_owned();
    let parent_worker_id = parent_worker_id.trim().to_owned();
    store
        .run(move |connection| {
            let mut statement = connection.prepare(
                "SELECT command_id FROM summary_worker_commands
                  WHERE project_id = ?1 AND parent_worker_id = ?2
                  ORDER BY created_at_unix_ms DESC, command_id",
            )?;
            let ids = statement
                .query_map(params![project_id, parent_worker_id], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let summaries = ids
                .iter()
                .map(|id| {
                    select_by_command(connection, id)?
                        .ok_or(ProjectStoreError::SummaryWorkerNotFound)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SummaryWorkers { summaries })
        })
        .await
}

pub(super) async fn get(
    store: &SqliteProjectStore,
    project_id: &str,
    parent_worker_id: &str,
    assignment_id: &str,
) -> Result<SummaryWorker, ProjectStoreError> {
    let project_id = project_id.trim().to_owned();
    let parent_worker_id = parent_worker_id.trim().to_owned();
    let assignment_id = assignment_id.trim().to_owned();
    store
        .run(move |connection| {
            let command_id = connection
                .query_row(
                    "SELECT command_id FROM summary_worker_commands
                      WHERE project_id = ?1 AND parent_worker_id = ?2
                        AND result_assignment_id = ?3",
                    params![project_id, parent_worker_id, assignment_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or(ProjectStoreError::SummaryWorkerNotFound)?;
            select_by_command(connection, &command_id)?
                .ok_or(ProjectStoreError::SummaryWorkerNotFound)
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn start_handoff(
    store: &SqliteProjectStore,
    project_id: &str,
    parent_worker_id: &str,
    assignment_id: &str,
    command: ReceiveSummaryWorker,
    pane_instance_id: Option<String>,
) -> Result<PreparedSummaryWorkerHandoff, ProjectStoreError> {
    let project_id = project_id.trim().to_owned();
    let parent_worker_id = parent_worker_id.trim().to_owned();
    let assignment_id = assignment_id.trim().to_owned();
    let command = command.normalize()?;
    let pane_instance_id = pane_instance_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let summary = select_by_assignment(&transaction, &assignment_id)?
                .ok_or(ProjectStoreError::SummaryWorkerNotFound)?;
            if summary.project_id != project_id || summary.parent_worker_id != parent_worker_id {
                return Err(ProjectStoreError::SummaryWorkerNotFound);
            }
            let project = select_project(&transaction, &project_id)?;
            if project.orchestrator.id != parent_worker_id {
                return Err(ProjectStoreError::SummaryParentNotCurrent);
            }
            if project.orchestrator.version != command.expected_parent_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: project.orchestrator.version,
                });
            }
            let artifact = ready_artifact(&summary)?;
            let existing_handoff = transaction.query_row(
                "SELECT handoff_command_id, cleanup_run_id
                   FROM summary_worker_commands WHERE command_id = ?1",
                [&summary.command_id],
                |row| row.get::<_, Option<String>>(0),
            )?;
            if let Some(existing_handoff) = existing_handoff {
                if existing_handoff != command.command_id {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(PreparedSummaryWorkerHandoff {
                    summary,
                    artifact_id: artifact.id,
                    replayed: true,
                });
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            let assignment = summary
                .assignment
                .as_ref()
                .ok_or(ProjectStoreError::SummaryWorkerHandoffNotReady)?;
            let receipt = assignment
                .completion_receipt
                .as_ref()
                .ok_or(ProjectStoreError::SummaryWorkerHandoffNotReady)?;
            let runtime = assignment
                .worker
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let owned = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM workers
                     WHERE id = ?1 AND ownership_kind = 'system_ephemeral'
                       AND parent_worker_id = ?2 AND ended_at_unix_ms IS NULL
                 )",
                params![assignment.worker.id, parent_worker_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !owned {
                return Err(ProjectStoreError::SummaryWorkerHandoffNotReady);
            }
            let policy_version = transaction.query_row(
                "SELECT version FROM worker_cleanup_policy WHERE singleton_id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            let now = unix_time_ms()?;
            let run_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO worker_cleanup_runs (
                    id, command_id, trigger, preview, status, advisor_state,
                    requested_by, policy_version, grace_period_ms, batch_size,
                    advisor_profile_id, cancellation_requested, created_at_unix_ms,
                    updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, 'manual', 0, 'pending', 'not_requested',
                    ?3, ?4, 0, 1, NULL, 0, ?5, ?5
                 )",
                params![
                    run_id,
                    command.command_id,
                    command.actor,
                    policy_version,
                    to_i64(now)?
                ],
            )?;
            transaction.execute(
                "INSERT INTO worker_cleanup_run_items (
                    run_id, worker_id, project_id, assignment_id,
                    completion_receipt_id, expected_worker_version,
                    expected_runtime_version, adapter, runtime_session,
                    runtime_workspace_id, terminal_id, tab_id, pane_id,
                    provider_session_json, status, reason, attempts,
                    next_attempt_at_unix_ms, is_cleanup_advisor,
                    updated_at_unix_ms, pane_instance_id
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, 'pending', 'summary_handoff_complete', 0, ?15, 0, ?15, ?16
                 )",
                params![
                    run_id,
                    assignment.worker.id,
                    project_id,
                    assignment.id,
                    receipt.id,
                    to_i64(assignment.worker.version)?,
                    to_i64(runtime.version)?,
                    runtime.adapter,
                    runtime.session,
                    runtime.workspace_id,
                    runtime.terminal_id,
                    runtime.tab_id,
                    runtime.pane_id,
                    runtime
                        .provider_session
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    to_i64(now)?,
                    pane_instance_id,
                ],
            )?;
            transaction.execute(
                "UPDATE summary_worker_commands
                    SET handoff_command_id = ?1, cleanup_run_id = ?2,
                        updated_at_unix_ms = ?3
                  WHERE command_id = ?4 AND handoff_command_id IS NULL",
                params![command.command_id, run_id, to_i64(now)?, summary.command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "summary_worker",
                &summary.command_id,
                3,
                "summary_worker_handoff_committed",
                &command.actor,
                now,
            )?;
            let summary = select_by_command(&transaction, &summary.command_id)?
                .ok_or(ProjectStoreError::SummaryWorkerNotFound)?;
            transaction.commit()?;
            Ok(PreparedSummaryWorkerHandoff {
                summary,
                artifact_id: artifact.id,
                replayed: false,
            })
        })
        .await
}

fn select_by_assignment(
    connection: &Connection,
    assignment_id: &str,
) -> Result<Option<SummaryWorker>, ProjectStoreError> {
    let command_id = connection
        .query_row(
            "SELECT command_id FROM summary_worker_commands
              WHERE result_assignment_id = ?1",
            [assignment_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    command_id
        .map(|command_id| select_by_command(connection, &command_id))
        .transpose()
        .map(Option::flatten)
}

#[allow(clippy::too_many_lines)]
fn select_by_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<SummaryWorker>, ProjectStoreError> {
    let stored = connection
        .query_row(
            "SELECT command_id, project_id, parent_worker_id, profile_id,
                    profile_version, expected_artifact_id, objective,
                    captured_workspace_id, status,
                    result_assignment_id, cleanup_run_id, error_message,
                    created_at_unix_ms, updated_at_unix_ms
               FROM summary_worker_commands WHERE command_id = ?1",
            [command_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            },
        )
        .optional()?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let mut assignment = stored
        .9
        .as_deref()
        .map(|assignment_id| {
            select_assignment_record(connection, assignment_id)?
                .map(|record| record.assignment)
                .ok_or(ProjectStoreError::AssignmentNotFound)
        })
        .transpose()?;
    if let Some(assignment) = &mut assignment {
        assignment.completion_receipt = select_completion_receipt(connection, &assignment.id)?;
    }
    let artifact = assignment
        .as_ref()
        .and_then(|assignment| assignment.completion_receipt.as_ref())
        .and_then(|receipt| {
            if receipt.unresolved_blockers.is_empty() && receipt.artifacts.len() == 1 {
                receipt.artifacts.first().cloned()
            } else {
                None
            }
        })
        .filter(|artifact| {
            artifact.id == stored.5
                && valid_artifact(artifact, assignment.as_ref().expect("assignment"))
        });
    let cleanup = stored
        .10
        .as_deref()
        .map(|run_id| {
            connection
                .query_row(
                    "SELECT status, reason FROM worker_cleanup_run_items
                      WHERE run_id = ?1",
                    [run_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
        })
        .transpose()?
        .flatten();
    let invalid_completion = assignment.as_ref().is_some_and(|assignment| {
        assignment.lifecycle == AssignmentLifecycle::Completed && artifact.is_none()
    });
    let failed_assignment = assignment
        .as_ref()
        .is_some_and(|assignment| assignment.lifecycle == AssignmentLifecycle::Failed);
    let state = if stored.8 == "failed" || invalid_completion || failed_assignment {
        SummaryWorkerState::Failed
    } else if let Some((status, _)) = &cleanup {
        match status.as_str() {
            "pending" => SummaryWorkerState::RetirementPending,
            "retired" => SummaryWorkerState::Retired,
            _ => SummaryWorkerState::RetirementDeferred,
        }
    } else if artifact.is_some()
        && assignment
            .as_ref()
            .is_some_and(|assignment| assignment.lifecycle == AssignmentLifecycle::Completed)
    {
        SummaryWorkerState::Ready
    } else if assignment.is_some() {
        SummaryWorkerState::Active
    } else {
        SummaryWorkerState::Allocating
    };
    let error = stored
        .11
        .or_else(|| {
            invalid_completion.then(|| {
                "completion receipt did not reference the exact Markdown summary artifact"
                    .to_owned()
            })
        })
        .or_else(|| {
            failed_assignment.then(|| {
                assignment
                    .as_ref()
                    .and_then(|assignment| assignment.attempt.error.clone())
                    .unwrap_or_else(|| "summary assignment failed".to_owned())
            })
        });
    Ok(Some(SummaryWorker {
        command_id: stored.0,
        project_id: stored.1,
        parent_worker_id: stored.2,
        profile_id: stored.3,
        profile_version: stored.4.try_into()?,
        expected_artifact_id: stored.5,
        objective: stored.6,
        captured_workspace_id: stored.7,
        state,
        assignment,
        artifact,
        cleanup_run_id: stored.10,
        retirement_reason: cleanup.map(|(_, reason)| reason),
        error,
        created_at_unix_ms: stored.12.try_into()?,
        updated_at_unix_ms: stored.13.try_into()?,
    }))
}

fn ready_artifact(summary: &SummaryWorker) -> Result<Artifact, ProjectStoreError> {
    let assignment = summary
        .assignment
        .as_ref()
        .ok_or(ProjectStoreError::SummaryWorkerHandoffNotReady)?;
    if assignment.lifecycle != AssignmentLifecycle::Completed {
        return Err(ProjectStoreError::SummaryWorkerHandoffNotReady);
    }
    summary
        .artifact
        .clone()
        .ok_or(ProjectStoreError::SummaryWorkerHandoffNotReady)
}

fn valid_artifact(artifact: &Artifact, assignment: &yard_domain::Assignment) -> bool {
    artifact.kind == ArtifactKind::Markdown
        && artifact.project_id == assignment.project_id
        && artifact.assignment_id == assignment.id
        && artifact.attempt_id == assignment.attempt.id
        && artifact.worker_id == assignment.worker.id
}

fn validate_capture(capture: &SummaryParentRuntimeCapture) -> Result<(), ProjectStoreError> {
    if capture.adapter.trim().is_empty()
        || capture.session.trim().is_empty()
        || capture.workspace_id.trim().is_empty()
        || capture.terminal_id.trim().is_empty()
        || capture.tab_id.trim().is_empty()
        || capture.pane_id.trim().is_empty()
        || capture.observed_at_unix_ms == 0
    {
        return Err(ProjectStoreError::SummaryParentRuntimeUnverified);
    }
    Ok(())
}

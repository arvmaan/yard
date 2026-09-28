//! One-request assignment disposition: a minimal completion receipt or a
//! cancellation, optionally ending the worker's session in the same
//! IMMEDIATE transaction.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;
use yard_domain::{
    AssignmentLifecycle, AttemptLifecycle, DisposeAssignment, DisposedAssignment,
    DispositionOutcome, MINIMAL_RECEIPT_SUMMARY, RequestOrigin, WorkerAvailability,
    WorkerOwnershipKind,
};

use super::{
    ProjectStoreError, RuntimeCleanupExpectation, SqliteProjectStore, attach_assignment_outcome,
    command_id_exists, insert_lifecycle_event, insert_retired_runtime_binding,
    insert_runtime_cleanup_job, request_origin_value, required_assignment_id, required_id,
    row_optional_u64, row_u64, runtime_cleanup_pending, select_assignment_record,
    select_worker_candidate, to_i64, transcript_capture_store, unix_time_ms,
};

#[derive(Debug)]
struct StoredDispositionCommand {
    project_id: String,
    assignment_id: String,
    attempt_id: String,
    expected_assignment_version: u64,
    expected_attempt_version: u64,
    outcome: String,
    end_session: bool,
    expected_worker_version: Option<u64>,
    expected_runtime_version: Option<u64>,
    actor: String,
    status: String,
    error_message: Option<String>,
}

impl StoredDispositionCommand {
    fn matches(&self, project_id: &str, assignment_id: &str, command: &DisposeAssignment) -> bool {
        self.project_id == project_id
            && self.assignment_id == assignment_id
            && self.attempt_id == command.attempt_id
            && self.expected_assignment_version == command.expected_assignment_version
            && self.expected_attempt_version == command.expected_attempt_version
            && self.outcome == outcome_value(command.outcome)
            && self.end_session == command.end_session
            && self.expected_worker_version == command.expected_worker_version
            && self.expected_runtime_version == command.expected_runtime_version
            && self.actor == command.actor
    }
}

/// The unfinished handoff whose source is the disposed assignment.
pub(super) struct SourceHandoff {
    pub(super) command_id: String,
    pub(super) status: String,
}

const fn outcome_value(outcome: DispositionOutcome) -> &'static str {
    match outcome {
        DispositionOutcome::Completed => "completed",
        DispositionOutcome::Cancelled => "cancelled",
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn dispose_assignment(
    store: &SqliteProjectStore,
    project_id: &str,
    assignment_id: &str,
    command: DisposeAssignment,
    request_origin: RequestOrigin,
) -> Result<DisposedAssignment, ProjectStoreError> {
    let project_id = required_id(project_id)?;
    let assignment_id = required_assignment_id(assignment_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_disposition_command(&transaction, &command.command_id)? {
                if !existing.matches(&project_id, &assignment_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => {
                        let result = disposed_result(
                            &transaction,
                            &command.command_id,
                            &assignment_id,
                            command.outcome,
                            command.end_session,
                            true,
                        )?;
                        transaction.commit()?;
                        Ok(result)
                    }
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing
                            .error_message
                            .unwrap_or_else(|| "assignment disposition failed".to_owned()),
                    )),
                    _ => Err(ProjectStoreError::CommandInProgress),
                };
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }

            let record = select_assignment_record(&transaction, &assignment_id)?
                .ok_or(ProjectStoreError::AssignmentNotFound)?;
            let assignment = &record.assignment;
            if assignment.project_id != project_id {
                return Err(ProjectStoreError::AssignmentNotFound);
            }
            let (from_lifecycle, from_attempt_lifecycle, handoff) = match assignment.lifecycle {
                AssignmentLifecycle::Active => ("active", AttemptLifecycle::Active, None),
                AssignmentLifecycle::HandingOff => {
                    let handoff = select_unfinished_source_handoff(&transaction, &assignment_id)?;
                    match handoff {
                        Some(handoff) if handoff.status == "pending" => {
                            return Err(ProjectStoreError::CommandInProgress);
                        }
                        Some(handoff) if handoff.status == "ambiguous" => {
                            if command.outcome != DispositionOutcome::Cancelled {
                                return Err(ProjectStoreError::AssignmentHandoffUnresolved);
                            }
                            ("handing_off", AttemptLifecycle::HandingOff, Some(handoff))
                        }
                        _ => return Err(ProjectStoreError::AssignmentNotActive),
                    }
                }
                _ => return Err(ProjectStoreError::AssignmentNotActive),
            };
            if assignment.version != command.expected_assignment_version {
                return Err(ProjectStoreError::AssignmentVersionConflict {
                    current_version: assignment.version,
                });
            }
            if assignment.attempt.id != command.attempt_id {
                return Err(ProjectStoreError::AttemptNotCurrent {
                    current_attempt_id: assignment.attempt.id.clone(),
                });
            }
            if assignment.attempt.lifecycle != from_attempt_lifecycle {
                return Err(ProjectStoreError::AttemptNotActive);
            }
            if assignment.attempt.version != command.expected_attempt_version {
                return Err(ProjectStoreError::AttemptVersionConflict {
                    current_version: assignment.attempt.version,
                });
            }
            let prompt_pending = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM assignment_prompt_commands apc
                      JOIN command_acknowledgements ca ON ca.id = apc.command_id
                     WHERE apc.assignment_id = ?1
                       AND ca.status = 'pending'
                 )",
                [&assignment_id],
                |row| row.get::<_, bool>(0),
            )?;
            if prompt_pending {
                return Err(ProjectStoreError::AssignmentInterventionInProgress);
            }

            let worker_id = assignment.worker.id.clone();
            let candidate = select_worker_candidate(&transaction, &worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            match candidate.availability {
                WorkerAvailability::YardOrchestrator => {
                    return Err(ProjectStoreError::YardOrchestratorSessionEndForbidden);
                }
                WorkerAvailability::CoordinationNode => {
                    return Err(ProjectStoreError::CoordinationNodeSessionEndForbidden);
                }
                WorkerAvailability::Orchestrator => {
                    return Err(ProjectStoreError::OrchestratorSessionEndForbidden);
                }
                _ => {}
            }
            // Mainline's ephemeral summary workers run work that only their
            // summary command may finish (it also settles the cleanup run).
            let summary_assignment = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM summary_worker_commands
                     WHERE result_assignment_id = ?1
                 )",
                [&assignment_id],
                |row| row.get::<_, bool>(0),
            )?;
            if candidate.worker.ownership_kind == WorkerOwnershipKind::SystemEphemeral
                || summary_assignment
            {
                return Err(ProjectStoreError::SystemEphemeralWorker);
            }
            let runtime = candidate.worker.runtime.clone();
            if command.end_session {
                if Some(candidate.worker.version) != command.expected_worker_version {
                    return Err(ProjectStoreError::WorkerVersionConflict {
                        current_version: candidate.worker.version,
                    });
                }
                let current_runtime_version = runtime.as_ref().map(|runtime| runtime.version);
                if current_runtime_version != command.expected_runtime_version {
                    return Err(ProjectStoreError::WorkerRuntimeVersionConflict {
                        current_version: current_runtime_version,
                    });
                }
            }

            let next_assignment_version = assignment
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let next_attempt_version = assignment
                .attempt
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            let origin = request_origin_value(request_origin);
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'assignment_disposition', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO assignment_disposition_commands (
                    command_id, project_id, assignment_id, attempt_id,
                    worker_id, expected_assignment_version,
                    expected_attempt_version, outcome, end_session,
                    expected_worker_version, expected_runtime_version,
                    request_origin, handoff_command_id, finished_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    command.command_id,
                    project_id,
                    assignment_id,
                    command.attempt_id,
                    worker_id,
                    to_i64(command.expected_assignment_version)?,
                    to_i64(command.expected_attempt_version)?,
                    outcome_value(command.outcome),
                    command.end_session,
                    command.expected_worker_version.map(to_i64).transpose()?,
                    command.expected_runtime_version.map(to_i64).transpose()?,
                    origin,
                    handoff.as_ref().map(|handoff| handoff.command_id.as_str()),
                    to_i64(now)?,
                ],
            )?;

            // The objective is copied from the assignment row inside this
            // transaction; it is never taken from the client.
            let objective = assignment.objective.clone();
            let (to_lifecycle, event_type) = match command.outcome {
                DispositionOutcome::Completed => {
                    transaction.execute(
                        "INSERT INTO completion_receipts (
                            id, command_id, assignment_id, attempt_id, outcome,
                            summary, actor, created_at_unix_ms, detail_level,
                            objective_snapshot, request_origin
                         ) VALUES (
                            ?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?7, 'minimal', ?8, ?9
                         )",
                        params![
                            Uuid::now_v7().to_string(),
                            command.command_id,
                            assignment_id,
                            command.attempt_id,
                            MINIMAL_RECEIPT_SUMMARY,
                            command.actor,
                            to_i64(now)?,
                            objective,
                            origin,
                        ],
                    )?;
                    ("completed", "completion_receipt_recorded")
                }
                DispositionOutcome::Cancelled => {
                    transaction.execute(
                        "INSERT INTO assignment_cancellations (
                            assignment_id, attempt_id, command_id, reason, actor,
                            request_origin, objective_snapshot, cancelled_at_unix_ms
                         ) VALUES (
                            ?1, ?2, ?3, 'ended_without_completion', ?4, ?5, ?6, ?7
                         )",
                        params![
                            assignment_id,
                            command.attempt_id,
                            command.command_id,
                            command.actor,
                            origin,
                            objective,
                            to_i64(now)?,
                        ],
                    )?;
                    ("cancelled", "assignment_cancelled")
                }
            };

            if let Some(handoff) = handoff.as_ref() {
                close_ambiguous_handoff(&transaction, &handoff.command_id, now)?;
            }

            let assignment_rows = transaction.execute(
                "UPDATE assignments
                    SET lifecycle = ?1, version = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4 AND lifecycle = ?5 AND version = ?6",
                params![
                    to_lifecycle,
                    to_i64(next_assignment_version)?,
                    to_i64(now)?,
                    assignment_id,
                    from_lifecycle,
                    to_i64(command.expected_assignment_version)?,
                ],
            )?;
            if assignment_rows != 1 {
                return Err(ProjectStoreError::AssignmentNotActive);
            }
            let attempt_rows = transaction.execute(
                "UPDATE assignment_attempts
                    SET lifecycle = ?1, version = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4 AND assignment_id = ?5
                    AND lifecycle = ?6 AND version = ?7",
                params![
                    to_lifecycle,
                    to_i64(next_attempt_version)?,
                    to_i64(now)?,
                    command.attempt_id,
                    assignment_id,
                    from_lifecycle,
                    to_i64(command.expected_attempt_version)?,
                ],
            )?;
            if attempt_rows != 1 {
                return Err(ProjectStoreError::AttemptNotActive);
            }
            transaction.execute(
                "UPDATE worker_allocations
                    SET ended_at_unix_ms = ?1
                  WHERE id = ?2 AND ended_at_unix_ms IS NULL",
                params![to_i64(now)?, record.allocation.id],
            )?;

            // Queue the transcript capture before any binding is removed, so
            // the captured identity is the one the assignment ran in.
            if let Some(runtime) = runtime.as_ref() {
                transcript_capture_store::enqueue_transcript_capture(
                    &transaction,
                    &command.command_id,
                    &worker_id,
                    Some(&assignment_id),
                    Some(&record.allocation.id),
                    runtime,
                    now,
                )?;
            }

            if command.end_session {
                end_worker_session_writes(
                    &transaction,
                    &SessionEnd {
                        command_id: &command.command_id,
                        worker_id: &worker_id,
                        worker_version: candidate.worker.version,
                        runtime: runtime.as_ref(),
                        reason: "worker_session_end",
                        event_type: "worker_session_ended",
                        source: "user",
                    },
                    now,
                )?;
            }

            transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = 'succeeded', updated_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending'",
                params![to_i64(now)?, command.command_id],
            )?;
            insert_lifecycle_event(
                &transaction,
                "assignment",
                &assignment_id,
                next_assignment_version,
                event_type,
                "user",
                now,
            )?;
            let result = disposed_result(
                &transaction,
                &command.command_id,
                &assignment_id,
                command.outcome,
                command.end_session,
                false,
            )?;
            transaction.commit()?;
            Ok(result)
        })
        .await
}

/// One worker session ended under another command's ID.
#[derive(Clone, Copy)]
pub(super) struct SessionEnd<'a> {
    pub(super) command_id: &'a str,
    pub(super) worker_id: &'a str,
    pub(super) worker_version: u64,
    pub(super) runtime: Option<&'a yard_domain::WorkerRuntimeBinding>,
    /// The retirement and cleanup reason (`worker_session_end` or
    /// `project_archive`).
    pub(super) reason: &'a str,
    pub(super) event_type: &'a str,
    pub(super) source: &'a str,
}

/// The `end_worker_session` writes, applied under the calling command's ID:
/// retire the binding, queue a detached cleanup job, drop the binding, and
/// mark the worker ended.
pub(super) fn end_worker_session_writes(
    transaction: &Transaction<'_>,
    end: &SessionEnd<'_>,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let SessionEnd {
        command_id,
        worker_id,
        worker_version,
        runtime,
        reason,
        event_type,
        source,
    } = *end;
    let next_worker_version = worker_version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    if let Some(runtime) = runtime {
        insert_retired_runtime_binding(transaction, command_id, worker_id, reason, runtime, now)?;
        insert_runtime_cleanup_job(
            transaction,
            command_id,
            worker_id,
            reason,
            runtime,
            RuntimeCleanupExpectation {
                worker_version: Some(next_worker_version),
                binding_state: "detached",
            },
            now,
        )?;
        let rows = transaction.execute(
            "DELETE FROM worker_runtime_bindings
              WHERE worker_id = ?1 AND version = ?2",
            params![worker_id, to_i64(runtime.version)?],
        )?;
        if rows != 1 {
            return Err(ProjectStoreError::WorkerRuntimeVersionConflict {
                current_version: None,
            });
        }
    }
    let rows = transaction.execute(
        "UPDATE workers
            SET ended_at_unix_ms = ?1, version = ?2, updated_at_unix_ms = ?1
          WHERE id = ?3 AND version = ?4 AND ended_at_unix_ms IS NULL",
        params![
            to_i64(now)?,
            to_i64(next_worker_version)?,
            worker_id,
            to_i64(worker_version)?,
        ],
    )?;
    if rows != 1 {
        return Err(ProjectStoreError::WorkerVersionConflict {
            current_version: worker_version,
        });
    }
    insert_lifecycle_event(
        transaction,
        "worker",
        worker_id,
        next_worker_version,
        event_type,
        source,
        now,
    )
}

/// Close an ambiguous handoff so it stops counting as unfinished anywhere.
/// A claimed target runtime is quarantined first (as a finalize failure
/// does), so its pane can never be adopted or reused silently. The ack stays
/// `ambiguous`.
pub(super) fn close_ambiguous_handoff(
    transaction: &Transaction<'_>,
    handoff_command_id: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "INSERT INTO quarantined_provisioning_runtime_bindings (
            command_id, adapter, runtime_session, runtime_workspace_id,
            terminal_id, tab_id, pane_id, provider_session_source,
            provider_session_provider, provider_session_kind,
            provider_session_value, owns_tab, observation_state,
            process_state, observed_status, state_change_sequence,
            runtime_revision, runtime_version, last_observed_at_unix_ms,
            captured_at_unix_ms
         )
         SELECT command_id, target_runtime_adapter, target_runtime_session,
                target_runtime_workspace_id, target_terminal_id, target_tab_id,
                target_pane_id, target_provider_session_source,
                target_provider_session_provider, target_provider_session_kind,
                target_provider_session_value, 0, 'ambiguous', 'unknown',
                'unknown', 0, 0, 1, target_runtime_claimed_at_unix_ms, ?2
           FROM worker_handoff_commands
          WHERE command_id = ?1 AND target_terminal_id IS NOT NULL
         ON CONFLICT(command_id) DO NOTHING",
        params![handoff_command_id, to_i64(now)?],
    )?;
    let rows = transaction.execute(
        "UPDATE worker_handoff_commands
            SET finished_at_unix_ms = ?1
          WHERE command_id = ?2 AND finished_at_unix_ms IS NULL",
        params![to_i64(now)?, handoff_command_id],
    )?;
    if rows != 1 {
        return Err(ProjectStoreError::HandoffSourceChanged);
    }
    Ok(())
}

pub(super) fn select_unfinished_source_handoff(
    connection: &Connection,
    assignment_id: &str,
) -> Result<Option<SourceHandoff>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT handoff.command_id, ack.status
               FROM worker_handoff_commands handoff
               JOIN command_acknowledgements ack ON ack.id = handoff.command_id
              WHERE handoff.source_assignment_id = ?1
                AND handoff.finished_at_unix_ms IS NULL
              ORDER BY ack.created_at_unix_ms DESC, handoff.command_id DESC
              LIMIT 1",
            [assignment_id],
            |row| {
                Ok(SourceHandoff {
                    command_id: row.get(0)?,
                    status: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_disposition_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredDispositionCommand>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT command.project_id, command.assignment_id,
                    command.attempt_id, command.expected_assignment_version,
                    command.expected_attempt_version, command.outcome,
                    command.end_session, command.expected_worker_version,
                    command.expected_runtime_version, ack.actor, ack.status,
                    ack.error_message
               FROM assignment_disposition_commands command
               JOIN command_acknowledgements ack ON ack.id = command.command_id
              WHERE command.command_id = ?1",
            [command_id],
            |row| {
                Ok(StoredDispositionCommand {
                    project_id: row.get(0)?,
                    assignment_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    expected_assignment_version: row_u64(row, 3)?,
                    expected_attempt_version: row_u64(row, 4)?,
                    outcome: row.get(5)?,
                    end_session: row.get(6)?,
                    expected_worker_version: row_optional_u64(row, 7)?,
                    expected_runtime_version: row_optional_u64(row, 8)?,
                    actor: row.get(9)?,
                    status: row.get(10)?,
                    error_message: row.get(11)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

/// Build the response from current durable state, so a replay reports live
/// cleanup and transcript status.
fn disposed_result(
    connection: &Connection,
    command_id: &str,
    assignment_id: &str,
    outcome: DispositionOutcome,
    end_session: bool,
    replayed: bool,
) -> Result<DisposedAssignment, ProjectStoreError> {
    let mut record = select_assignment_record(connection, assignment_id)?
        .ok_or(ProjectStoreError::AssignmentNotFound)?;
    attach_assignment_outcome(connection, &mut record.assignment)?;
    let assignment = record.assignment;
    let (receipt, cancellation) = match outcome {
        DispositionOutcome::Completed => (assignment.completion_receipt.clone(), None),
        DispositionOutcome::Cancelled => (None, assignment.cancellation.clone()),
    };
    Ok(DisposedAssignment {
        command_id: command_id.to_owned(),
        worker: end_session.then(|| assignment.worker.clone()),
        receipt,
        cancellation,
        cleanup_pending: runtime_cleanup_pending(connection, command_id)?,
        transcript_pending: transcript_capture_store::transcript_capture_pending(
            connection, command_id,
        )?,
        assignment,
        replayed,
    })
}

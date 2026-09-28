//! Archive a project that still has active workers.
//!
//! The disposition preview lists every assignment that is still allocating,
//! active, or handing off. An archive (or delete from active) with
//! `active_work = cancel` must echo exactly that set back; inside the archive
//! transaction each listed assignment is then recorded as cancelled with
//! reason `project_archived` (never completed), its allocation ends, its
//! worker session ends, and a transcript capture is queued. An allocating
//! assignment is listed but never cancelled: its unresolved allocation
//! refuses the archive, and the web dialog waits for it instead of offering
//! the confirmation.
//!
//! Assignments of Yard's ephemeral summary workers are listed separately in
//! the preview but are part of the same expected set. Cancelling one also
//! moves its `summary_worker_commands` row to `failed` with
//! `error_message = 'project_archived'`. A summary command that is still
//! `allocating` has no assignment to cancel, so it blocks the archive (409
//! `project_summary_worker_allocating`) until it is active or has failed.
//!
//! Pane-management leases (mainline 0030) held for a cancelled worker are
//! left alone here: releasing one needs a Herdr RPC with the lease token, and
//! Yard has no durable release path. Once the runtime cleanup closes the
//! pane, the renewal loop marks the lease `recovery_required`.

use rusqlite::{Connection, Transaction, params};
use yard_domain::{
    AssignmentLifecycle, AttemptLifecycle, ExpectedActiveAssignment, ProjectArchiveActiveWork,
    ProjectArchivePreconditions, ProjectDispositionAssignment, ProjectDispositionPreview,
    RequestOrigin, WorkerAvailability, WorkerDesiredState,
};

use super::assignment_disposition_store::{
    SessionEnd, close_ambiguous_handoff, end_worker_session_writes,
    select_unfinished_source_handoff,
};
use super::{
    ProjectStoreError, insert_lifecycle_event, request_origin_value, row_u64,
    select_assignment_record, select_project, select_worker_candidate, to_i64,
    transcript_capture_store,
};

/// The lifecycles an archive either refuses or cancels.
const ACTIVE_LIFECYCLES: &str = "('allocating', 'active', 'handing_off')";

/// Everything a project archive or delete would end, from current state.
pub(super) fn disposition_preview(
    connection: &Connection,
    project_id: &str,
) -> Result<ProjectDispositionPreview, ProjectStoreError> {
    let project = select_project(connection, project_id)?;
    let mut statement = connection.prepare(&format!(
        "SELECT assignment.id, EXISTS (
                SELECT 1 FROM summary_worker_commands summary
                 WHERE summary.result_assignment_id = assignment.id
            )
           FROM assignments assignment
          WHERE assignment.project_id = ?1
            AND assignment.lifecycle IN {ACTIVE_LIFECYCLES}
          ORDER BY assignment.created_at_unix_ms, assignment.id"
    ))?;
    let assignment_ids = statement
        .query_map([project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut active_assignments = Vec::new();
    let mut summary_worker_assignments = Vec::new();
    for (assignment_id, summary_worker) in &assignment_ids {
        let listed = disposition_assignment(connection, assignment_id)?;
        if *summary_worker {
            summary_worker_assignments.push(listed);
        } else {
            active_assignments.push(listed);
        }
    }
    Ok(ProjectDispositionPreview {
        project_id: project.id,
        name: project.name,
        project_version: project.version,
        orchestrator_worker_id: project.orchestrator.id,
        orchestrator_worker_version: project.orchestrator.version,
        orchestrator_runtime_version: project
            .orchestrator
            .runtime
            .as_ref()
            .map(|runtime| runtime.version),
        active_assignments,
        summary_worker_assignments,
    })
}

fn disposition_assignment(
    connection: &Connection,
    assignment_id: &str,
) -> Result<ProjectDispositionAssignment, ProjectStoreError> {
    let record = select_assignment_record(connection, assignment_id)?
        .ok_or(ProjectStoreError::AssignmentNotFound)?;
    let assignment = record.assignment;
    Ok(ProjectDispositionAssignment {
        assignment_id: assignment.id,
        assignment_version: assignment.version,
        lifecycle: assignment.lifecycle,
        objective: assignment.objective,
        role: assignment.role,
        profile_name: assignment.profile_name,
        worker_id: assignment.worker.id,
        runtime_present: assignment.worker.runtime.is_some(),
    })
}

/// Gate 2 of the archive: compare the live active assignments with the
/// caller's choice. Returns the (sorted) set the archive must cancel.
///
/// With `reject`, any active assignment refuses the archive. With `cancel`,
/// the live set must equal `expected_active_assignments` in every ID and
/// version, so an archive never cancels work the user did not see. Both
/// refusals carry a fresh preview.
pub(super) fn gate_active_work(
    connection: &Connection,
    project_id: &str,
    preconditions: &ProjectArchivePreconditions,
) -> Result<Vec<ExpectedActiveAssignment>, ProjectStoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT id, version FROM assignments
          WHERE project_id = ?1 AND lifecycle IN {ACTIVE_LIFECYCLES}
          ORDER BY id"
    ))?;
    let live = statement
        .query_map([project_id], |row| {
            Ok(ExpectedActiveAssignment {
                assignment_id: row.get(0)?,
                expected_assignment_version: row_u64(row, 1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    match preconditions.active_work {
        ProjectArchiveActiveWork::Reject if live.is_empty() => Ok(live),
        ProjectArchiveActiveWork::Reject => Err(ProjectStoreError::ProjectHasActiveWork(Box::new(
            disposition_preview(connection, project_id)?,
        ))),
        ProjectArchiveActiveWork::Cancel => {
            let expected = preconditions
                .expected_active_assignments
                .as_deref()
                .unwrap_or_default();
            if live == expected {
                Ok(live)
            } else {
                Err(ProjectStoreError::ProjectArchivePreviewStale(Box::new(
                    disposition_preview(connection, project_id)?,
                )))
            }
        }
    }
}

/// Record each listed assignment as cancelled by the archive and end its
/// worker, all under the archive's command ID. The caller has already run
/// [`gate_active_work`] in the same transaction.
#[allow(clippy::too_many_lines)]
pub(super) fn cancel_active_assignments(
    transaction: &Transaction<'_>,
    command_id: &str,
    actor: &str,
    request_origin: RequestOrigin,
    listed: &[ExpectedActiveAssignment],
    now: u64,
) -> Result<(), ProjectStoreError> {
    let origin = request_origin_value(request_origin);
    for expected in listed {
        let record = select_assignment_record(transaction, &expected.assignment_id)?
            .ok_or(ProjectStoreError::AssignmentNotFound)?;
        let assignment = &record.assignment;
        if assignment.version != expected.expected_assignment_version {
            return Err(ProjectStoreError::AssignmentVersionConflict {
                current_version: assignment.version,
            });
        }
        let (from_lifecycle, from_attempt_lifecycle, handoff) = match assignment.lifecycle {
            AssignmentLifecycle::Active => ("active", AttemptLifecycle::Active, None),
            AssignmentLifecycle::HandingOff => {
                // A pending handoff out of this project already refused the
                // archive as in flight; only an ambiguous one is left here.
                match select_unfinished_source_handoff(transaction, &assignment.id)? {
                    Some(handoff) if handoff.status == "ambiguous" => {
                        ("handing_off", AttemptLifecycle::HandingOff, Some(handoff))
                    }
                    Some(_) => return Err(ProjectStoreError::ProjectArchiveHandoffInProgress),
                    None => return Err(ProjectStoreError::AssignmentNotActive),
                }
            }
            // An allocating assignment belongs to an unresolved allocation,
            // which the dependency gate refuses before any write.
            _ => return Err(ProjectStoreError::ProjectHasArchiveDependencies),
        };
        if assignment.attempt.lifecycle != from_attempt_lifecycle {
            return Err(ProjectStoreError::AttemptNotActive);
        }
        let prompt_pending = transaction.query_row(
            "SELECT EXISTS (
                SELECT 1
                  FROM assignment_prompt_commands apc
                  JOIN command_acknowledgements ca ON ca.id = apc.command_id
                 WHERE apc.assignment_id = ?1
                   AND ca.status = 'pending'
             )",
            [&assignment.id],
            |row| row.get::<_, bool>(0),
        )?;
        if prompt_pending {
            return Err(ProjectStoreError::AssignmentInterventionInProgress);
        }
        let candidate = select_worker_candidate(transaction, &assignment.worker.id)?
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
        let next_assignment_version = assignment
            .version
            .checked_add(1)
            .ok_or(ProjectStoreError::VersionOverflow)?;
        let next_attempt_version = assignment
            .attempt
            .version
            .checked_add(1)
            .ok_or(ProjectStoreError::VersionOverflow)?;

        // The objective is copied from the assignment row inside this
        // transaction. No receipt is ever written: archive is not completion.
        transaction.execute(
            "INSERT INTO assignment_cancellations (
                assignment_id, attempt_id, command_id, reason, actor,
                request_origin, objective_snapshot, cancelled_at_unix_ms
             ) VALUES (?1, ?2, ?3, 'project_archived', ?4, ?5, ?6, ?7)",
            params![
                assignment.id,
                assignment.attempt.id,
                command_id,
                actor,
                origin,
                assignment.objective,
                to_i64(now)?,
            ],
        )?;
        if let Some(handoff) = handoff.as_ref() {
            close_ambiguous_handoff(transaction, &handoff.command_id, now)?;
        }
        let assignment_rows = transaction.execute(
            "UPDATE assignments
                SET lifecycle = 'cancelled', version = ?1, updated_at_unix_ms = ?2
              WHERE id = ?3 AND lifecycle = ?4 AND version = ?5",
            params![
                to_i64(next_assignment_version)?,
                to_i64(now)?,
                assignment.id,
                from_lifecycle,
                to_i64(assignment.version)?,
            ],
        )?;
        if assignment_rows != 1 {
            return Err(ProjectStoreError::AssignmentNotActive);
        }
        let attempt_rows = transaction.execute(
            "UPDATE assignment_attempts
                SET lifecycle = 'cancelled', version = ?1, updated_at_unix_ms = ?2
              WHERE id = ?3 AND assignment_id = ?4
                AND lifecycle = ?5 AND version = ?6",
            params![
                to_i64(next_attempt_version)?,
                to_i64(now)?,
                assignment.attempt.id,
                assignment.id,
                from_lifecycle,
                to_i64(assignment.attempt.version)?,
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

        // Queue the transcript capture before the binding is removed, so the
        // captured identity is the one the assignment ran in.
        let runtime = candidate.worker.runtime.as_ref();
        if let Some(runtime) = runtime {
            transcript_capture_store::enqueue_transcript_capture(
                transaction,
                command_id,
                &candidate.worker.id,
                Some(&assignment.id),
                Some(&record.allocation.id),
                runtime,
                now,
            )?;
        }
        if candidate.worker.desired_state == WorkerDesiredState::Running {
            end_worker_session_writes(
                transaction,
                &SessionEnd {
                    command_id,
                    worker_id: &candidate.worker.id,
                    worker_version: candidate.worker.version,
                    runtime,
                    reason: "project_archive",
                    event_type: "project_archive_worker_ended",
                    source: actor,
                },
                now,
            )?;
        }
        // A summary worker ended with the archive leaves its summary command
        // in the terminal `failed` state, so nothing waits for its handoff.
        transaction.execute(
            "UPDATE summary_worker_commands
                SET status = 'failed', error_message = 'project_archived',
                    updated_at_unix_ms = ?1
              WHERE result_assignment_id = ?2 AND status = 'active'",
            params![to_i64(now)?, assignment.id],
        )?;
        insert_lifecycle_event(
            transaction,
            "assignment",
            &assignment.id,
            next_assignment_version,
            "assignment_cancelled",
            actor,
            now,
        )?;
    }
    Ok(())
}

/// The assignments one archive (or delete from active) recorded as
/// cancelled, for results and replays.
pub(super) fn cancelled_assignment_ids(
    connection: &Connection,
    command_id: &str,
) -> Result<Vec<String>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT assignment_id FROM assignment_cancellations
          WHERE command_id = ?1 AND reason = 'project_archived'
          ORDER BY assignment_id",
    )?;
    statement
        .query_map([command_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

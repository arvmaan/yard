//! Durable transcript capture jobs and the read-only transcripts they store.
//!
//! A job captures one runtime identity (`worker` + adapter, session,
//! terminal, pane) when an assignment ends. The server reads the pane, then
//! stores the bounded text here. Jobs follow the claim-token pattern of
//! `runtime_cleanup_jobs`; they never expire while Herdr is unreachable.

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use uuid::Uuid;
use yard_domain::{
    ProviderSessionRef, TranscriptStatus, TranscriptUnavailableReason, WorkerRuntimeBinding,
    WorkerTranscript, bound_transcript_text,
};

use super::{
    ProjectStoreError, SqliteProjectStore, provider_session_from_columns, required_assignment_id,
    required_command_id, required_id, row_optional_u64, row_u64, to_i64, unix_time_ms,
};

/// One claimed transcript capture job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTranscriptCapture {
    pub id: String,
    pub command_id: String,
    pub worker_id: String,
    pub assignment_id: Option<String>,
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub provider_session: Option<ProviderSessionRef>,
    pub attempts: u32,
    pub claim_token: String,
}

/// Durable facts that decide whether a capture may still read its pane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscriptCaptureGuard {
    /// The worker is still bound to exactly the captured runtime identity.
    pub bound_to_worker: bool,
    /// The worker has taken new work since the capture was queued.
    pub worker_reallocated: bool,
    /// Another Yard worker is bound to the captured identity.
    pub bound_elsewhere: bool,
}

/// Terminal reasons a capture job ends without text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptCaptureExpiry {
    RuntimeClosed,
    RuntimeReused,
    WorkerReallocated,
}

impl TranscriptCaptureExpiry {
    const fn value(self) -> &'static str {
        match self {
            Self::RuntimeClosed => "runtime_closed",
            Self::RuntimeReused => "runtime_reused",
            Self::WorkerReallocated => "worker_reallocated",
        }
    }
}

/// Text read from the captured pane, before bounding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedTranscriptText {
    pub source: String,
    pub format: String,
    pub text: String,
    pub truncated: bool,
}

/// What a successful read turned into once the durable guard was re-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptCaptureOutcome {
    Stored,
    Expired(TranscriptCaptureExpiry),
}

/// Queue a capture of `runtime` for `worker_id`. Called inside the
/// transaction that ends the assignment, before any binding is removed.
pub(super) fn enqueue_transcript_capture(
    transaction: &Transaction<'_>,
    command_id: &str,
    worker_id: &str,
    assignment_id: Option<&str>,
    allocation_id: Option<&str>,
    runtime: &WorkerRuntimeBinding,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let provider = runtime.provider_session.as_ref();
    transaction.execute(
        "INSERT INTO transcript_capture_jobs (
            id, command_id, worker_id, assignment_id, allocation_id,
            adapter, runtime_session, runtime_workspace_id, terminal_id,
            tab_id, pane_id, provider_session_source,
            provider_session_provider, provider_session_kind,
            provider_session_value, status, expired_reason, attempts,
            last_error, next_attempt_at_unix_ms, claim_token,
            claim_expires_at_unix_ms, created_at_unix_ms,
            updated_at_unix_ms, completed_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, 'pending', NULL, 0, NULL, ?16, NULL, NULL, ?16, ?16, NULL
         )",
        params![
            Uuid::now_v7().to_string(),
            command_id,
            worker_id,
            assignment_id,
            allocation_id,
            runtime.adapter,
            runtime.session,
            runtime.workspace_id,
            runtime.terminal_id,
            runtime.tab_id,
            runtime.pane_id,
            provider.map(|session| session.source.as_str()),
            provider.map(|session| session.provider.as_str()),
            provider.map(|session| session.kind.as_str()),
            provider.map(|session| session.value.as_str()),
            to_i64(now)?,
        ],
    )?;
    Ok(())
}

/// Whether a command still has a queued or retrying capture.
pub(super) fn transcript_capture_pending(
    connection: &Connection,
    command_id: &str,
) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM transcript_capture_jobs
                 WHERE command_id = ?1 AND status = 'pending'
             )",
            [command_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub(super) async fn claim_pending(
    store: &SqliteProjectStore,
    command_id: Option<&str>,
    limit: usize,
    claim_ttl_ms: u64,
) -> Result<Vec<PendingTranscriptCapture>, ProjectStoreError> {
    let command_id = command_id.map(required_command_id).transpose()?;
    let limit = i64::try_from(limit.clamp(1, 100))?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now = unix_time_ms()?;
            let expires_at = now.saturating_add(claim_ttl_ms.max(1));
            let job_ids = {
                let mut statement = transaction.prepare(
                    "SELECT id
                       FROM transcript_capture_jobs
                      WHERE status = 'pending'
                        AND (?1 IS NULL OR command_id = ?1)
                        AND next_attempt_at_unix_ms <= ?2
                        AND (
                            claim_token IS NULL
                            OR claim_expires_at_unix_ms <= ?2
                        )
                      ORDER BY created_at_unix_ms, id
                      LIMIT ?3",
                )?;
                statement
                    .query_map(params![command_id, to_i64(now)?, limit], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut jobs = Vec::with_capacity(job_ids.len());
            for job_id in job_ids {
                let claim_token = Uuid::now_v7().to_string();
                let claimed = transaction.execute(
                    "UPDATE transcript_capture_jobs
                        SET claim_token = ?1, claim_expires_at_unix_ms = ?2,
                            updated_at_unix_ms = ?3
                      WHERE id = ?4 AND status = 'pending'
                        AND (
                            claim_token IS NULL
                            OR claim_expires_at_unix_ms <= ?3
                        )",
                    params![claim_token, to_i64(expires_at)?, to_i64(now)?, job_id],
                )?;
                if claimed == 1 {
                    jobs.push(select_claimed(&transaction, &job_id, &claim_token)?);
                }
            }
            transaction.commit()?;
            Ok(jobs)
        })
        .await
}

pub(super) async fn guard(
    store: &SqliteProjectStore,
    job_id: &str,
    claim_token: &str,
) -> Result<TranscriptCaptureGuard, ProjectStoreError> {
    let job_id = required_id(job_id)?;
    let claim_token = required_id(claim_token)?;
    store
        .run(move |connection| {
            select_claimed(connection, &job_id, &claim_token)?;
            select_guard(connection, &job_id)
        })
        .await
}

pub(super) async fn succeed(
    store: &SqliteProjectStore,
    job_id: &str,
    claim_token: &str,
    captured: CapturedTranscriptText,
) -> Result<TranscriptCaptureOutcome, ProjectStoreError> {
    let job_id = required_id(job_id)?;
    let claim_token = required_id(claim_token)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let job = select_claimed(&transaction, &job_id, &claim_token)?;
            let guard = select_guard(&transaction, &job_id)?;
            let now = unix_time_ms()?;
            // Re-check the durable guard atomically with the write, so text
            // read just before a reallocation or re-binding is never stored.
            let expiry = if guard.worker_reallocated {
                Some(TranscriptCaptureExpiry::WorkerReallocated)
            } else if guard.bound_elsewhere {
                Some(TranscriptCaptureExpiry::RuntimeReused)
            } else {
                None
            };
            if let Some(expiry) = expiry {
                finish_expired(&transaction, &job_id, &claim_token, expiry, now)?;
                transaction.commit()?;
                return Ok(TranscriptCaptureOutcome::Expired(expiry));
            }
            let bounded = bound_transcript_text(&captured.text);
            let provider = job.provider_session.as_ref();
            transaction.execute(
                "INSERT INTO worker_transcripts (
                    id, job_id, worker_id, assignment_id, adapter,
                    runtime_session, terminal_id, pane_id,
                    provider_session_source, provider_session_provider,
                    provider_session_kind, provider_session_value, source,
                    format, text, line_count, byte_count, truncated,
                    captured_at_unix_ms, purged_at_unix_ms, purged_by
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, NULL, NULL
                 )",
                params![
                    Uuid::now_v7().to_string(),
                    job.id,
                    job.worker_id,
                    job.assignment_id,
                    job.adapter,
                    job.session,
                    job.terminal_id,
                    job.pane_id,
                    provider.map(|session| session.source.as_str()),
                    provider.map(|session| session.provider.as_str()),
                    provider.map(|session| session.kind.as_str()),
                    provider.map(|session| session.value.as_str()),
                    captured.source,
                    captured.format,
                    bounded.text,
                    i64::try_from(bounded.line_count)?,
                    i64::try_from(bounded.byte_count)?,
                    bounded.truncated || captured.truncated,
                    to_i64(now)?,
                ],
            )?;
            let rows = transaction.execute(
                "UPDATE transcript_capture_jobs
                    SET status = 'succeeded', attempts = attempts + 1,
                        last_error = NULL, claim_token = NULL,
                        claim_expires_at_unix_ms = NULL,
                        updated_at_unix_ms = ?1, completed_at_unix_ms = ?1
                  WHERE id = ?2 AND status = 'pending' AND claim_token = ?3",
                params![to_i64(now)?, job_id, claim_token],
            )?;
            if rows != 1 {
                return Err(ProjectStoreError::TranscriptCaptureClaimLost);
            }
            transaction.commit()?;
            Ok(TranscriptCaptureOutcome::Stored)
        })
        .await
}

pub(super) async fn expire(
    store: &SqliteProjectStore,
    job_id: &str,
    claim_token: &str,
    expiry: TranscriptCaptureExpiry,
) -> Result<(), ProjectStoreError> {
    let job_id = required_id(job_id)?;
    let claim_token = required_id(claim_token)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            finish_expired(&transaction, &job_id, &claim_token, expiry, unix_time_ms()?)?;
            transaction.commit()?;
            Ok(())
        })
        .await
}

pub(super) async fn fail(
    store: &SqliteProjectStore,
    job_id: &str,
    claim_token: &str,
    message: &str,
    retry_after_ms: u64,
) -> Result<(), ProjectStoreError> {
    let job_id = required_id(job_id)?;
    let claim_token = required_id(claim_token)?;
    let message = message.trim().to_owned();
    if message.is_empty() {
        return Err(ProjectStoreError::CommandFailureMessageRequired);
    }
    store
        .run(move |connection| {
            let now = unix_time_ms()?;
            let next_attempt = now.saturating_add(retry_after_ms.max(1));
            let rows = connection.execute(
                "UPDATE transcript_capture_jobs
                    SET attempts = attempts + 1, last_error = ?1,
                        next_attempt_at_unix_ms = ?2, claim_token = NULL,
                        claim_expires_at_unix_ms = NULL,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending' AND claim_token = ?5",
                params![
                    message,
                    to_i64(next_attempt)?,
                    to_i64(now)?,
                    job_id,
                    claim_token
                ],
            )?;
            if rows != 1 {
                return Err(ProjectStoreError::TranscriptCaptureClaimLost);
            }
            Ok(())
        })
        .await
}

pub(super) async fn assignment_transcript(
    store: &SqliteProjectStore,
    project_id: &str,
    assignment_id: &str,
) -> Result<WorkerTranscript, ProjectStoreError> {
    let project_id = required_id(project_id)?;
    let assignment_id = required_assignment_id(assignment_id)?;
    store
        .run(move |connection| {
            let worker_id = connection
                .query_row(
                    "SELECT worker_id FROM assignments
                      WHERE id = ?1 AND project_id = ?2",
                    params![assignment_id, project_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or(ProjectStoreError::AssignmentNotFound)?;
            if let Some(transcript) = select_latest_for_assignment(connection, &assignment_id)? {
                return Ok(transcript);
            }
            let (terminal_id, provider_session) =
                select_assignment_runtime_reference(connection, &worker_id, &assignment_id)?
                    .map_or((None, None), |(terminal_id, provider_session)| {
                        (Some(terminal_id), provider_session)
                    });
            Ok(WorkerTranscript {
                worker_id,
                assignment_id: Some(assignment_id),
                status: TranscriptStatus::Unavailable,
                unavailable_reason: Some(TranscriptUnavailableReason::NotCaptured),
                terminal_id,
                provider_session,
                source: None,
                format: None,
                text: None,
                line_count: 0,
                byte_count: 0,
                truncated: false,
                captured_at_unix_ms: None,
                attempts: 0,
                last_error: None,
                next_attempt_at_unix_ms: None,
            })
        })
        .await
}

fn finish_expired(
    transaction: &Transaction<'_>,
    job_id: &str,
    claim_token: &str,
    expiry: TranscriptCaptureExpiry,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let rows = transaction.execute(
        "UPDATE transcript_capture_jobs
            SET status = 'expired', expired_reason = ?1,
                attempts = attempts + 1, claim_token = NULL,
                claim_expires_at_unix_ms = NULL, updated_at_unix_ms = ?2,
                completed_at_unix_ms = ?2
          WHERE id = ?3 AND status = 'pending' AND claim_token = ?4",
        params![expiry.value(), to_i64(now)?, job_id, claim_token],
    )?;
    if rows != 1 {
        return Err(ProjectStoreError::TranscriptCaptureClaimLost);
    }
    Ok(())
}

fn select_claimed(
    connection: &Connection,
    job_id: &str,
    claim_token: &str,
) -> Result<PendingTranscriptCapture, ProjectStoreError> {
    connection
        .query_row(
            "SELECT id, command_id, worker_id, assignment_id, adapter,
                    runtime_session, runtime_workspace_id, terminal_id,
                    tab_id, pane_id, provider_session_source,
                    provider_session_provider, provider_session_kind,
                    provider_session_value, attempts, claim_token
               FROM transcript_capture_jobs
              WHERE id = ?1 AND status = 'pending' AND claim_token = ?2",
            params![job_id, claim_token],
            |row| {
                Ok(PendingTranscriptCapture {
                    id: row.get(0)?,
                    command_id: row.get(1)?,
                    worker_id: row.get(2)?,
                    assignment_id: row.get(3)?,
                    adapter: row.get(4)?,
                    session: row.get(5)?,
                    workspace_id: row.get(6)?,
                    terminal_id: row.get(7)?,
                    tab_id: row.get(8)?,
                    pane_id: row.get(9)?,
                    provider_session: provider_session_from_columns(row, 10)?,
                    attempts: row.get(14)?,
                    claim_token: row.get(15)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::TranscriptCaptureClaimLost)
}

fn select_guard(
    connection: &Connection,
    job_id: &str,
) -> Result<TranscriptCaptureGuard, ProjectStoreError> {
    connection
        .query_row(
            "SELECT
                EXISTS (
                    SELECT 1 FROM worker_runtime_bindings binding
                     WHERE binding.worker_id = job.worker_id
                       AND binding.adapter = job.adapter
                       AND binding.runtime_session = job.runtime_session
                       AND binding.runtime_workspace_id = job.runtime_workspace_id
                       AND binding.terminal_id = job.terminal_id
                       AND binding.pane_id = job.pane_id
                       AND binding.tab_id IS job.tab_id
                ),
                job.allocation_id IS NOT NULL AND EXISTS (
                    SELECT 1 FROM worker_allocations newer
                     WHERE newer.worker_id = job.worker_id
                       AND newer.id <> job.allocation_id
                       AND newer.started_at_unix_ms >= job.created_at_unix_ms
                ),
                EXISTS (
                    SELECT 1 FROM worker_runtime_bindings binding
                     WHERE binding.worker_id <> job.worker_id
                       AND binding.adapter = job.adapter
                       AND binding.runtime_session = job.runtime_session
                       AND (
                           binding.terminal_id = job.terminal_id
                           OR (
                               job.provider_session_source IS NOT NULL
                               AND binding.provider_session_source
                                   = job.provider_session_source
                               AND binding.provider_session_provider
                                   = job.provider_session_provider
                               AND binding.provider_session_kind
                                   = job.provider_session_kind
                               AND binding.provider_session_value
                                   = job.provider_session_value
                           )
                       )
                )
               FROM transcript_capture_jobs job
              WHERE job.id = ?1",
            [job_id],
            |row| {
                Ok(TranscriptCaptureGuard {
                    bound_to_worker: row.get(0)?,
                    worker_reallocated: row.get(1)?,
                    bound_elsewhere: row.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::TranscriptCaptureClaimLost)
}

fn select_latest_for_assignment(
    connection: &Connection,
    assignment_id: &str,
) -> Result<Option<WorkerTranscript>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT job.worker_id, job.assignment_id, job.status,
                    job.expired_reason, job.terminal_id,
                    job.provider_session_source, job.provider_session_provider,
                    job.provider_session_kind, job.provider_session_value,
                    transcript.source, transcript.format, transcript.text,
                    transcript.line_count, transcript.byte_count,
                    transcript.truncated, transcript.captured_at_unix_ms,
                    job.attempts, job.last_error, job.next_attempt_at_unix_ms
               FROM transcript_capture_jobs job
               LEFT JOIN worker_transcripts transcript ON transcript.job_id = job.id
              WHERE job.assignment_id = ?1
              ORDER BY job.created_at_unix_ms DESC, job.id DESC
              LIMIT 1",
            [assignment_id],
            transcript_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn transcript_from_row(row: &Row<'_>) -> rusqlite::Result<WorkerTranscript> {
    let job_status = row.get::<_, String>(2)?;
    let expired_reason = row.get::<_, Option<String>>(3)?;
    let captured_at = row_optional_u64(row, 15)?;
    let (status, unavailable_reason) = match job_status.as_str() {
        "pending" => (TranscriptStatus::Pending, None),
        "succeeded" if captured_at.is_some() => (TranscriptStatus::Captured, None),
        "succeeded" => (
            TranscriptStatus::Unavailable,
            Some(TranscriptUnavailableReason::NotCaptured),
        ),
        "expired" => (
            TranscriptStatus::Unavailable,
            Some(match expired_reason.as_deref() {
                Some("runtime_closed") => TranscriptUnavailableReason::RuntimeClosed,
                Some("runtime_reused") => TranscriptUnavailableReason::RuntimeReused,
                Some("worker_reallocated") => TranscriptUnavailableReason::WorkerReallocated,
                value => {
                    return Err(super::enum_conversion_error(
                        3,
                        "transcript expiry reason",
                        value.unwrap_or_default(),
                    ));
                }
            }),
        ),
        value => {
            return Err(super::enum_conversion_error(
                2,
                "transcript job status",
                value,
            ));
        }
    };
    let pending = status == TranscriptStatus::Pending;
    Ok(WorkerTranscript {
        worker_id: row.get(0)?,
        assignment_id: row.get(1)?,
        status,
        unavailable_reason,
        terminal_id: row.get(4)?,
        provider_session: provider_session_from_columns(row, 5)?,
        source: row.get(9)?,
        format: row.get(10)?,
        text: row.get(11)?,
        line_count: row.get::<_, Option<u32>>(12)?.unwrap_or_default(),
        byte_count: row_optional_u64(row, 13)?.unwrap_or_default(),
        truncated: row.get::<_, Option<bool>>(14)?.unwrap_or_default(),
        captured_at_unix_ms: captured_at,
        attempts: row.get(16)?,
        last_error: row.get(17)?,
        next_attempt_at_unix_ms: if pending {
            Some(row_u64(row, 18)?)
        } else {
            None
        },
    })
}

/// The worker's current runtime reference, else its most recently retired
/// one, so an uncaptured transcript can still name the provider session.
type RuntimeReference = (String, Option<ProviderSessionRef>);

/// The runtime an uncaptured assignment provably ran in, or `None`.
///
/// A worker's binding moves on (handoff, session end, a later allocation),
/// so its current binding is used only while nothing could have replaced
/// it: the assignment's allocation is still open, or no later allocation
/// exists and no binding was retired since the allocation started.
/// Otherwise the first binding retired inside the allocation window is the
/// assignment's own; with none, no reference is returned.
fn select_assignment_runtime_reference(
    connection: &Connection,
    worker_id: &str,
    assignment_id: &str,
) -> Result<Option<RuntimeReference>, ProjectStoreError> {
    let Some((allocation_id, started_at, ended_at)) = connection
        .query_row(
            "SELECT wa.id, wa.started_at_unix_ms, wa.ended_at_unix_ms
               FROM assignments a
               JOIN worker_allocations wa ON wa.id = a.allocation_id
              WHERE a.id = ?1 AND wa.worker_id = ?2",
            params![assignment_id, worker_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let Some(ended_at) = ended_at else {
        return select_current_runtime_reference(connection, worker_id);
    };
    let later_allocation = connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM worker_allocations
             WHERE worker_id = ?1 AND id <> ?2 AND started_at_unix_ms >= ?3
         )",
        params![worker_id, allocation_id, started_at],
        |row| row.get::<_, bool>(0),
    )?;
    // A binding retired in the same instant the allocation started belongs
    // to the previous allocation (a handoff retires the source and starts
    // the target in one transaction), so the lower bound is strict.
    let retired = connection
        .query_row(
            "SELECT terminal_id, provider_session_source, provider_session_provider,
                    provider_session_kind, provider_session_value
               FROM retired_runtime_bindings
              WHERE worker_id = ?1
                AND retired_at_unix_ms > ?2
                AND (?3 IS NULL OR retired_at_unix_ms <= ?3)
              ORDER BY retired_at_unix_ms ASC, id ASC
              LIMIT 1",
            params![worker_id, started_at, later_allocation.then_some(ended_at)],
            |row| Ok((row.get(0)?, provider_session_from_columns(row, 1)?)),
        )
        .optional()?;
    if retired.is_some() || later_allocation {
        return Ok(retired);
    }
    select_current_runtime_reference(connection, worker_id)
}

fn select_current_runtime_reference(
    connection: &Connection,
    worker_id: &str,
) -> Result<Option<RuntimeReference>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT terminal_id, provider_session_source, provider_session_provider,
                    provider_session_kind, provider_session_value
               FROM worker_runtime_bindings
              WHERE worker_id = ?1",
            [worker_id],
            |row| Ok((row.get(0)?, provider_session_from_columns(row, 1)?)),
        )
        .optional()
        .map_err(Into::into)
}

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use uuid::Uuid;
use yard_domain::{
    CleanupAdvisorArtifact, CleanupAdvisorRecommendation, CleanupAdvisorState,
    CompletedRuntimeCleanupCandidate, CompletedRuntimeCleanupPreview,
    CompletedRuntimeRetentionReason, StartWorkerCleanupRun, UpdateWorkerCleanupPolicy,
    WorkerCleanupDashboard, WorkerCleanupItemStatus, WorkerCleanupPolicy, WorkerCleanupRun,
    WorkerCleanupRunItem, WorkerCleanupRunStatus, WorkerCleanupRunTrigger, WorkerCleanupRuns,
};

use super::{
    ClaimedWorkerCleanupItem, MAX_COMPLETED_RUNTIME_CLEANUP_PREVIEW_LIMIT, ProjectStoreError,
    SqliteProjectStore, WorkerRuntimeBinding, insert_lifecycle_event, row_optional_u64, row_u64,
    to_i64, unix_time_ms,
};

const MAX_RUN_LIST_LIMIT: usize = 50;
const CANDIDATE_SCAN_LIMIT: usize = 500;

struct CleanupCandidate {
    public: CompletedRuntimeCleanupCandidate,
    runtime: WorkerRuntimeBinding,
    worker_version: u64,
    is_cleanup_advisor: bool,
}

pub(super) async fn dashboard(
    store: &SqliteProjectStore,
    preview_limit: usize,
    run_limit: usize,
) -> Result<WorkerCleanupDashboard, ProjectStoreError> {
    validate_limits(preview_limit, run_limit)?;
    store
        .run(move |connection| {
            let policy = select_policy(connection)?;
            let candidates = select_candidates(
                connection,
                preview_limit + 1,
                policy.grace_period_ms,
                unix_time_ms()?,
            )?;
            let truncated = candidates.len() > preview_limit;
            let mut candidates = candidates;
            candidates.truncate(preview_limit);
            let close_ready_count = candidates
                .iter()
                .filter(|candidate| candidate.public.close_eligible)
                .count();
            Ok(WorkerCleanupDashboard {
                policy,
                preview: CompletedRuntimeCleanupPreview {
                    candidate_count: candidates.len(),
                    close_ready_count,
                    limit: preview_limit,
                    truncated,
                    candidates: candidates.into_iter().map(|item| item.public).collect(),
                },
                runs: select_runs(connection, run_limit)?,
            })
        })
        .await
}

pub(super) async fn get_policy(
    store: &SqliteProjectStore,
) -> Result<WorkerCleanupPolicy, ProjectStoreError> {
    store.run(|connection| select_policy(connection)).await
}

pub(super) async fn update_policy(
    store: &SqliteProjectStore,
    command: UpdateWorkerCleanupPolicy,
) -> Result<WorkerCleanupPolicy, ProjectStoreError> {
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = select_policy(&transaction)?;
            if current.version != command.expected_version {
                return Err(ProjectStoreError::WorkerCleanupPolicyVersionConflict {
                    current_version: current.version,
                });
            }
            let version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE worker_cleanup_policy
                    SET automatic_enabled = ?1, schedule_minutes = ?2,
                        grace_period_ms = ?3, batch_size = ?4,
                        advisor_profile_id = ?5, version = ?6, updated_by = ?7,
                        updated_at_unix_ms = ?8
                  WHERE singleton_id = 1 AND version = ?9",
                params![
                    command.automatic_enabled,
                    to_i64(command.schedule_minutes)?,
                    to_i64(command.grace_period_ms)?,
                    i64::try_from(command.batch_size)?,
                    command.advisor_profile_id,
                    to_i64(version)?,
                    command.actor,
                    to_i64(now)?,
                    to_i64(current.version)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "worker_cleanup_policy",
                "singleton",
                version,
                "worker_cleanup_policy_updated",
                &command.actor,
                now,
            )?;
            let policy = select_policy(&transaction)?;
            transaction.commit()?;
            Ok(policy)
        })
        .await
}

pub(super) async fn start_run(
    store: &SqliteProjectStore,
    trigger: WorkerCleanupRunTrigger,
    command: StartWorkerCleanupRun,
) -> Result<WorkerCleanupRun, ProjectStoreError> {
    let command = command.normalize()?;
    if command.preview != matches!(trigger, WorkerCleanupRunTrigger::Preview) {
        return Err(ProjectStoreError::WorkerCleanupRunIdConflict);
    }
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_run_by_command(&transaction, &command.command_id)? {
                if existing.trigger != trigger
                    || existing.preview != command.preview
                    || existing.requested_by != command.actor
                {
                    return Err(ProjectStoreError::WorkerCleanupRunIdConflict);
                }
                return Ok(existing);
            }
            let policy = select_policy(&transaction)?;
            let now = unix_time_ms()?;
            let run_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO worker_cleanup_runs (
                    id, command_id, trigger, preview, status, advisor_state,
                    requested_by, policy_version, grace_period_ms, batch_size,
                    advisor_profile_id, cancellation_requested, last_error,
                    created_at_unix_ms, updated_at_unix_ms, completed_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, 'pending', 'not_requested', ?5, ?6, ?7,
                    ?8, ?9, 0, NULL, ?10, ?10, NULL
                 )",
                params![
                    run_id,
                    command.command_id,
                    trigger_value(trigger),
                    command.preview,
                    command.actor,
                    to_i64(policy.version)?,
                    to_i64(policy.grace_period_ms)?,
                    i64::try_from(policy.batch_size)?,
                    policy.advisor_profile_id,
                    to_i64(now)?,
                ],
            )?;
            let mut inserted = 0;
            let mut offset = 0;
            while inserted < policy.batch_size {
                let candidates = select_candidates_page(
                    &transaction,
                    CANDIDATE_SCAN_LIMIT,
                    offset,
                    policy.grace_period_ms,
                    now,
                )?;
                let scanned = candidates.len();
                for candidate in candidates
                    .into_iter()
                    .filter(|candidate| candidate.public.close_eligible)
                    .take(policy.batch_size - inserted)
                {
                    insert_item(&transaction, &run_id, candidate, now)?;
                    inserted += 1;
                }
                offset += scanned;
                if scanned < CANDIDATE_SCAN_LIMIT {
                    break;
                }
            }
            if inserted == 0 {
                transaction.execute(
                    "UPDATE worker_cleanup_runs
                        SET status = 'completed', completed_at_unix_ms = ?1,
                            updated_at_unix_ms = ?1
                      WHERE id = ?2",
                    params![to_i64(now)?, run_id],
                )?;
            }
            insert_lifecycle_event(
                &transaction,
                "worker_cleanup_run",
                &run_id,
                1,
                "worker_cleanup_run_started",
                &command.actor,
                now,
            )?;
            let run = select_run(&transaction, &run_id)?;
            transaction.commit()?;
            Ok(run)
        })
        .await
}

pub(super) async fn list_runs(
    store: &SqliteProjectStore,
    limit: usize,
) -> Result<WorkerCleanupRuns, ProjectStoreError> {
    validate_run_limit(limit)?;
    store
        .run(move |connection| select_runs(connection, limit))
        .await
}

pub(super) async fn get_run(
    store: &SqliteProjectStore,
    run_id: &str,
) -> Result<WorkerCleanupRun, ProjectStoreError> {
    let run_id = run_id.trim().to_owned();
    store
        .run(move |connection| select_run(connection, &run_id))
        .await
}

pub(super) async fn cancel_run(
    store: &SqliteProjectStore,
    run_id: &str,
    command: yard_domain::CancelWorkerCleanupRun,
) -> Result<WorkerCleanupRun, ProjectStoreError> {
    let command = command.normalize()?;
    let run_id = run_id.trim().to_owned();
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let run = select_run(&transaction, &run_id)?;
            let existing_cancellation = transaction.query_row(
                "SELECT cancellation_command_id, cancelled_by
                   FROM worker_cleanup_runs WHERE id = ?1",
                [&run_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )?;
            if let Some(existing_command_id) = existing_cancellation.0 {
                if existing_command_id == command.command_id
                    && existing_cancellation.1.as_deref() == Some(&command.actor)
                {
                    return Ok(run);
                }
                return Err(ProjectStoreError::WorkerCleanupRunIdConflict);
            }
            let command_conflict = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM worker_cleanup_runs
                     WHERE command_id = ?1 OR cancellation_command_id = ?1
                )",
                [&command.command_id],
                |row| row.get::<_, bool>(0),
            )?;
            if command_conflict {
                return Err(ProjectStoreError::WorkerCleanupRunIdConflict);
            }
            let now = unix_time_ms()?;
            if matches!(
                run.status,
                WorkerCleanupRunStatus::Completed
                    | WorkerCleanupRunStatus::Cancelled
                    | WorkerCleanupRunStatus::Failed
            ) {
                transaction.execute(
                    "UPDATE worker_cleanup_runs
                        SET cancellation_command_id = ?1, cancelled_by = ?2,
                            updated_at_unix_ms = ?3
                      WHERE id = ?4 AND cancellation_command_id IS NULL",
                    params![command.command_id, command.actor, to_i64(now)?, run_id],
                )?;
                insert_lifecycle_event(
                    &transaction,
                    "worker_cleanup_run",
                    &run_id,
                    1,
                    "worker_cleanup_cancel_noop",
                    &command.actor,
                    now,
                )?;
                let run = select_run(&transaction, &run_id)?;
                transaction.commit()?;
                return Ok(run);
            }
            transaction.execute(
                "UPDATE worker_cleanup_runs
                    SET cancellation_requested = 1, cancellation_command_id = ?1,
                        cancelled_by = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4 AND cancellation_command_id IS NULL",
                params![command.command_id, command.actor, to_i64(now)?, run_id],
            )?;
            transaction.execute(
                "UPDATE worker_cleanup_run_items
                    SET status = 'cancelled', reason = ?1, claim_token = NULL,
                        claim_expires_at_unix_ms = NULL, updated_at_unix_ms = ?2
                  WHERE run_id = ?3 AND status = 'pending' AND claim_token IS NULL",
                params![
                    format!("cancelled_by:{}", command.actor),
                    to_i64(now)?,
                    run_id
                ],
            )?;
            update_run_summary(&transaction, &run_id, now)?;
            insert_lifecycle_event(
                &transaction,
                "worker_cleanup_run",
                &run_id,
                1,
                "worker_cleanup_cancel_requested",
                &command.actor,
                now,
            )?;
            let run = select_run(&transaction, &run_id)?;
            transaction.commit()?;
            Ok(run)
        })
        .await
}

pub(super) async fn claim_items(
    store: &SqliteProjectStore,
    run_id: Option<&str>,
    limit: usize,
    claim_ttl_ms: u64,
) -> Result<Vec<ClaimedWorkerCleanupItem>, ProjectStoreError> {
    if limit == 0 || limit > 100 || claim_ttl_ms == 0 {
        return Err(ProjectStoreError::WorkerCleanupRunListLimitInvalid { max: 100 });
    }
    let run_id = run_id.map(str::trim).map(str::to_owned);
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now = unix_time_ms()?;
            let mut cancelled_statement = transaction.prepare(
                "SELECT DISTINCT item.run_id
                   FROM worker_cleanup_run_items item
                   JOIN worker_cleanup_runs run ON run.id = item.run_id
                  WHERE run.cancellation_requested = 1
                    AND item.status = 'pending'
                    AND (
                        item.claim_token IS NULL
                        OR item.claim_expires_at_unix_ms <= ?1
                    )",
            )?;
            let cancelled_runs = cancelled_statement
                .query_map([to_i64(now)?], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(cancelled_statement);
            transaction.execute(
                "UPDATE worker_cleanup_run_items
                    SET status = 'cancelled', reason = 'cancelled_after_claim',
                        claim_token = NULL, claim_expires_at_unix_ms = NULL,
                        updated_at_unix_ms = ?1
                  WHERE status = 'pending'
                    AND (
                        claim_token IS NULL
                        OR claim_expires_at_unix_ms <= ?1
                    )
                    AND run_id IN (
                        SELECT id FROM worker_cleanup_runs
                         WHERE cancellation_requested = 1
                    )",
                [to_i64(now)?],
            )?;
            for cancelled_run in cancelled_runs {
                update_run_summary(&transaction, &cancelled_run, now)?;
            }
            let expires = now.saturating_add(claim_ttl_ms);
            let mut statement = transaction.prepare(
                "SELECT item.run_id, item.worker_id
                   FROM worker_cleanup_run_items item
                   JOIN worker_cleanup_runs run ON run.id = item.run_id
                  WHERE item.status = 'pending'
                    AND item.next_attempt_at_unix_ms <= ?1
                    AND (item.claim_token IS NULL OR item.claim_expires_at_unix_ms <= ?1)
                    AND run.cancellation_requested = 0
                    AND run.status IN ('pending', 'running')
                    AND (?2 IS NULL OR item.run_id = ?2)
                  ORDER BY run.created_at_unix_ms, item.worker_id
                  LIMIT ?3",
            )?;
            let keys = statement
                .query_map(
                    params![to_i64(now)?, run_id, i64::try_from(limit)?],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let mut claimed = Vec::with_capacity(keys.len());
            for (run_id, worker_id) in keys {
                let token = Uuid::now_v7().to_string();
                let changed = transaction.execute(
                    "UPDATE worker_cleanup_run_items
                        SET claim_token = ?1, claim_expires_at_unix_ms = ?2,
                            attempts = attempts + 1, updated_at_unix_ms = ?3
                      WHERE run_id = ?4 AND worker_id = ?5 AND status = 'pending'
                        AND (claim_token IS NULL OR claim_expires_at_unix_ms <= ?3)",
                    params![token, to_i64(expires)?, to_i64(now)?, run_id, worker_id],
                )?;
                if changed == 1 {
                    transaction.execute(
                        "UPDATE worker_cleanup_runs
                            SET status = 'running', updated_at_unix_ms = ?1
                          WHERE id = ?2 AND status = 'pending'",
                        params![to_i64(now)?, run_id],
                    )?;
                    claimed.push(select_claimed_item(&transaction, &run_id, &worker_id)?);
                }
            }
            transaction.commit()?;
            Ok(claimed)
        })
        .await
}

pub(super) async fn finish_item(
    store: &SqliteProjectStore,
    run_id: &str,
    worker_id: &str,
    claim_token: &str,
    status: WorkerCleanupItemStatus,
    reason: &str,
) -> Result<(), ProjectStoreError> {
    let run_id = run_id.trim().to_owned();
    let worker_id = worker_id.trim().to_owned();
    let claim_token = claim_token.trim().to_owned();
    let reason = reason.trim().to_owned();
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now = unix_time_ms()?;
            if matches!(
                status,
                WorkerCleanupItemStatus::Retired | WorkerCleanupItemStatus::Reconciled
            ) {
                archive_binding(
                    &transaction,
                    &run_id,
                    &worker_id,
                    &claim_token,
                    status == WorkerCleanupItemStatus::Retired,
                    now,
                )?;
            }
            let changed = transaction.execute(
                "UPDATE worker_cleanup_run_items
                    SET status = ?1, reason = ?2, claim_token = NULL,
                        claim_expires_at_unix_ms = NULL, updated_at_unix_ms = ?3
                  WHERE run_id = ?4 AND worker_id = ?5 AND claim_token = ?6
                    AND status = 'pending'",
                params![
                    item_status_value(status),
                    reason,
                    to_i64(now)?,
                    run_id,
                    worker_id,
                    claim_token
                ],
            )?;
            if changed != 1 {
                return Err(ProjectStoreError::WorkerCleanupClaimConflict);
            }
            update_run_summary(&transaction, &run_id, now)?;
            transaction.commit()?;
            Ok(())
        })
        .await
}

pub(super) async fn retry_item(
    store: &SqliteProjectStore,
    run_id: &str,
    worker_id: &str,
    claim_token: &str,
    reason: &str,
    retry_after_ms: u64,
) -> Result<(), ProjectStoreError> {
    let run_id = run_id.trim().to_owned();
    let worker_id = worker_id.trim().to_owned();
    let claim_token = claim_token.trim().to_owned();
    let reason = reason.trim().to_owned();
    store
        .run(move |connection| {
            let now = unix_time_ms()?;
            let changed = connection.execute(
                "UPDATE worker_cleanup_run_items
                    SET reason = ?1, next_attempt_at_unix_ms = ?2,
                        claim_token = NULL, claim_expires_at_unix_ms = NULL,
                        updated_at_unix_ms = ?3
                  WHERE run_id = ?4 AND worker_id = ?5 AND claim_token = ?6
                    AND status = 'pending'",
                params![
                    reason,
                    to_i64(now.saturating_add(retry_after_ms))?,
                    to_i64(now)?,
                    run_id,
                    worker_id,
                    claim_token,
                ],
            )?;
            if changed == 1 {
                Ok(())
            } else {
                Err(ProjectStoreError::WorkerCleanupClaimConflict)
            }
        })
        .await
}

pub(super) async fn reconcile_missing(
    store: &SqliteProjectStore,
    item: &ClaimedWorkerCleanupItem,
    observed_at_unix_ms: u64,
) -> Result<(), ProjectStoreError> {
    finish_item(
        store,
        &item.run_id,
        &item.worker_id,
        &item.claim_token,
        WorkerCleanupItemStatus::Reconciled,
        &format!("pane_missing_at:{observed_at_unix_ms}"),
    )
    .await
}

pub(super) async fn authorize_close(
    store: &SqliteProjectStore,
    item: &ClaimedWorkerCleanupItem,
) -> Result<(), ProjectStoreError> {
    let item = item.clone();
    store
        .run(move |connection| {
            if cleanup_item_authorized(
                connection,
                &item.run_id,
                &item.worker_id,
                &item.claim_token,
                true,
                unix_time_ms()?,
            )? {
                Ok(())
            } else {
                Err(ProjectStoreError::WorkerCleanupRevalidationFailed)
            }
        })
        .await
}

pub(super) async fn record_advisor_assignment(
    store: &SqliteProjectStore,
    item: &ClaimedWorkerCleanupItem,
    parent_worker_id: &str,
    advisor_worker_id: &str,
    advisor_assignment_id: &str,
) -> Result<(), ProjectStoreError> {
    let item = item.clone();
    let parent_worker_id = parent_worker_id.trim().to_owned();
    let advisor_worker_id = advisor_worker_id.trim().to_owned();
    let advisor_assignment_id = advisor_assignment_id.trim().to_owned();
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT advisor_worker_id, advisor_assignment_id
                       FROM worker_cleanup_run_items
                      WHERE run_id = ?1 AND worker_id = ?2 AND claim_token = ?3",
                    params![item.run_id, item.worker_id, item.claim_token],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(ProjectStoreError::WorkerCleanupClaimConflict)?;
            if existing.0.as_deref() == Some(&advisor_worker_id)
                && existing.1.as_deref() == Some(&advisor_assignment_id)
            {
                return Ok(());
            }
            if existing.0.is_some() || existing.1.is_some() {
                return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
            }
            let Some(advisor_profile_id) = item.advisor_profile_id.as_deref() else {
                return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
            };
            let valid = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM assignments advisor
                    JOIN workers worker ON worker.id = advisor.worker_id
                    JOIN worker_allocations allocation
                      ON allocation.id = advisor.allocation_id
                     AND allocation.worker_id = worker.id
                    JOIN worker_runtime_bindings runtime ON runtime.worker_id = worker.id
                    WHERE advisor.id = ?1 AND worker.id = ?2
                      AND advisor.project_id = ?3
                      AND runtime.runtime_workspace_id = ?4
                      AND advisor.profile_id = ?5
                      AND advisor.role = 'cleanup_advisor'
                      AND advisor.lifecycle = 'completed'
                      AND allocation.mode = 'create_new'
                      AND worker.ownership_kind = 'yard_owned'
                      AND worker.parent_worker_id IS NULL
                      AND worker.ended_at_unix_ms IS NULL
                      AND worker.id <> ?6
                      AND NOT EXISTS (
                          SELECT 1 FROM projects
                           WHERE orchestrator_worker_id = worker.id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM yard_orchestrator
                           WHERE worker_id = worker.id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM coordination_nodes
                           WHERE worker_id = worker.id
                      )
                 )",
                params![
                    advisor_assignment_id,
                    advisor_worker_id,
                    item.project_id,
                    item.runtime.workspace_id,
                    advisor_profile_id,
                    item.worker_id,
                ],
                |row| row.get::<_, bool>(0),
            )?;
            if !valid || parent_worker_id != item.worker_id {
                return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
            }
            let worker_rows = transaction.execute(
                "UPDATE workers
                    SET ownership_kind = 'system_ephemeral', parent_worker_id = ?1
                  WHERE id = ?2 AND ownership_kind = 'yard_owned'
                    AND parent_worker_id IS NULL AND ended_at_unix_ms IS NULL",
                params![parent_worker_id, advisor_worker_id],
            )?;
            let item_rows = transaction.execute(
                "UPDATE worker_cleanup_run_items
                    SET advisor_worker_id = ?1, advisor_assignment_id = ?2,
                        updated_at_unix_ms = ?3
                  WHERE run_id = ?4 AND worker_id = ?5 AND claim_token = ?6
                    AND advisor_worker_id IS NULL AND advisor_assignment_id IS NULL",
                params![
                    advisor_worker_id,
                    advisor_assignment_id,
                    to_i64(unix_time_ms()?)?,
                    item.run_id,
                    item.worker_id,
                    item.claim_token
                ],
            )?;
            if worker_rows != 1 || item_rows != 1 {
                return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
            }
            transaction.execute(
                "UPDATE worker_cleanup_runs
                    SET advisor_state = 'pending'
                  WHERE id = ?1",
                [item.run_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
}

pub(super) async fn record_advisor_artifact(
    store: &SqliteProjectStore,
    item: &ClaimedWorkerCleanupItem,
    completion_receipt_id: &str,
    artifact_id: &str,
    artifact: CleanupAdvisorArtifact,
) -> Result<(), ProjectStoreError> {
    let item = item.clone();
    let completion_receipt_id = completion_receipt_id.trim().to_owned();
    let artifact_id = artifact_id.trim().to_owned();
    let artifact = artifact.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (advisor_worker_id, advisor_assignment_id) = transaction.query_row(
                    "SELECT advisor_worker_id, advisor_assignment_id
                       FROM worker_cleanup_run_items
                      WHERE run_id = ?1 AND worker_id = ?2 AND claim_token = ?3",
                    params![item.run_id, item.worker_id, item.claim_token],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )?;
            let advisor_worker_id =
                advisor_worker_id.ok_or(ProjectStoreError::WorkerCleanupRevalidationFailed)?;
            let advisor_assignment_id =
                advisor_assignment_id.ok_or(ProjectStoreError::WorkerCleanupRevalidationFailed)?;
            let existing = transaction
                .query_row(
                    "SELECT completion_receipt_id, artifact_id, recommendation,
                            evidence_ids_json, rationale
                       FROM cleanup_advisor_artifacts
                      WHERE run_id = ?1 AND target_worker_id = ?2",
                    params![item.run_id, item.worker_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            let evidence_json = serde_json::to_string(&artifact.evidence_ids)?;
            if let Some(existing) = existing {
                if existing.0 == completion_receipt_id
                    && existing.1 == artifact_id
                    && existing.2 == recommendation_value(artifact.recommendation)
                    && existing.3 == evidence_json
                    && existing.4 == artifact.rationale
                {
                    return Ok(());
                }
                return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
            }
            let valid_artifact = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM artifacts artifact
                    JOIN completion_receipt_artifact_links link
                      ON link.artifact_id = artifact.id
                    JOIN completion_receipts receipt ON receipt.id = link.receipt_id
                    JOIN assignments assignment ON assignment.id = receipt.assignment_id
                    WHERE artifact.id = ?1 AND artifact.assignment_id = ?2
                      AND artifact.worker_id = ?3
                      AND receipt.id = ?4 AND receipt.assignment_id = ?2
                      AND assignment.lifecycle = 'completed'
                      AND NOT EXISTS (
                          SELECT 1 FROM completion_receipt_blockers blocker
                           WHERE blocker.receipt_id = receipt.id
                      )
                 )",
                params![
                    artifact_id,
                    advisor_assignment_id,
                    advisor_worker_id,
                    completion_receipt_id
                ],
                |row| row.get::<_, bool>(0),
            )?;
            if !valid_artifact {
                return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO cleanup_advisor_artifacts (
                    artifact_id, run_id, target_worker_id, completion_receipt_id,
                    recommendation, evidence_ids_json, rationale, recorded_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![artifact_id, item.run_id, item.worker_id, completion_receipt_id, recommendation_value(artifact.recommendation), evidence_json, artifact.rationale, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE worker_cleanup_run_items
                    SET advisor_recommendation = ?1,
                        advisor_completion_receipt_id = ?2,
                        advisor_artifact_id = ?3, advisor_rationale = ?4,
                        updated_at_unix_ms = ?5
                  WHERE run_id = ?6 AND worker_id = ?7 AND claim_token = ?8",
                params![recommendation_value(artifact.recommendation), completion_receipt_id, artifact_id, artifact.rationale, to_i64(now)?, item.run_id, item.worker_id, item.claim_token],
            )?;
            transaction.execute(
                "UPDATE worker_cleanup_runs SET advisor_state = 'completed', updated_at_unix_ms = ?1 WHERE id = ?2",
                params![to_i64(now)?, item.run_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
}

fn validate_limits(preview_limit: usize, run_limit: usize) -> Result<(), ProjectStoreError> {
    if !(1..=MAX_COMPLETED_RUNTIME_CLEANUP_PREVIEW_LIMIT).contains(&preview_limit) {
        return Err(
            ProjectStoreError::CompletedRuntimeCleanupPreviewLimitInvalid {
                max: MAX_COMPLETED_RUNTIME_CLEANUP_PREVIEW_LIMIT,
            },
        );
    }
    validate_run_limit(run_limit)
}

fn validate_run_limit(limit: usize) -> Result<(), ProjectStoreError> {
    if (1..=MAX_RUN_LIST_LIMIT).contains(&limit) {
        Ok(())
    } else {
        Err(ProjectStoreError::WorkerCleanupRunListLimitInvalid {
            max: MAX_RUN_LIST_LIMIT,
        })
    }
}

fn select_policy(connection: &Connection) -> Result<WorkerCleanupPolicy, ProjectStoreError> {
    connection
        .query_row(
            "SELECT automatic_enabled, schedule_minutes, grace_period_ms,
                    batch_size, advisor_profile_id, version, updated_by,
                    updated_at_unix_ms
               FROM worker_cleanup_policy WHERE singleton_id = 1",
            [],
            |row| {
                Ok(WorkerCleanupPolicy {
                    automatic_enabled: row.get(0)?,
                    schedule_minutes: row_u64(row, 1)?,
                    grace_period_ms: row_u64(row, 2)?,
                    batch_size: usize::try_from(row.get::<_, i64>(3)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    advisor_profile_id: row.get(4)?,
                    version: row_u64(row, 5)?,
                    updated_by: row.get(6)?,
                    updated_at_unix_ms: row_u64(row, 7)?,
                })
            },
        )
        .map_err(Into::into)
}

fn select_candidates(
    connection: &Connection,
    limit: usize,
    grace_period_ms: u64,
    now: u64,
) -> Result<Vec<CleanupCandidate>, ProjectStoreError> {
    select_candidates_page(connection, limit, 0, grace_period_ms, now)
}

fn select_candidates_page(
    connection: &Connection,
    limit: usize,
    offset: usize,
    grace_period_ms: u64,
    now: u64,
) -> Result<Vec<CleanupCandidate>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "WITH ranked_allocations AS (
            SELECT wa.id, wa.worker_id, wa.project_id, wa.mode,
                   ROW_NUMBER() OVER (
                       PARTITION BY wa.worker_id
                       ORDER BY wa.started_at_unix_ms DESC, wa.id DESC
                   ) AS allocation_rank
              FROM worker_allocations wa
         )
         SELECT w.id, pr.name, a.project_id, p.name, a.id, a.role, cr.id,
                cr.created_at_unix_ms,
                (SELECT COUNT(*) FROM completion_receipt_artifact_links link
                  WHERE link.receipt_id = cr.id),
                w.version, w.ownership_kind, w.parent_worker_id,
                wrb.adapter, wrb.runtime_session, wrb.runtime_workspace_id,
                wrb.terminal_id, wrb.tab_id, wrb.pane_id,
                wrb.provider_session_source, wrb.provider_session_provider,
                wrb.provider_session_kind, wrb.provider_session_value,
                wrb.owns_tab, wrb.observation_state, wrb.process_state,
                wrb.observed_status, wrb.state_change_sequence,
                wrb.runtime_revision, wrb.version, wrb.last_observed_at_unix_ms,
                pwb.adapter, pwb.runtime_session, pwb.runtime_workspace_id,
                EXISTS (SELECT 1 FROM completion_receipt_blockers blocker
                         WHERE blocker.receipt_id = cr.id),
                EXISTS (SELECT 1 FROM assignments open_assignment
                         WHERE open_assignment.worker_id = w.id
                           AND open_assignment.lifecycle IN ('allocating', 'active', 'handing_off')),
                EXISTS (SELECT 1 FROM assignment_prompt_commands prompt
                         JOIN command_acknowledgements command ON command.id = prompt.command_id
                         WHERE prompt.assignment_id = a.id
                           AND command.status IN ('pending', 'ambiguous')),
                EXISTS (SELECT 1 FROM worker_handoff_commands handoff
                         WHERE handoff.worker_id = w.id AND handoff.finished_at_unix_ms IS NULL),
                EXISTS (SELECT 1 FROM projects protected WHERE protected.orchestrator_worker_id = w.id)
                  OR EXISTS (SELECT 1 FROM yard_orchestrator WHERE worker_id = w.id),
                EXISTS (SELECT 1 FROM coordination_nodes WHERE worker_id = w.id),
                EXISTS (SELECT 1 FROM worker_cleanup_pins WHERE worker_id = w.id),
                EXISTS (SELECT 1 FROM worker_runtime_bindings conflict
                         WHERE conflict.worker_id <> w.id
                           AND conflict.adapter = wrb.adapter
                           AND conflict.runtime_session = wrb.runtime_session
                           AND conflict.pane_id = wrb.pane_id),
                CASE WHEN w.parent_worker_id IS NULL THEN 0 ELSE NOT EXISTS (
                    SELECT 1 FROM workers parent
                    JOIN ranked_allocations parent_allocation
                      ON parent_allocation.worker_id = parent.id
                     AND parent_allocation.allocation_rank = 1
                    WHERE parent.id = w.parent_worker_id
                      AND parent.ownership_kind IN ('yard_owned', 'system_ephemeral')
                      AND parent_allocation.project_id = a.project_id
                ) END,
                EXISTS (
                    SELECT 1 FROM artifacts artifact
                    JOIN completion_receipt_artifact_links link ON link.artifact_id = artifact.id
                    JOIN cleanup_advisor_artifacts advisor ON advisor.artifact_id = artifact.id
                    WHERE link.receipt_id = cr.id AND artifact.worker_id = w.id
                ),
                EXISTS (
                    SELECT 1 FROM runtime_cleanup_jobs cleanup
                     WHERE cleanup.status = 'pending'
                       AND cleanup.adapter = wrb.adapter
                       AND cleanup.runtime_session = wrb.runtime_session
                       AND cleanup.pane_id = wrb.pane_id
                ),
                EXISTS (
                    SELECT 1 FROM provisioning_runtime_claims claim
                     WHERE claim.adapter = wrb.adapter
                       AND claim.runtime_session = wrb.runtime_session
                       AND claim.pane_id = wrb.pane_id
                )
           FROM ranked_allocations latest
           JOIN assignments a ON a.allocation_id = latest.id
           JOIN assignment_attempts aa ON aa.assignment_id = a.id
           JOIN completion_receipts cr ON cr.assignment_id = a.id AND cr.attempt_id = aa.id
           JOIN workers w ON w.id = a.worker_id
           JOIN projects p ON p.id = a.project_id
           JOIN project_workspace_bindings pwb ON pwb.project_id = p.id
           JOIN worker_profile_revisions pr ON pr.profile_id = a.profile_id AND pr.version = a.profile_version
           LEFT JOIN worker_runtime_bindings wrb ON wrb.worker_id = w.id
          WHERE latest.allocation_rank = 1 AND latest.mode = 'create_new'
            AND a.lifecycle = 'completed' AND aa.lifecycle = 'completed'
            AND w.ended_at_unix_ms IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM worker_cleanup_run_items pending_cleanup
                 WHERE pending_cleanup.worker_id = w.id
                   AND pending_cleanup.status = 'pending'
            )
          ORDER BY cr.created_at_unix_ms, a.id
          LIMIT ?1 OFFSET ?2",
    )?;
    statement
        .query_map(
            params![i64::try_from(limit)?, i64::try_from(offset)?],
            |row| candidate_from_row(row, grace_period_ms, now),
        )?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn candidate_from_row(
    row: &Row<'_>,
    grace_period_ms: u64,
    now: u64,
) -> rusqlite::Result<CleanupCandidate> {
    let ownership = row.get::<_, String>(10)?;
    let parent_worker_id = row.get::<_, Option<String>>(11)?;
    let linked_artifact_count = usize::try_from(row.get::<_, i64>(8)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let completed_at = row_u64(row, 7)?;
    let role = row.get::<_, String>(5)?;
    let is_cleanup_advisor = ownership == "system_ephemeral";
    let mut reasons = Vec::new();
    if !matches!(ownership.as_str(), "yard_owned" | "system_ephemeral") {
        reasons.push(CompletedRuntimeRetentionReason::NotYardOwned);
    }
    if row.get::<_, Option<String>>(12)?.as_deref() != Some(row.get::<_, String>(30)?.as_str())
        || row.get::<_, Option<String>>(13)?.as_deref() != Some(row.get::<_, String>(31)?.as_str())
        || row.get::<_, Option<String>>(14)?.as_deref() != Some(row.get::<_, String>(32)?.as_str())
    {
        reasons.push(CompletedRuntimeRetentionReason::ProjectOwnershipMismatch);
    }
    if (is_cleanup_advisor && parent_worker_id.is_none()) || row.get::<_, bool>(41)? {
        reasons.push(CompletedRuntimeRetentionReason::ParentOwnershipMismatch);
    }
    if now.saturating_sub(completed_at) < grace_period_ms {
        reasons.push(CompletedRuntimeRetentionReason::GraceNotElapsed);
    }
    if linked_artifact_count == 0 {
        reasons.push(CompletedRuntimeRetentionReason::NoLinkedArtifacts);
    }
    if row.get::<_, bool>(33)? {
        reasons.push(CompletedRuntimeRetentionReason::UnresolvedCompletionBlockers);
    }
    if row.get::<_, bool>(34)? {
        reasons.push(CompletedRuntimeRetentionReason::NewActiveAssignment);
    }
    if row.get::<_, bool>(35)? {
        reasons.push(CompletedRuntimeRetentionReason::PendingAssignmentIntervention);
    }
    if row.get::<_, bool>(36)? {
        reasons.push(CompletedRuntimeRetentionReason::HandoffInProgress);
    }
    if row.get::<_, bool>(37)? {
        reasons.push(CompletedRuntimeRetentionReason::ProtectedOrchestrator);
    }
    if row.get::<_, bool>(38)? {
        reasons.push(CompletedRuntimeRetentionReason::CoordinationNode);
    }
    if row.get::<_, bool>(39)? {
        reasons.push(CompletedRuntimeRetentionReason::Pinned);
    }
    let runtime_missing_or_failed = row.get::<_, Option<String>>(12)?.is_none()
        || row.get::<_, Option<String>>(23)?.as_deref() != Some("observed")
        || row.get::<_, Option<String>>(24)?.as_deref() != Some("running");
    if runtime_missing_or_failed || row.get::<_, bool>(40)? {
        reasons.push(CompletedRuntimeRetentionReason::RuntimeConflict);
    }
    if is_cleanup_advisor && !row.get::<_, bool>(42)? {
        reasons.push(CompletedRuntimeRetentionReason::CleanupAdvisorArtifactMissing);
    }
    if role == "cleanup_advisor" && !is_cleanup_advisor {
        reasons.push(CompletedRuntimeRetentionReason::CleanupAdvisorRecursionPrevented);
    }
    if row.get::<_, bool>(43)? || row.get::<_, bool>(44)? {
        reasons.push(CompletedRuntimeRetentionReason::RuntimeConflict);
    }
    let runtime = WorkerRuntimeBinding {
        adapter: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
        session: row.get::<_, Option<String>>(13)?.unwrap_or_default(),
        workspace_id: row.get::<_, Option<String>>(14)?.unwrap_or_default(),
        terminal_id: row.get::<_, Option<String>>(15)?.unwrap_or_default(),
        tab_id: row.get(16)?,
        pane_id: row.get::<_, Option<String>>(17)?.unwrap_or_default(),
        provider_session: match row.get::<_, Option<String>>(18)? {
            Some(source) => Some(yard_domain::ProviderSessionRef {
                source,
                provider: row.get::<_, Option<String>>(19)?.unwrap_or_default(),
                kind: row.get::<_, Option<String>>(20)?.unwrap_or_default(),
                value: row.get::<_, Option<String>>(21)?.unwrap_or_default(),
            }),
            None => None,
        },
        owns_tab: row.get::<_, Option<bool>>(22)?.unwrap_or(false),
        observation_state: parse_observation_state(
            row.get::<_, Option<String>>(23)?
                .as_deref()
                .unwrap_or("ambiguous"),
        )?,
        process_state: parse_process_state(
            row.get::<_, Option<String>>(24)?
                .as_deref()
                .unwrap_or("unknown"),
        )?,
        status: parse_observed_status(
            row.get::<_, Option<String>>(25)?
                .as_deref()
                .unwrap_or("unknown"),
        )?,
        state_change_sequence: row
            .get::<_, Option<i64>>(26)?
            .unwrap_or(0)
            .try_into()
            .unwrap_or(0),
        revision: row
            .get::<_, Option<i64>>(27)?
            .unwrap_or(0)
            .try_into()
            .unwrap_or(0),
        version: row
            .get::<_, Option<i64>>(28)?
            .unwrap_or(1)
            .try_into()
            .unwrap_or(1),
        last_observed_at_unix_ms: row
            .get::<_, Option<i64>>(29)?
            .unwrap_or(0)
            .try_into()
            .unwrap_or(0),
    };
    Ok(CleanupCandidate {
        public: CompletedRuntimeCleanupCandidate {
            worker_id: row.get(0)?,
            profile_name: row.get(1)?,
            project_id: row.get(2)?,
            project_name: row.get(3)?,
            assignment_id: row.get(4)?,
            role,
            completion_receipt_id: row.get(6)?,
            completed_at_unix_ms: completed_at,
            linked_artifact_count,
            close_eligible: reasons.is_empty(),
            retained_reasons: reasons,
        },
        runtime,
        worker_version: row_u64(row, 9)?,
        is_cleanup_advisor,
    })
}

fn insert_item(
    connection: &Connection,
    run_id: &str,
    candidate: CleanupCandidate,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let provider = candidate.runtime.provider_session.as_ref();
    connection.execute(
        "INSERT INTO worker_cleanup_run_items (
            run_id, worker_id, project_id, assignment_id, completion_receipt_id,
            expected_worker_version, expected_runtime_version, adapter,
            runtime_session, runtime_workspace_id, terminal_id, tab_id, pane_id,
            provider_session_json, status, reason, attempts, next_attempt_at_unix_ms,
            claim_token, claim_expires_at_unix_ms, advisor_worker_id,
            advisor_assignment_id, advisor_recommendation, advisor_artifact_id,
            advisor_rationale, is_cleanup_advisor, updated_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, 'pending', 'eligible', 0, ?15, NULL, NULL, NULL, NULL, NULL,
            NULL, NULL, ?16, ?15
         )",
        params![
            run_id,
            candidate.public.worker_id,
            candidate.public.project_id,
            candidate.public.assignment_id,
            candidate.public.completion_receipt_id,
            to_i64(candidate.worker_version)?,
            to_i64(candidate.runtime.version)?,
            candidate.runtime.adapter,
            candidate.runtime.session,
            candidate.runtime.workspace_id,
            candidate.runtime.terminal_id,
            candidate.runtime.tab_id,
            candidate.runtime.pane_id,
            provider.map(serde_json::to_string).transpose()?,
            to_i64(now)?,
            candidate.is_cleanup_advisor,
        ],
    )?;
    Ok(())
}

fn select_claimed_item(
    connection: &Connection,
    run_id: &str,
    worker_id: &str,
) -> Result<ClaimedWorkerCleanupItem, ProjectStoreError> {
    connection
        .query_row(
            "SELECT item.run_id, item.worker_id, item.project_id, item.assignment_id,
                    item.completion_receipt_id, item.expected_worker_version,
                    item.expected_runtime_version, item.adapter, item.runtime_session,
                    item.runtime_workspace_id, item.terminal_id, item.tab_id,
                    item.pane_id, item.provider_session_json, run.preview,
                    run.advisor_profile_id, item.advisor_assignment_id,
                    item.is_cleanup_advisor, item.claim_token, item.attempts
               FROM worker_cleanup_run_items item
               JOIN worker_cleanup_runs run ON run.id = item.run_id
              WHERE item.run_id = ?1 AND item.worker_id = ?2",
            params![run_id, worker_id],
            |row| {
                let provider_json = row.get::<_, Option<String>>(13)?;
                Ok(ClaimedWorkerCleanupItem {
                    run_id: row.get(0)?,
                    worker_id: row.get(1)?,
                    project_id: row.get(2)?,
                    assignment_id: row.get(3)?,
                    completion_receipt_id: row.get(4)?,
                    expected_worker_version: row_u64(row, 5)?,
                    expected_runtime_version: row_u64(row, 6)?,
                    runtime: WorkerRuntimeBinding {
                        adapter: row.get(7)?,
                        session: row.get(8)?,
                        workspace_id: row.get(9)?,
                        terminal_id: row.get(10)?,
                        tab_id: row.get(11)?,
                        pane_id: row.get(12)?,
                        provider_session: provider_json
                            .map(|json| serde_json::from_str(&json))
                            .transpose()
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    13,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                        owns_tab: true,
                        observation_state: yard_domain::RuntimeObservationState::Observed,
                        process_state: yard_domain::RuntimeProcessState::Running,
                        status: yard_domain::ObservedStatus::Done,
                        state_change_sequence: 0,
                        revision: 0,
                        version: row_u64(row, 6)?,
                        last_observed_at_unix_ms: 0,
                    },
                    preview: row.get(14)?,
                    advisor_profile_id: row.get(15)?,
                    advisor_assignment_id: row.get(16)?,
                    is_cleanup_advisor: row.get(17)?,
                    claim_token: row.get(18)?,
                    attempts: u32::try_from(row.get::<_, i64>(19)?).unwrap_or(u32::MAX),
                })
            },
        )
        .map_err(Into::into)
}

fn archive_binding(
    connection: &Connection,
    run_id: &str,
    worker_id: &str,
    claim_token: &str,
    require_retire_evidence: bool,
    now: u64,
) -> Result<(), ProjectStoreError> {
    if !cleanup_item_authorized(
        connection,
        run_id,
        worker_id,
        claim_token,
        require_retire_evidence,
        now,
    )? {
        return Err(ProjectStoreError::WorkerCleanupRevalidationFailed);
    }
    connection.execute(
        "DELETE FROM worker_runtime_bindings WHERE worker_id = ?1",
        [worker_id],
    )?;
    connection.execute(
        "UPDATE workers SET ended_at_unix_ms = ?1, version = version + 1,
                            updated_at_unix_ms = ?1
          WHERE id = ?2 AND ended_at_unix_ms IS NULL",
        params![to_i64(now)?, worker_id],
    )?;
    connection.execute(
        "UPDATE worker_allocations SET ended_at_unix_ms = COALESCE(ended_at_unix_ms, ?1)
          WHERE worker_id = ?2",
        params![to_i64(now)?, worker_id],
    )?;
    Ok(())
}

fn cleanup_item_authorized(
    connection: &Connection,
    run_id: &str,
    worker_id: &str,
    claim_token: &str,
    require_retire_evidence: bool,
    now: u64,
) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
        "SELECT EXISTS (
            SELECT 1 FROM worker_cleanup_run_items item
            JOIN worker_cleanup_runs run ON run.id = item.run_id
            JOIN workers worker ON worker.id = item.worker_id
            JOIN worker_runtime_bindings runtime ON runtime.worker_id = worker.id
            JOIN assignments assignment ON assignment.id = item.assignment_id
            JOIN worker_allocations allocation
              ON allocation.id = assignment.allocation_id
             AND allocation.worker_id = worker.id
            JOIN completion_receipts receipt
              ON receipt.id = item.completion_receipt_id
             AND receipt.assignment_id = assignment.id
            JOIN project_workspace_bindings project_runtime
              ON project_runtime.project_id = item.project_id
            WHERE item.run_id = ?1 AND item.worker_id = ?2 AND item.claim_token = ?3
              AND item.status = 'pending'
              AND worker.version = item.expected_worker_version
              AND runtime.version = item.expected_runtime_version
              AND worker.ended_at_unix_ms IS NULL
              AND worker.ownership_kind IN ('yard_owned', 'system_ephemeral')
              AND assignment.project_id = item.project_id
              AND assignment.worker_id = worker.id
              AND assignment.lifecycle = 'completed'
              AND allocation.project_id = item.project_id
              AND allocation.mode = 'create_new'
              AND NOT EXISTS (
                  SELECT 1 FROM worker_allocations later
                   WHERE later.worker_id = worker.id
                     AND (
                         later.started_at_unix_ms > allocation.started_at_unix_ms
                         OR (
                             later.started_at_unix_ms = allocation.started_at_unix_ms
                             AND later.id > allocation.id
                         )
                     )
              )
              AND runtime.adapter = item.adapter
              AND runtime.runtime_session = item.runtime_session
              AND runtime.runtime_workspace_id = item.runtime_workspace_id
              AND runtime.terminal_id = item.terminal_id
              AND runtime.pane_id = item.pane_id
              AND project_runtime.adapter = runtime.adapter
              AND project_runtime.runtime_session = runtime.runtime_session
              AND project_runtime.runtime_workspace_id = runtime.runtime_workspace_id
              AND receipt.created_at_unix_ms + run.grace_period_ms <= ?4
              AND EXISTS (
                  SELECT 1 FROM completion_receipt_artifact_links link
                   WHERE link.receipt_id = receipt.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM completion_receipt_blockers blocker
                   WHERE blocker.receipt_id = receipt.id
              )
              AND NOT EXISTS (SELECT 1 FROM assignments active
                              WHERE active.worker_id = worker.id
                                AND active.lifecycle IN ('allocating', 'active', 'handing_off'))
              AND NOT EXISTS (
                  SELECT 1 FROM assignment_prompt_commands prompt
                  JOIN command_acknowledgements command ON command.id = prompt.command_id
                   WHERE prompt.assignment_id = assignment.id
                     AND command.status IN ('pending', 'ambiguous')
              )
              AND NOT EXISTS (SELECT 1 FROM worker_handoff_commands handoff
                              WHERE handoff.worker_id = worker.id AND handoff.finished_at_unix_ms IS NULL)
              AND NOT EXISTS (SELECT 1 FROM projects WHERE orchestrator_worker_id = worker.id)
              AND NOT EXISTS (SELECT 1 FROM yard_orchestrator WHERE worker_id = worker.id)
              AND NOT EXISTS (SELECT 1 FROM coordination_nodes WHERE worker_id = worker.id)
              AND NOT EXISTS (SELECT 1 FROM worker_cleanup_pins WHERE worker_id = worker.id)
              AND NOT EXISTS (
                  SELECT 1 FROM worker_runtime_bindings conflict
                   WHERE conflict.worker_id <> worker.id
                     AND conflict.adapter = runtime.adapter
                     AND conflict.runtime_session = runtime.runtime_session
                     AND conflict.pane_id = runtime.pane_id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM runtime_cleanup_jobs cleanup
                   WHERE cleanup.status = 'pending'
                     AND cleanup.adapter = runtime.adapter
                     AND cleanup.runtime_session = runtime.runtime_session
                     AND cleanup.pane_id = runtime.pane_id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM provisioning_runtime_claims claim
                   WHERE claim.adapter = runtime.adapter
                     AND claim.runtime_session = runtime.runtime_session
                     AND claim.pane_id = runtime.pane_id
              )
              AND (
                  (
                      worker.ownership_kind = 'yard_owned'
                      AND worker.parent_worker_id IS NULL
                      AND (
                          ?5 = 0
                          OR (
                              item.advisor_recommendation = 'retire'
                              AND item.advisor_completion_receipt_id IS NOT NULL
                              AND item.advisor_artifact_id IS NOT NULL
                          )
                      )
                  )
                  OR (
                      worker.ownership_kind = 'system_ephemeral'
                      AND worker.parent_worker_id IS NOT NULL
                      AND assignment.role = 'cleanup_advisor'
                      AND EXISTS (
                          SELECT 1 FROM workers parent
                          JOIN worker_allocations parent_allocation
                            ON parent_allocation.worker_id = parent.id
                           WHERE parent.id = worker.parent_worker_id
                             AND parent.ownership_kind IN ('yard_owned', 'system_ephemeral')
                             AND parent_allocation.project_id = item.project_id
                      )
                      AND EXISTS (
                          SELECT 1 FROM cleanup_advisor_artifacts advisor
                          JOIN artifacts artifact ON artifact.id = advisor.artifact_id
                          JOIN completion_receipt_artifact_links link
                            ON link.artifact_id = artifact.id
                           WHERE artifact.worker_id = worker.id
                             AND link.receipt_id = receipt.id
                      )
                  )
              )
         )",
        params![
            run_id,
            worker_id,
            claim_token,
            to_i64(now)?,
            require_retire_evidence
        ],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Into::into)
}

fn update_run_summary(
    connection: &Connection,
    run_id: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let pending = connection.query_row(
        "SELECT COUNT(*) FROM worker_cleanup_run_items WHERE run_id = ?1 AND status = 'pending'",
        [run_id],
        |row| row.get::<_, i64>(0),
    )?;
    if pending == 0 {
        connection.execute(
            "UPDATE worker_cleanup_runs
                SET status = CASE WHEN cancellation_requested = 1 THEN 'cancelled' ELSE 'completed' END,
                    updated_at_unix_ms = ?1, completed_at_unix_ms = ?1
              WHERE id = ?2",
            params![to_i64(now)?, run_id],
        )?;
    } else {
        connection.execute(
            "UPDATE worker_cleanup_runs SET updated_at_unix_ms = ?1 WHERE id = ?2",
            params![to_i64(now)?, run_id],
        )?;
    }
    Ok(())
}

fn select_runs(
    connection: &Connection,
    limit: usize,
) -> Result<WorkerCleanupRuns, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT id FROM worker_cleanup_runs ORDER BY created_at_unix_ms DESC, id DESC LIMIT ?1",
    )?;
    let ids = statement
        .query_map([i64::try_from(limit)?], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WorkerCleanupRuns {
        runs: ids
            .into_iter()
            .map(|id| select_run(connection, &id))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn select_run_by_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<WorkerCleanupRun>, ProjectStoreError> {
    let id = connection
        .query_row(
            "SELECT id FROM worker_cleanup_runs WHERE command_id = ?1",
            [command_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    id.map(|id| select_run(connection, &id)).transpose()
}

fn select_run(
    connection: &Connection,
    run_id: &str,
) -> Result<WorkerCleanupRun, ProjectStoreError> {
    let mut run = connection
        .query_row(
            "SELECT id, command_id, trigger, preview, status, advisor_state,
                    requested_by, cancellation_requested, last_error,
                    created_at_unix_ms, updated_at_unix_ms, completed_at_unix_ms
               FROM worker_cleanup_runs WHERE id = ?1",
            [run_id],
            |row| {
                Ok(WorkerCleanupRun {
                    id: row.get(0)?,
                    command_id: row.get(1)?,
                    trigger: parse_trigger(&row.get::<_, String>(2)?)?,
                    preview: row.get(3)?,
                    status: parse_run_status(&row.get::<_, String>(4)?)?,
                    advisor_state: parse_advisor_state(&row.get::<_, String>(5)?)?,
                    requested_by: row.get(6)?,
                    candidate_count: 0,
                    keep_count: 0,
                    review_count: 0,
                    reconciled_count: 0,
                    retired_count: 0,
                    failed_count: 0,
                    cancellation_requested: row.get(7)?,
                    last_error: row.get(8)?,
                    created_at_unix_ms: row_u64(row, 9)?,
                    updated_at_unix_ms: row_u64(row, 10)?,
                    completed_at_unix_ms: row_optional_u64(row, 11)?,
                    items: Vec::new(),
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::WorkerCleanupRunNotFound)?;
    let mut statement = connection.prepare(
        "SELECT worker_id, project_id, assignment_id, completion_receipt_id,
                status, reason, attempts, advisor_recommendation,
                advisor_completion_receipt_id, advisor_artifact_id
           FROM worker_cleanup_run_items WHERE run_id = ?1 ORDER BY worker_id",
    )?;
    run.items = statement
        .query_map([run_id], |row| {
            Ok(WorkerCleanupRunItem {
                worker_id: row.get(0)?,
                project_id: row.get(1)?,
                assignment_id: row.get(2)?,
                completion_receipt_id: row.get(3)?,
                status: parse_item_status(&row.get::<_, String>(4)?)?,
                reason: row.get(5)?,
                attempts: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(u32::MAX),
                advisor_recommendation: row
                    .get::<_, Option<String>>(7)?
                    .map(|value| parse_recommendation(&value))
                    .transpose()?,
                advisor_completion_receipt_id: row.get(8)?,
                advisor_artifact_id: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    run.candidate_count = run.items.len();
    for item in &run.items {
        match item.status {
            WorkerCleanupItemStatus::Keep => run.keep_count += 1,
            WorkerCleanupItemStatus::Review => run.review_count += 1,
            WorkerCleanupItemStatus::Reconciled => run.reconciled_count += 1,
            WorkerCleanupItemStatus::Retired => run.retired_count += 1,
            WorkerCleanupItemStatus::Failed => run.failed_count += 1,
            WorkerCleanupItemStatus::Pending | WorkerCleanupItemStatus::Cancelled => {}
        }
    }
    Ok(run)
}

fn trigger_value(value: WorkerCleanupRunTrigger) -> &'static str {
    match value {
        WorkerCleanupRunTrigger::Preview => "preview",
        WorkerCleanupRunTrigger::Manual => "manual",
        WorkerCleanupRunTrigger::Scheduled => "scheduled",
    }
}

fn item_status_value(value: WorkerCleanupItemStatus) -> &'static str {
    match value {
        WorkerCleanupItemStatus::Pending => "pending",
        WorkerCleanupItemStatus::Keep => "keep",
        WorkerCleanupItemStatus::Review => "review",
        WorkerCleanupItemStatus::Reconciled => "reconciled",
        WorkerCleanupItemStatus::Retired => "retired",
        WorkerCleanupItemStatus::Failed => "failed",
        WorkerCleanupItemStatus::Cancelled => "cancelled",
    }
}

fn recommendation_value(value: CleanupAdvisorRecommendation) -> &'static str {
    match value {
        CleanupAdvisorRecommendation::Keep => "keep",
        CleanupAdvisorRecommendation::Retire => "retire",
        CleanupAdvisorRecommendation::Review => "review",
    }
}

fn parse_trigger(value: &str) -> rusqlite::Result<WorkerCleanupRunTrigger> {
    match value {
        "preview" => Ok(WorkerCleanupRunTrigger::Preview),
        "manual" => Ok(WorkerCleanupRunTrigger::Manual),
        "scheduled" => Ok(WorkerCleanupRunTrigger::Scheduled),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_run_status(value: &str) -> rusqlite::Result<WorkerCleanupRunStatus> {
    match value {
        "pending" => Ok(WorkerCleanupRunStatus::Pending),
        "running" => Ok(WorkerCleanupRunStatus::Running),
        "completed" => Ok(WorkerCleanupRunStatus::Completed),
        "cancelled" => Ok(WorkerCleanupRunStatus::Cancelled),
        "failed" => Ok(WorkerCleanupRunStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_advisor_state(value: &str) -> rusqlite::Result<CleanupAdvisorState> {
    match value {
        "not_requested" => Ok(CleanupAdvisorState::NotRequested),
        "unsupported" => Ok(CleanupAdvisorState::Unsupported),
        "pending" => Ok(CleanupAdvisorState::Pending),
        "completed" => Ok(CleanupAdvisorState::Completed),
        "failed" => Ok(CleanupAdvisorState::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_item_status(value: &str) -> rusqlite::Result<WorkerCleanupItemStatus> {
    match value {
        "pending" => Ok(WorkerCleanupItemStatus::Pending),
        "keep" => Ok(WorkerCleanupItemStatus::Keep),
        "review" => Ok(WorkerCleanupItemStatus::Review),
        "reconciled" => Ok(WorkerCleanupItemStatus::Reconciled),
        "retired" => Ok(WorkerCleanupItemStatus::Retired),
        "failed" => Ok(WorkerCleanupItemStatus::Failed),
        "cancelled" => Ok(WorkerCleanupItemStatus::Cancelled),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_recommendation(value: &str) -> rusqlite::Result<CleanupAdvisorRecommendation> {
    match value {
        "keep" => Ok(CleanupAdvisorRecommendation::Keep),
        "retire" => Ok(CleanupAdvisorRecommendation::Retire),
        "review" => Ok(CleanupAdvisorRecommendation::Review),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_observation_state(value: &str) -> rusqlite::Result<yard_domain::RuntimeObservationState> {
    match value {
        "observed" => Ok(yard_domain::RuntimeObservationState::Observed),
        "missing" => Ok(yard_domain::RuntimeObservationState::Missing),
        "ambiguous" => Ok(yard_domain::RuntimeObservationState::Ambiguous),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_process_state(value: &str) -> rusqlite::Result<yard_domain::RuntimeProcessState> {
    match value {
        "running" => Ok(yard_domain::RuntimeProcessState::Running),
        "exited" => Ok(yard_domain::RuntimeProcessState::Exited),
        "unknown" => Ok(yard_domain::RuntimeProcessState::Unknown),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_observed_status(value: &str) -> rusqlite::Result<yard_domain::ObservedStatus> {
    match value {
        "idle" => Ok(yard_domain::ObservedStatus::Idle),
        "working" => Ok(yard_domain::ObservedStatus::Working),
        "blocked" => Ok(yard_domain::ObservedStatus::Blocked),
        "done" => Ok(yard_domain::ObservedStatus::Done),
        "unknown" => Ok(yard_domain::ObservedStatus::Unknown),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

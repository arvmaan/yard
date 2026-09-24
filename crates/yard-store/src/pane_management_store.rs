use std::fmt;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;
use yard_domain::{PaneManagementBatchResult, WorkerRuntimeBinding};

use super::{
    ProjectStoreError, SqliteProjectStore, insert_worker_runtime_binding, to_i64, to_u64,
    unix_time_ms,
};

#[derive(Clone, PartialEq, Eq)]
pub struct StoredLeaseToken(String);

impl StoredLeaseToken {
    #[must_use]
    pub fn from_secret(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for StoredLeaseToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoredLeaseToken([REDACTED])")
    }
}

#[derive(Clone)]
pub struct StoredPaneManagementLease {
    pub worker_id: String,
    pub project_id: String,
    pub session: String,
    pub pane_id: String,
    pub pane_instance_id: String,
    pub owner_id: String,
    pub token: StoredLeaseToken,
    pub expires_at_unix_ms: u64,
    pub recovery_required: bool,
}

impl fmt::Debug for StoredPaneManagementLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredPaneManagementLease")
            .field("worker_id", &self.worker_id)
            .field("project_id", &self.project_id)
            .field("session", &self.session)
            .field("pane_id", &self.pane_id)
            .field("pane_instance_id", &self.pane_instance_id)
            .field("owner_id", &self.owner_id)
            .field("token", &self.token)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .field("recovery_required", &self.recovery_required)
            .finish()
    }
}

pub struct ManagedPaneAdoption {
    pub command_id: String,
    pub project_id: String,
    pub runtime: WorkerRuntimeBinding,
    pub installation_uuid: String,
    pub owner_id: String,
    pub token: StoredLeaseToken,
    pub pane_instance_id: String,
    pub expires_at_unix_ms: u64,
    pub acquisition_request_id: String,
}

pub enum BeginPaneManagementBatch {
    Started,
    Replayed(PaneManagementBatchResult),
}

pub(super) async fn installation_uuid(
    store: &SqliteProjectStore,
) -> Result<String, ProjectStoreError> {
    store
        .run(|connection| {
            if let Some(value) = connection
                .query_row(
                    "SELECT installation_uuid FROM yard_installation WHERE singleton_id = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(value);
            }
            let value = Uuid::now_v7().to_string();
            connection.execute(
                "INSERT INTO yard_installation (singleton_id, installation_uuid, created_at_unix_ms)
                 VALUES (1, ?1, ?2)",
                params![value, to_i64(unix_time_ms()?)?],
            )?;
            Ok(value)
        })
        .await
}

pub(super) async fn list(
    store: &SqliteProjectStore,
) -> Result<Vec<StoredPaneManagementLease>, ProjectStoreError> {
    store
        .run(|connection| {
            let mut statement = connection.prepare(
                "SELECT worker_id, project_id, herdr_session, pane_id,
                        pane_instance_id, owner_id, lease_token,
                        expires_at_unix_ms, status
                   FROM pane_management_leases
                  ORDER BY created_at_unix_ms, worker_id",
            )?;
            statement
                .query_map([], lease_from_row)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(Into::into)
        })
        .await
}

pub(super) async fn begin_batch(
    store: &SqliteProjectStore,
    command_id: &str,
    actor: &str,
    input_hash: &str,
) -> Result<BeginPaneManagementBatch, ProjectStoreError> {
    let command_id = command_id.to_owned();
    let actor = actor.to_owned();
    let input_hash = input_hash.to_owned();
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT actor, input_hash, status, result_json
                       FROM pane_management_batches WHERE command_id = ?1",
                    [&command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((saved_actor, saved_hash, status, result_json)) = existing {
                if saved_actor != actor || saved_hash != input_hash {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                if status == "succeeded" {
                    let result = serde_json::from_str(
                        result_json
                            .as_deref()
                            .ok_or(ProjectStoreError::CommandInProgress)?,
                    )?;
                    return Ok(BeginPaneManagementBatch::Replayed(result));
                }
                return Err(ProjectStoreError::CommandInProgress);
            }
            let now = to_i64(unix_time_ms()?)?;
            transaction.execute(
                "INSERT INTO pane_management_batches (
                    command_id, actor, input_hash, status, result_json,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, 'pending', NULL, ?4, ?4)",
                params![command_id, actor, input_hash, now],
            )?;
            transaction.commit()?;
            Ok(BeginPaneManagementBatch::Started)
        })
        .await
}

pub(super) async fn adopt(
    store: &SqliteProjectStore,
    adoption: ManagedPaneAdoption,
) -> Result<String, ProjectStoreError> {
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let pending = transaction.query_row(
                "SELECT status FROM pane_management_batches WHERE command_id = ?1",
                [&adoption.command_id],
                |row| row.get::<_, String>(0),
            )?;
            if pending != "pending" {
                return Err(ProjectStoreError::CommandInProgress);
            }
            let associated = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM project_workspace_bindings
                     WHERE project_id = ?1 AND adapter = ?2
                       AND runtime_session = ?3 AND runtime_workspace_id = ?4
                 )",
                params![
                    adoption.project_id,
                    adoption.runtime.adapter,
                    adoption.runtime.session,
                    adoption.runtime.workspace_id,
                ],
                |row| row.get::<_, bool>(0),
            )?;
            if !associated {
                return Err(ProjectStoreError::RuntimeWorkspaceMismatch);
            }
            let worker_id = Uuid::now_v7().to_string();
            let allocation_id = Uuid::now_v7().to_string();
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO workers (
                    id, profile_id, profile_version, desired_state, version,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, NULL, NULL, 'running', 1, ?2, ?2)",
                params![worker_id, to_i64(now)?],
            )?;
            insert_worker_runtime_binding(&transaction, &worker_id, &adoption.runtime, now)?;
            transaction.execute(
                "INSERT INTO worker_allocations (
                    id, project_id, worker_id, mode, started_by_command_id,
                    started_at_unix_ms, ended_at_unix_ms
                 ) VALUES (?1, ?2, ?3, 'adopt_existing', ?4, ?5, NULL)",
                params![
                    allocation_id,
                    adoption.project_id,
                    worker_id,
                    adoption.command_id,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO pane_management_leases (
                    worker_id, project_id, allocation_id, installation_uuid,
                    herdr_session, pane_id, pane_instance_id, owner_id,
                    lease_token, expires_at_unix_ms, acquisition_request_id,
                    status, renewal_failures, renew_after_unix_ms,
                    last_error_code, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                    'active', 0, ?12, NULL, ?13, ?13
                 )",
                params![
                    worker_id,
                    adoption.project_id,
                    allocation_id,
                    adoption.installation_uuid,
                    adoption.runtime.session,
                    adoption.runtime.pane_id,
                    adoption.pane_instance_id,
                    adoption.owner_id,
                    adoption.token.expose_secret(),
                    to_i64(adoption.expires_at_unix_ms)?,
                    adoption.acquisition_request_id,
                    to_i64(adoption.expires_at_unix_ms.saturating_sub(20_000))?,
                    to_i64(now)?,
                ],
            )?;
            transaction.commit()?;
            Ok(worker_id)
        })
        .await
}

pub(super) async fn complete_batch(
    store: &SqliteProjectStore,
    result: PaneManagementBatchResult,
) -> Result<(), ProjectStoreError> {
    let command_id = result.command_id.clone();
    let result_json = serde_json::to_string(&result)?;
    store
        .run(move |connection| {
            let updated = connection.execute(
                "UPDATE pane_management_batches
                    SET status = 'succeeded', result_json = ?1,
                        updated_at_unix_ms = ?2
                  WHERE command_id = ?3 AND status = 'pending'",
                params![result_json, to_i64(unix_time_ms()?)?, command_id],
            )?;
            if updated != 1 {
                return Err(ProjectStoreError::CommandInProgress);
            }
            Ok(())
        })
        .await
}

pub(super) async fn list_due(
    store: &SqliteProjectStore,
    now_unix_ms: u64,
    limit: usize,
) -> Result<Vec<StoredPaneManagementLease>, ProjectStoreError> {
    store
        .run(move |connection| {
            let mut statement = connection.prepare(
                "SELECT worker_id, project_id, herdr_session, pane_id,
                        pane_instance_id, owner_id, lease_token,
                        expires_at_unix_ms, status
                   FROM pane_management_leases
                  WHERE status = 'active' AND renew_after_unix_ms <= ?1
                  ORDER BY renew_after_unix_ms, worker_id LIMIT ?2",
            )?;
            statement
                .query_map(
                    params![to_i64(now_unix_ms)?, i64::try_from(limit)?],
                    lease_from_row,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(Into::into)
        })
        .await
}

pub(super) async fn renewed(
    store: &SqliteProjectStore,
    worker_id: &str,
    expires_at_unix_ms: u64,
) -> Result<(), ProjectStoreError> {
    let worker_id = worker_id.to_owned();
    store
        .run(move |connection| {
            let updated = connection.execute(
                "UPDATE pane_management_leases
                    SET expires_at_unix_ms = ?1, renew_after_unix_ms = ?2,
                        renewal_failures = 0, last_error_code = NULL,
                        updated_at_unix_ms = ?3
                  WHERE worker_id = ?4 AND status = 'active'",
                params![
                    to_i64(expires_at_unix_ms)?,
                    to_i64(expires_at_unix_ms.saturating_sub(20_000))?,
                    to_i64(unix_time_ms()?)?,
                    worker_id,
                ],
            )?;
            if updated != 1 {
                return Err(ProjectStoreError::PaneManagementLeaseChanged);
            }
            Ok(())
        })
        .await
}

pub(super) async fn mark_recovery_required(
    store: &SqliteProjectStore,
    worker_id: &str,
    error_code: &str,
) -> Result<(), ProjectStoreError> {
    let worker_id = worker_id.to_owned();
    let error_code = error_code.to_owned();
    store
        .run(move |connection| {
            connection.execute(
                "UPDATE pane_management_leases
                    SET status = 'recovery_required', last_error_code = ?1,
                        renewal_failures = renewal_failures + 1,
                        updated_at_unix_ms = ?2
                  WHERE worker_id = ?3 AND status = 'active'",
                params![error_code, to_i64(unix_time_ms()?)?, worker_id],
            )?;
            Ok(())
        })
        .await
}

fn lease_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredPaneManagementLease> {
    Ok(StoredPaneManagementLease {
        worker_id: row.get(0)?,
        project_id: row.get(1)?,
        session: row.get(2)?,
        pane_id: row.get(3)?,
        pane_instance_id: row.get(4)?,
        owner_id: row.get(5)?,
        token: StoredLeaseToken::from_secret(row.get(6)?),
        expires_at_unix_ms: to_u64(row.get::<_, i64>(7)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        recovery_required: row.get::<_, String>(8)? == "recovery_required",
    })
}

#[cfg(test)]
mod tests {
    use super::StoredLeaseToken;

    #[test]
    fn stored_tokens_are_redacted_from_debug() {
        let token = StoredLeaseToken::from_secret("private-token".to_owned());
        assert_eq!(format!("{token:?}"), "StoredLeaseToken([REDACTED])");
        assert!(!format!("{token:?}").contains(token.expose_secret()));
    }
}

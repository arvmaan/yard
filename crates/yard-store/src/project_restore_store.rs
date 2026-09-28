//! Restore: the Undo for a project archive (design D7).
//!
//! Restore puts an archived project back while its archive is current, the
//! project is not deleted, and nothing else holds its Herdr workspace. The
//! server resolves the orchestrator's retired runtime against a fresh Herdr
//! inventory first ([`ProjectRestorePlan`]); one transaction then re-binds
//! the workspace, resumes the orchestrator with a new open allocation,
//! cancels its archive cleanup job, re-binds its runtime when it is still the
//! same pane (releasing the retirement), and marks the archive restored.
//! Assignments the archive cancelled stay cancelled and their workers stay
//! ended.
//!
//! The re-bind goes through mainline's identity rules, not a copy of the
//! retired row: the server builds the binding from the fresh observation
//! (`runtime_binding_from_observed_worker`) only when exactly one coherent
//! agent and pane carry the retired identity (terminal, pane, tab, workspace
//! and provider session), the pane's `pane_instance_id` agrees with the
//! agent's, and, when Yard holds a pane-management lease for the
//! orchestrator, with the lease's instance. Here the binding must still equal
//! the retired identity, and `insert_worker_runtime_binding` re-checks that no
//! other worker or pending cleanup holds it. Anything else restores unbound
//! and keeps the retirement, so a recycled pane id is never bound.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;
use yard_domain::{
    ArchivedProjectSummary, ArchivedProjects, ProjectRestoreUnavailableReason,
    ProjectRuntimeBinding, ProjectVisibility, ProviderSessionRef, RestoreProject,
    RestoredOrchestratorRuntime, RestoredProject, RuntimeObservationState, RuntimeProcessState,
    WorkerRuntimeBinding,
};

use super::{
    ProjectStoreError, command_id_exists, coordination_node_store, deleted_project_exists,
    deleted_worker_exists, ensure_project_workspace_available, insert_lifecycle_event,
    insert_worker_runtime_binding, project_archive_store, provider_session_from_columns,
    row_optional_u64, row_u64, runtime_cleanup_pending, to_i64,
};

/// The orchestrator runtime an archive retired, as Restore must find it
/// again in Herdr before it can be re-bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredOrchestratorRuntime {
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub tab_id: Option<String>,
    pub pane_id: String,
    pub provider_session: Option<ProviderSessionRef>,
    pub owns_tab: bool,
    /// The pane instance Yard's pane-management lease recorded for the
    /// orchestrator on this pane, when it holds one (Herdr with
    /// `pane_management_lease_v1`). Herdr without leases leaves it `None`.
    pub pane_instance_id: Option<String>,
}

/// What the Restore service reads before it contacts Herdr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRestorePlan {
    /// The same command already restored the project.
    Replayed(Box<RestoredProject>),
    /// Restore can run now. With a retired orchestrator runtime, the service
    /// must resolve it against a fresh inventory first.
    Ready {
        retired_orchestrator: Option<Box<RetiredOrchestratorRuntime>>,
    },
}

/// What the fresh Herdr inventory says about the retired orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRestoreRuntime {
    /// The same pane is still running: re-bind it. The binding is built from
    /// the fresh observation; its version is set here.
    Rebind(Box<WorkerRuntimeBinding>),
    /// The pane is gone, or the archive had no runtime: restore unbound.
    Absent,
    /// The identity now belongs to another pane: restore unbound and keep
    /// the retirement, so the reused pane is never adopted by mistake.
    Conflict,
}

/// The project's current (not restored) archive.
struct CurrentArchive {
    command_id: String,
    orchestrator_worker_id: String,
    expected_orchestrator_runtime_version: Option<u64>,
    runtime: ProjectRuntimeBinding,
}

fn select_current_archive(
    connection: &Connection,
    project_id: &str,
) -> Result<Option<CurrentArchive>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT command_id, expected_orchestrator_worker_id,
                    expected_orchestrator_runtime_version, runtime_adapter,
                    runtime_session, runtime_workspace_id
               FROM archived_projects
              WHERE project_id = ?1 AND restored_at_unix_ms IS NULL",
            [project_id],
            |row| {
                Ok(CurrentArchive {
                    command_id: row.get(0)?,
                    orchestrator_worker_id: row.get(1)?,
                    expected_orchestrator_runtime_version: row_optional_u64(row, 2)?,
                    runtime: ProjectRuntimeBinding {
                        adapter: row.get(3)?,
                        session: row.get(4)?,
                        workspace_id: row.get(5)?,
                    },
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

/// The project's visibility, derived from its tombstones.
pub(super) fn project_visibility(
    connection: &Connection,
    project_id: &str,
) -> Result<ProjectVisibility, ProjectStoreError> {
    if deleted_project_exists(connection, project_id)? {
        return Ok(ProjectVisibility::Deleted);
    }
    Ok(
        if select_current_archive(connection, project_id)?.is_some() {
            ProjectVisibility::Archived
        } else {
            ProjectVisibility::Active
        },
    )
}

/// Every check Restore makes before and inside its transaction, in order:
/// the project exists and is not deleted, the caller's archive is its
/// current one, the orchestrator the archive ended is still there, and
/// nothing else holds the workspace.
fn check_restore_target(
    connection: &Connection,
    project_id: &str,
    expected_archive_command_id: &str,
) -> Result<CurrentArchive, ProjectStoreError> {
    let orchestrator_worker_id = connection
        .query_row(
            "SELECT orchestrator_worker_id FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(ProjectStoreError::ProjectNotFound)?;
    if deleted_project_exists(connection, project_id)? {
        return Err(unavailable(ProjectRestoreUnavailableReason::ProjectDeleted));
    }
    let archive = select_current_archive(connection, project_id)?
        .ok_or(ProjectStoreError::ProjectRestoreNotArchived)?;
    if archive.command_id != expected_archive_command_id {
        return Err(unavailable(ProjectRestoreUnavailableReason::ArchiveChanged));
    }
    let orchestrator_ended = connection
        .query_row(
            "SELECT ended_at_unix_ms IS NOT NULL FROM workers WHERE id = ?1",
            [&archive.orchestrator_worker_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    if orchestrator_worker_id != archive.orchestrator_worker_id
        || orchestrator_ended != Some(true)
        || deleted_worker_exists(connection, &archive.orchestrator_worker_id)?
    {
        return Err(unavailable(
            ProjectRestoreUnavailableReason::OrchestratorUnavailable,
        ));
    }
    match ensure_project_workspace_available(connection, &archive.runtime, None, Some(project_id)) {
        Ok(()) => Ok(archive),
        Err(
            ProjectStoreError::RuntimeWorkspaceAlreadyBound
            | ProjectStoreError::RuntimeWorkspaceReserved,
        ) => Err(unavailable(
            ProjectRestoreUnavailableReason::WorkspaceReserved,
        )),
        Err(error) => Err(error),
    }
}

const fn unavailable(reason: ProjectRestoreUnavailableReason) -> ProjectStoreError {
    ProjectStoreError::ProjectRestoreUnavailable(reason)
}

/// Whether Restore could undo this archive now (Herdr reachability is only
/// checked when Restore runs).
pub(super) fn archive_restorable(
    connection: &Connection,
    project_id: &str,
    archive_command_id: &str,
) -> Result<bool, ProjectStoreError> {
    match check_restore_target(connection, project_id, archive_command_id) {
        Ok(_) => Ok(true),
        Err(
            ProjectStoreError::ProjectRestoreUnavailable(_)
            | ProjectStoreError::ProjectRestoreNotArchived
            | ProjectStoreError::ProjectNotFound,
        ) => Ok(false),
        Err(error) => Err(error),
    }
}

struct RetiredRow {
    id: String,
    runtime: RetiredOrchestratorRuntime,
}

/// The orchestrator retirement this archive recorded and has not released.
/// `owns_tab` comes from the archive's cleanup job for it.
fn select_retired_orchestrator(
    connection: &Connection,
    archive: &CurrentArchive,
) -> Result<Option<RetiredRow>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT retired.id, retired.adapter, retired.runtime_session,
                    retired.runtime_workspace_id, retired.terminal_id,
                    retired.tab_id, retired.pane_id,
                    retired.provider_session_source,
                    retired.provider_session_provider,
                    retired.provider_session_kind,
                    retired.provider_session_value,
                    COALESCE((
                        SELECT cleanup.owns_tab FROM runtime_cleanup_jobs cleanup
                         WHERE cleanup.command_id = retired.command_id
                           AND cleanup.worker_id = retired.worker_id
                         ORDER BY cleanup.created_at_unix_ms, cleanup.id
                         LIMIT 1
                    ), 0),
                    (
                        SELECT lease.pane_instance_id
                          FROM pane_management_leases lease
                         WHERE lease.worker_id = retired.worker_id
                           AND lease.herdr_session = retired.runtime_session
                           AND lease.pane_id = retired.pane_id
                    )
               FROM retired_runtime_bindings retired
              WHERE retired.command_id = ?1 AND retired.worker_id = ?2
                AND retired.released_at_unix_ms IS NULL",
            params![archive.command_id, archive.orchestrator_worker_id],
            |row| {
                Ok(RetiredRow {
                    id: row.get(0)?,
                    runtime: RetiredOrchestratorRuntime {
                        adapter: row.get(1)?,
                        session: row.get(2)?,
                        workspace_id: row.get(3)?,
                        terminal_id: row.get(4)?,
                        tab_id: row.get(5)?,
                        pane_id: row.get(6)?,
                        provider_session: provider_session_from_columns(row, 7)?,
                        owns_tab: row.get(11)?,
                        pane_instance_id: row.get(12)?,
                    },
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

struct StoredRestore {
    command_id: String,
    project_id: String,
    actor: String,
    expected_archive_command_id: String,
    orchestrator_worker_id: String,
    orchestrator_runtime: RestoredOrchestratorRuntime,
    restored_at_unix_ms: u64,
}

impl StoredRestore {
    fn matches(&self, project_id: &str, command: &RestoreProject) -> bool {
        self.project_id == project_id
            && self.actor == command.actor
            && self.expected_archive_command_id == command.expected_archive_command_id
    }

    fn replayed(self, connection: &Connection) -> Result<RestoredProject, ProjectStoreError> {
        let cancelled_assignment_ids = project_archive_store::cancelled_assignment_ids(
            connection,
            &self.expected_archive_command_id,
        )?;
        let visibility = project_visibility(connection, &self.project_id)?;
        Ok(RestoredProject {
            command_id: self.command_id,
            project_id: self.project_id,
            archive_command_id: self.expected_archive_command_id,
            orchestrator_worker_id: self.orchestrator_worker_id,
            orchestrator_runtime: self.orchestrator_runtime,
            cancelled_assignment_ids,
            restored_at_unix_ms: self.restored_at_unix_ms,
            visibility,
            replayed: true,
        })
    }
}

fn select_restore_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredRestore>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT restore.command_id, restore.project_id, command.actor,
                    restore.expected_archive_command_id,
                    archived.expected_orchestrator_worker_id,
                    restore.orchestrator_runtime, restore.restored_at_unix_ms
               FROM project_restore_commands restore
               JOIN command_acknowledgements command
                 ON command.id = restore.command_id
               JOIN archived_projects archived
                 ON archived.command_id = restore.expected_archive_command_id
              WHERE restore.command_id = ?1
                AND command.command_type = 'project_restore'",
            [command_id],
            |row| {
                let runtime: String = row.get(5)?;
                Ok(StoredRestore {
                    command_id: row.get(0)?,
                    project_id: row.get(1)?,
                    actor: row.get(2)?,
                    expected_archive_command_id: row.get(3)?,
                    orchestrator_worker_id: row.get(4)?,
                    orchestrator_runtime: if runtime == "rebound" {
                        RestoredOrchestratorRuntime::Rebound
                    } else {
                        RestoredOrchestratorRuntime::Unbound
                    },
                    restored_at_unix_ms: row_u64(row, 6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

/// Replay, or the idempotency refusals, for a restore command ID.
fn replay_or_reject(
    connection: &Connection,
    project_id: &str,
    command: &RestoreProject,
) -> Result<Option<RestoredProject>, ProjectStoreError> {
    if let Some(existing) = select_restore_command(connection, &command.command_id)? {
        if !existing.matches(project_id, command) {
            return Err(ProjectStoreError::IdempotencyConflict);
        }
        return existing.replayed(connection).map(Some);
    }
    if command_id_exists(connection, &command.command_id)? {
        return Err(ProjectStoreError::IdempotencyConflict);
    }
    Ok(None)
}

/// Everything Restore can decide before contacting Herdr.
pub(super) fn restore_plan(
    connection: &Connection,
    project_id: &str,
    command: &RestoreProject,
) -> Result<ProjectRestorePlan, ProjectStoreError> {
    if let Some(replayed) = replay_or_reject(connection, project_id, command)? {
        return Ok(ProjectRestorePlan::Replayed(Box::new(replayed)));
    }
    let archive =
        check_restore_target(connection, project_id, &command.expected_archive_command_id)?;
    Ok(ProjectRestorePlan::Ready {
        retired_orchestrator: select_retired_orchestrator(connection, &archive)?
            .map(|retired| Box::new(retired.runtime)),
    })
}

/// Restore an archived project in the caller's IMMEDIATE transaction.
#[allow(clippy::too_many_lines)]
pub(super) fn restore_project_tx(
    transaction: &Transaction<'_>,
    project_id: &str,
    command: &RestoreProject,
    runtime: ProjectRestoreRuntime,
    now: u64,
) -> Result<RestoredProject, ProjectStoreError> {
    if let Some(replayed) = replay_or_reject(transaction, project_id, command)? {
        return Ok(replayed);
    }
    let archive = check_restore_target(
        transaction,
        project_id,
        &command.expected_archive_command_id,
    )?;
    let orchestrator_id = archive.orchestrator_worker_id.clone();
    let project_version = transaction.query_row(
        "SELECT version FROM projects WHERE id = ?1",
        [project_id],
        |row| row_u64(row, 0),
    )?;
    let worker_version = transaction.query_row(
        "SELECT version FROM workers WHERE id = ?1",
        [&orchestrator_id],
        |row| row_u64(row, 0),
    )?;
    let next_project_version = project_version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    let next_worker_version = worker_version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;

    transaction.execute(
        "INSERT INTO command_acknowledgements (
            id, command_type, actor, status, error_message,
            created_at_unix_ms, updated_at_unix_ms
         ) VALUES (?1, 'project_restore', ?2, 'succeeded', NULL, ?3, ?3)",
        params![command.command_id, command.actor, to_i64(now)?],
    )?;
    // Cancel before any re-bind: a pending cleanup job reserves its runtime
    // identity. A runner still holding the old claim can no longer complete
    // it, because every runner write requires a pending job and its token.
    transaction.execute(
        "UPDATE runtime_cleanup_jobs
            SET status = 'cancelled', completed_at_unix_ms = ?1,
                updated_at_unix_ms = ?1, claim_token = NULL,
                claim_expires_at_unix_ms = NULL
          WHERE command_id = ?2 AND worker_id = ?3 AND status = 'pending'",
        params![to_i64(now)?, archive.command_id, orchestrator_id],
    )?;
    transaction.execute(
        "INSERT INTO project_workspace_bindings (
            project_id, adapter, runtime_session, runtime_workspace_id
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            project_id,
            archive.runtime.adapter,
            archive.runtime.session,
            archive.runtime.workspace_id,
        ],
    )?;
    let project_rows = transaction.execute(
        "UPDATE projects
            SET version = ?1, updated_at_unix_ms = ?2
          WHERE id = ?3 AND version = ?4",
        params![
            to_i64(next_project_version)?,
            to_i64(now)?,
            project_id,
            to_i64(project_version)?,
        ],
    )?;
    let worker_rows = transaction.execute(
        "UPDATE workers
            SET ended_at_unix_ms = NULL, version = ?1, updated_at_unix_ms = ?2
          WHERE id = ?3 AND version = ?4 AND ended_at_unix_ms IS NOT NULL",
        params![
            to_i64(next_worker_version)?,
            to_i64(now)?,
            orchestrator_id,
            to_i64(worker_version)?,
        ],
    )?;
    if project_rows != 1 || worker_rows != 1 {
        return Err(unavailable(
            ProjectRestoreUnavailableReason::OrchestratorUnavailable,
        ));
    }
    // Archive ended the orchestrator's allocation; transfer and replacement
    // both need an open one.
    let allocation_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO worker_allocations (
            id, project_id, worker_id, mode, started_by_command_id,
            started_at_unix_ms, ended_at_unix_ms
         ) VALUES (?1, ?2, ?3, 'adopt_existing', ?4, ?5, NULL)",
        params![
            allocation_id,
            project_id,
            orchestrator_id,
            command.command_id,
            to_i64(now)?,
        ],
    )?;

    let retired = select_retired_orchestrator(transaction, &archive)?;
    let orchestrator_runtime = match (runtime, retired) {
        (ProjectRestoreRuntime::Rebind(observed), Some(retired))
            if observed_identity_matches(&retired.runtime, &observed) =>
        {
            let mut binding =
                rebound_runtime(*observed, archive.expected_orchestrator_runtime_version)?;
            // Tab ownership is what the archive recorded, not what the caller says.
            binding.owns_tab = retired.runtime.owns_tab;
            match insert_worker_runtime_binding(transaction, &orchestrator_id, &binding, now) {
                Ok(()) => {}
                Err(ProjectStoreError::RuntimeWorkerAlreadyBound) => {
                    return Err(unavailable(
                        ProjectRestoreUnavailableReason::RuntimeReserved,
                    ));
                }
                Err(error) => return Err(error),
            }
            transaction.execute(
                "UPDATE retired_runtime_bindings
                    SET released_at_unix_ms = ?1, released_by_command_id = ?2
                  WHERE id = ?3 AND released_at_unix_ms IS NULL",
                params![to_i64(now)?, command.command_id, retired.id],
            )?;
            RestoredOrchestratorRuntime::Rebound
        }
        // Absent (or no runtime at archive time), or an identity another pane
        // now uses: restore unbound, as after a crash. A conflicting
        // identity keeps its retirement, so it is never adopted by mistake.
        _ => RestoredOrchestratorRuntime::Unbound,
    };

    let archive_rows = transaction.execute(
        "UPDATE archived_projects
            SET restore_command_id = ?1, restored_at_unix_ms = ?2
          WHERE command_id = ?3 AND restored_at_unix_ms IS NULL",
        params![command.command_id, to_i64(now)?, archive.command_id],
    )?;
    if archive_rows != 1 {
        return Err(unavailable(ProjectRestoreUnavailableReason::ArchiveChanged));
    }
    transaction.execute(
        "INSERT INTO project_restore_commands (
            command_id, project_id, expected_archive_command_id,
            orchestrator_runtime, allocation_id, result_project_version,
            result_orchestrator_worker_version, restored_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            command.command_id,
            project_id,
            archive.command_id,
            orchestrator_runtime.as_str(),
            allocation_id,
            to_i64(next_project_version)?,
            to_i64(next_worker_version)?,
            to_i64(now)?,
        ],
    )?;
    coordination_node_store::restore_project_attachments(
        transaction,
        project_id,
        &command.actor,
        now,
    )?;
    insert_lifecycle_event(
        transaction,
        "project",
        project_id,
        next_project_version,
        "project_restored",
        &command.actor,
        now,
    )?;
    insert_lifecycle_event(
        transaction,
        "worker",
        &orchestrator_id,
        next_worker_version,
        match orchestrator_runtime {
            RestoredOrchestratorRuntime::Rebound => "project_restore_orchestrator_rebound",
            RestoredOrchestratorRuntime::Unbound => "project_restore_orchestrator_unbound",
        },
        &command.actor,
        now,
    )?;
    Ok(RestoredProject {
        command_id: command.command_id.clone(),
        project_id: project_id.to_owned(),
        cancelled_assignment_ids: project_archive_store::cancelled_assignment_ids(
            transaction,
            &archive.command_id,
        )?,
        archive_command_id: archive.command_id,
        orchestrator_worker_id: orchestrator_id,
        orchestrator_runtime,
        restored_at_unix_ms: now,
        visibility: ProjectVisibility::Active,
        replayed: false,
    })
}

/// Whether the observed binding is exactly the retired identity.
fn observed_identity_matches(
    retired: &RetiredOrchestratorRuntime,
    observed: &WorkerRuntimeBinding,
) -> bool {
    observed.adapter == retired.adapter
        && observed.session == retired.session
        && observed.workspace_id == retired.workspace_id
        && observed.terminal_id == retired.terminal_id
        && observed.tab_id == retired.tab_id
        && observed.pane_id == retired.pane_id
        && observed.provider_session == retired.provider_session
}

/// The observed runtime bound again at the next runtime version; the next
/// reconciliation refreshes its observation.
fn rebound_runtime(
    mut observed: WorkerRuntimeBinding,
    archived_runtime_version: Option<u64>,
) -> Result<WorkerRuntimeBinding, ProjectStoreError> {
    observed.version = archived_runtime_version
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    observed.observation_state = RuntimeObservationState::Observed;
    observed.process_state = RuntimeProcessState::Running;
    Ok(observed)
}

/// Archived projects that are not deleted, newest first, for the Archived
/// view.
pub(super) fn list_archived_projects(
    connection: &Connection,
) -> Result<ArchivedProjects, ProjectStoreError> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT archived.project_id, project.name, archived.command_id,
                    archived.expected_orchestrator_worker_id,
                    archived.archived_at_unix_ms
               FROM archived_projects archived
               JOIN projects project ON project.id = archived.project_id
              WHERE archived.restored_at_unix_ms IS NULL
                AND NOT EXISTS (
                    SELECT 1 FROM deleted_projects deleted
                     WHERE deleted.project_id = archived.project_id
                )
              ORDER BY archived.archived_at_unix_ms DESC, archived.command_id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row_u64(row, 4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let projects = rows
        .into_iter()
        .map(
            |(project_id, name, archive_command_id, orchestrator_worker_id, archived_at)| {
                Ok(ArchivedProjectSummary {
                    cancelled_assignment_ids: project_archive_store::cancelled_assignment_ids(
                        connection,
                        &archive_command_id,
                    )?,
                    cleanup_pending: runtime_cleanup_pending(connection, &archive_command_id)?,
                    restorable: archive_restorable(connection, &project_id, &archive_command_id)?,
                    project_id,
                    name,
                    archive_command_id,
                    orchestrator_worker_id,
                    archived_at_unix_ms: archived_at,
                })
            },
        )
        .collect::<Result<Vec<_>, ProjectStoreError>>()?;
    Ok(ArchivedProjects { projects })
}

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use yard_domain::{
    ArchiveCoordinationNode, ArchivedCoordinationNode, AutomationState, CoordinationDeliveryStatus,
    CoordinationNode, CoordinationNodeArchivePreconditions, CoordinationNodeCommandResult,
    CoordinationNodeDispositionAutomation, CoordinationNodeDispositionBlocker,
    CoordinationNodeDispositionBlockerKind, CoordinationNodeDispositionPreview,
    CoordinationNodeDispositionProject, CoordinationNodeDispositionWorker, CoordinationNodeKind,
    CoordinationNodePlacement, CoordinationNodePromptAcknowledgement, CoordinationNodeRoute,
    CoordinationNodeRoutes, CoordinationNodes, CoordinationSnapshot, CoordinationSnapshots,
    CreateCoordinationNode, DeleteCoordinationNode, DeletedCoordinationNode,
    ProjectBackgroundStatus, ProvisionCoordinationNode, RequestCoordinationSnapshot,
    SendCoordinationNodePrompt, SendCoordinationNodeRoute, SnapshotAbandonmentReason,
    SnapshotCollectionProgress, SnapshotCollectionStatus, SnapshotProjectCollection,
    UpdateCoordinationNode, UpdateCoordinationNodePlacement, WorkerDesiredState,
    canonical_coordination_uuid,
};

use super::{
    BeginCoordinationNodePrompt, BeginCoordinationNodeRoute, ProjectStoreError,
    SnapshotDeliveryResult, SnapshotProjectFolder, SqliteProjectStore, WorkerAvailability,
    command_id_exists, insert_lifecycle_event, project_exists,
    reject_project_orchestrator_intervention, required_command_id, required_runtime_status,
    select_project, select_worker_candidate, to_i64, unix_time_ms,
};

const MAX_ROUTE_LIST_LIMIT: usize = 500;
/// A project snapshot that is still uncollected this long after its prompt
/// was submitted (or, when delivery failed or was ambiguous, after the
/// snapshot was requested) is marked `abandoned` with reason `expired`.
/// Yard keeps checking the folder, so late files still mark it collected.
pub(super) const SNAPSHOT_COLLECTION_EXPIRY_MS: u64 = 24 * 60 * 60 * 1_000;

pub(super) fn archive_project_attachments(
    transaction: &Transaction<'_>,
    project_id: &str,
    actor: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    version_project_attachments(
        transaction,
        project_id,
        actor,
        now,
        "coordination_node_project_archived",
    )
}

/// A restored project is listed again under the workstreams it stayed
/// attached to, so each of them gets a new version.
pub(super) fn restore_project_attachments(
    transaction: &Transaction<'_>,
    project_id: &str,
    actor: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    version_project_attachments(
        transaction,
        project_id,
        actor,
        now,
        "coordination_node_project_restored",
    )
}

fn version_project_attachments(
    transaction: &Transaction<'_>,
    project_id: &str,
    actor: &str,
    now: u64,
    event_type: &str,
) -> Result<(), ProjectStoreError> {
    // An archived (or deleted) node no longer changes, so it keeps the
    // version and audit trail its tombstone recorded.
    let attached_nodes = {
        let mut statement = transaction.prepare(
            "SELECT node.id, node.version
               FROM coordination_nodes node
               JOIN coordination_node_projects attached
                 ON attached.node_id = node.id
              WHERE attached.project_id = ?1
                AND NOT EXISTS (
                    SELECT 1 FROM archived_coordination_nodes archived
                     WHERE archived.node_id = node.id
                )
              ORDER BY node.id",
        )?;
        statement
            .query_map([project_id], |row| {
                Ok((row.get::<_, String>(0)?, super::row_u64(row, 1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (node_id, current_version) in attached_nodes {
        let next_version = current_version
            .checked_add(1)
            .ok_or(ProjectStoreError::VersionOverflow)?;
        let rows = transaction.execute(
            "UPDATE coordination_nodes
                SET version = ?1, updated_at_unix_ms = ?2
              WHERE id = ?3 AND version = ?4",
            params![
                to_i64(next_version)?,
                to_i64(now)?,
                node_id,
                to_i64(current_version)?,
            ],
        )?;
        if rows != 1 {
            return Err(ProjectStoreError::CoordinationNodeVersionConflict { current_version });
        }
        insert_lifecycle_event(
            transaction,
            "coordination_node",
            &node_id,
            next_version,
            event_type,
            actor,
            now,
        )?;
    }
    Ok(())
}

pub(super) async fn list_nodes(
    store: &SqliteProjectStore,
) -> Result<CoordinationNodes, ProjectStoreError> {
    store
        .run(|connection| {
            // A deleted node was archived first, so this hides both.
            let mut statement = connection.prepare(
                "SELECT id
                   FROM coordination_nodes node
                  WHERE NOT EXISTS (
                        SELECT 1 FROM archived_coordination_nodes archived
                         WHERE archived.node_id = node.id
                  )
                  ORDER BY created_at_unix_ms, id",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let nodes = ids
                .into_iter()
                .map(|id| select_node(connection, &id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CoordinationNodes { nodes })
        })
        .await
}

pub(super) async fn get_node(
    store: &SqliteProjectStore,
    node_id: &str,
) -> Result<CoordinationNode, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    store
        .run(move |connection| select_active_node(connection, &node_id))
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn create_node(
    store: &SqliteProjectStore,
    node_id: &str,
    workstream_cwd: Option<String>,
    knowledge_path: Option<String>,
    command: CreateCoordinationNode,
) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    validate_managed_paths(
        command.kind,
        workstream_cwd.as_ref(),
        knowledge_path.as_ref(),
    )?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let project_key = project_ids_key(&command.attached_project_ids);
            let existing = transaction
                .query_row(
                    "SELECT cncc.node_id, cncc.kind, cncc.name,
                            cncc.attached_project_ids, cncc.canvas_x,
                            cncc.canvas_y, cncc.canvas_width,
                            cncc.canvas_height, ca.actor
                       FROM coordination_node_create_commands cncc
                       JOIN command_acknowledgements ca
                         ON ca.id = cncc.command_id
                      WHERE cncc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, f64>(6)?,
                            row.get::<_, f64>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((
                existing_node_id,
                kind,
                name,
                attached_project_ids,
                x,
                y,
                width,
                height,
                actor,
            )) = existing
            {
                if kind != node_kind_value(command.kind)
                    || name != command.name
                    || attached_project_ids != project_key
                    || !same_f64(x, command.placement.x)
                    || !same_f64(y, command.placement.y)
                    || !same_f64(width, command.placement.width)
                    || !same_f64(height, command.placement.height)
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(CoordinationNodeCommandResult {
                    command_id: command.command_id,
                    node: select_node(&transaction, &existing_node_id)?,
                    replayed: true,
                });
            }
            if command_id_exists(&transaction, &command.command_id)? {
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            if transaction.query_row(
                "SELECT EXISTS (SELECT 1 FROM coordination_nodes WHERE id = ?1)",
                [&node_id],
                |row| row.get::<_, bool>(0),
            )? {
                return Err(ProjectStoreError::CoordinationNodeIdConflict);
            }
            validate_projects(&transaction, &command.attached_project_ids)?;

            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_create', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO coordination_nodes (
                    id, name, kind, worker_id, workstream_cwd,
                    knowledge_path, version, created_by,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, 1, ?6, ?7, ?7)",
                params![
                    node_id,
                    command.name,
                    node_kind_value(command.kind),
                    workstream_cwd,
                    knowledge_path,
                    command.actor,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO coordination_node_placements (
                    node_id, canvas_x, canvas_y, canvas_width,
                    canvas_height, version, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![
                    node_id,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                    to_i64(now)?,
                ],
            )?;
            replace_attachments(&transaction, &node_id, &command.attached_project_ids)?;
            transaction.execute(
                "INSERT INTO coordination_node_create_commands (
                    command_id, node_id, kind, name, attached_project_ids,
                    canvas_x, canvas_y, canvas_width, canvas_height
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    command.command_id,
                    node_id,
                    node_kind_value(command.kind),
                    command.name,
                    project_key,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "coordination_node",
                &node_id,
                1,
                "coordination_node_created",
                &command.actor,
                now,
            )?;
            let node = select_node(&transaction, &node_id)?;
            transaction.commit()?;
            Ok(CoordinationNodeCommandResult {
                command_id: command.command_id,
                node,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn update_node(
    store: &SqliteProjectStore,
    node_id: &str,
    command: UpdateCoordinationNode,
) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let project_key = project_ids_key(&command.attached_project_ids);
            let existing = transaction
                .query_row(
                    "SELECT cnuc.node_id, cnuc.expected_version, cnuc.name,
                            cnuc.attached_project_ids, ca.actor
                       FROM coordination_node_update_commands cnuc
                       JOIN command_acknowledgements ca
                         ON ca.id = cnuc.command_id
                      WHERE cnuc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            super::row_u64(row, 1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((existing_node_id, expected_version, name, project_ids, actor)) = existing {
                if existing_node_id != node_id
                    || expected_version != command.expected_version
                    || name != command.name
                    || project_ids != project_key
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(CoordinationNodeCommandResult {
                    command_id: command.command_id,
                    node: select_node(&transaction, &node_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let current = select_active_node(&transaction, &node_id)?;
            if current.version != command.expected_version {
                return Err(ProjectStoreError::CoordinationNodeVersionConflict {
                    current_version: current.version,
                });
            }
            validate_projects(&transaction, &command.attached_project_ids)?;
            let next_version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_update', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE coordination_nodes
                    SET name = ?1, version = ?2, updated_at_unix_ms = ?3
                  WHERE id = ?4 AND version = ?5",
                params![
                    command.name,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    node_id,
                    to_i64(current.version)?,
                ],
            )?;
            replace_attachments(&transaction, &node_id, &command.attached_project_ids)?;
            transaction.execute(
                "INSERT INTO coordination_node_update_commands (
                    command_id, node_id, expected_version, name,
                    attached_project_ids, result_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    command.command_id,
                    node_id,
                    to_i64(command.expected_version)?,
                    command.name,
                    project_key,
                    to_i64(next_version)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "coordination_node",
                &node_id,
                next_version,
                "coordination_node_updated",
                &command.actor,
                now,
            )?;
            let node = select_node(&transaction, &node_id)?;
            transaction.commit()?;
            Ok(CoordinationNodeCommandResult {
                command_id: command.command_id,
                node,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn update_placement(
    store: &SqliteProjectStore,
    node_id: &str,
    command: UpdateCoordinationNodePlacement,
) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT cnpc.node_id, cnpc.expected_version,
                            cnpc.canvas_x, cnpc.canvas_y,
                            cnpc.canvas_width, cnpc.canvas_height, ca.actor
                       FROM coordination_node_placement_commands cnpc
                       JOIN command_acknowledgements ca
                         ON ca.id = cnpc.command_id
                      WHERE cnpc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            super::row_u64(row, 1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((existing_node_id, version, x, y, width, height, actor)) = existing {
                if existing_node_id != node_id
                    || version != command.expected_version
                    || !same_f64(x, command.placement.x)
                    || !same_f64(y, command.placement.y)
                    || !same_f64(width, command.placement.width)
                    || !same_f64(height, command.placement.height)
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(CoordinationNodeCommandResult {
                    command_id: command.command_id,
                    node: select_node(&transaction, &node_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let current = select_active_node(&transaction, &node_id)?;
            if current.placement.version != command.expected_version {
                return Err(
                    ProjectStoreError::CoordinationNodePlacementVersionConflict {
                        current_version: current.placement.version,
                    },
                );
            }
            let next_version = current
                .placement
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_placement', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE coordination_node_placements
                    SET canvas_x = ?1, canvas_y = ?2, canvas_width = ?3,
                        canvas_height = ?4, version = ?5,
                        updated_at_unix_ms = ?6
                  WHERE node_id = ?7 AND version = ?8",
                params![
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    node_id,
                    to_i64(current.placement.version)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO coordination_node_placement_commands (
                    command_id, node_id, expected_version, canvas_x,
                    canvas_y, canvas_width, canvas_height, result_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    command.command_id,
                    node_id,
                    to_i64(command.expected_version)?,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                    to_i64(next_version)?,
                ],
            )?;
            let node = select_node(&transaction, &node_id)?;
            transaction.commit()?;
            Ok(CoordinationNodeCommandResult {
                command_id: command.command_id,
                node,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn configure_node(
    store: &SqliteProjectStore,
    node_id: &str,
    command: ProvisionCoordinationNode,
    worker_id: &str,
    expected_worker_version: u64,
) -> Result<CoordinationNodeCommandResult, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let worker_id = worker_id.trim().to_owned();
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT cnpc.node_id, cnpc.profile_id,
                            cnpc.profile_version, cnpc.expected_node_version,
                            cnpc.worker_id, ca.actor
                       FROM coordination_node_provision_commands cnpc
                       JOIN command_acknowledgements ca
                         ON ca.id = cnpc.command_id
                      WHERE cnpc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            super::row_u64(row, 2)?,
                            super::row_u64(row, 3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((
                existing_node_id,
                profile_id,
                profile_version,
                node_version,
                existing_worker_id,
                actor,
            )) = existing
            {
                if existing_node_id != node_id
                    || profile_id != command.profile_id
                    || profile_version != command.expected_profile_version
                    || node_version != command.expected_node_version
                    || existing_worker_id != worker_id
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(CoordinationNodeCommandResult {
                    command_id: command.command_id,
                    node: select_node(&transaction, &node_id)?,
                    replayed: true,
                });
            }
            let provision_intent = super::select_dedicated_runtime_provision_intent(
                &transaction,
                &command.command_id,
            )?;
            if let Some(intent) = provision_intent.as_ref() {
                let has_runtime_claim =
                    super::select_provisioning_runtime_claim(&transaction, &command.command_id)?
                        .is_some();
                if intent.command_type != "coordination_node_provision"
                    || intent.actor != command.actor
                    || intent.status != "pending"
                    || intent.kind != "coordination_node"
                    || intent.target_id != node_id
                    || intent.profile_id != command.profile_id
                    || intent.profile_version != command.expected_profile_version
                    || intent.expected_target_version != command.expected_node_version
                    || intent
                        .result_worker_id
                        .as_deref()
                        .is_some_and(|claimed_worker_id| claimed_worker_id != worker_id)
                    || (intent.result_worker_id.is_none() && has_runtime_claim)
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
            } else {
                reject_reused_command(&transaction, &command.command_id)?;
            }
            let current = select_active_node(&transaction, &node_id)?;
            if current.kind != CoordinationNodeKind::Workstream {
                return Err(ProjectStoreError::CoordinationNodeKindMismatch);
            }
            if current.version != command.expected_node_version {
                return Err(ProjectStoreError::CoordinationNodeVersionConflict {
                    current_version: current.version,
                });
            }
            if current.worker.is_some() {
                return Err(ProjectStoreError::CoordinationNodeAlreadyProvisioned);
            }
            let candidate = select_worker_candidate(&transaction, &worker_id)?
                .ok_or(ProjectStoreError::WorkerNotFound)?;
            if candidate.worker.version != expected_worker_version {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: candidate.worker.version,
                });
            }
            if candidate.availability != WorkerAvailability::UnassignedLive {
                return Err(ProjectStoreError::WorkerNotAvailable {
                    availability: candidate.availability,
                });
            }
            if candidate.worker.runtime.is_none() {
                return Err(ProjectStoreError::RuntimeBindingMissing);
            }
            if candidate.worker.profile_id.as_deref() != Some(&command.profile_id)
                || candidate.worker.profile_version != Some(command.expected_profile_version)
            {
                return Err(ProjectStoreError::WorkerProfileRevisionMismatch);
            }

            let next_version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            if provision_intent.is_some() {
                transaction.execute(
                    "UPDATE dedicated_runtime_provision_intents
                        SET result_worker_id = COALESCE(result_worker_id, ?1),
                            updated_at_unix_ms = ?2
                      WHERE command_id = ?3",
                    params![worker_id, to_i64(now)?, command.command_id],
                )?;
                let updated = transaction.execute(
                    "UPDATE command_acknowledgements
                        SET status = 'succeeded', error_message = NULL,
                            updated_at_unix_ms = ?1
                      WHERE id = ?2 AND status = 'pending'",
                    params![to_i64(now)?, command.command_id],
                )?;
                if updated != 1 {
                    return Err(ProjectStoreError::CommandNotPending);
                }
            } else {
                transaction.execute(
                    "INSERT INTO command_acknowledgements (
                        id, command_type, actor, status, error_message,
                        created_at_unix_ms, updated_at_unix_ms
                     ) VALUES (
                        ?1, 'coordination_node_provision', ?2, 'succeeded',
                        NULL, ?3, ?3
                     )",
                    params![command.command_id, command.actor, to_i64(now)?],
                )?;
            }
            transaction.execute(
                "UPDATE workers
                    SET version = version + 1, updated_at_unix_ms = ?1
                  WHERE id = ?2 AND version = ?3",
                params![to_i64(now)?, worker_id, to_i64(expected_worker_version)?,],
            )?;
            transaction.execute(
                "UPDATE coordination_nodes
                    SET worker_id = ?1, version = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND version = ?5 AND worker_id IS NULL",
                params![
                    worker_id,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    node_id,
                    to_i64(current.version)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO coordination_node_provision_commands (
                    command_id, node_id, profile_id, profile_version,
                    expected_node_version, worker_id, result_node_version,
                    finished_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    command.command_id,
                    node_id,
                    command.profile_id,
                    to_i64(command.expected_profile_version)?,
                    to_i64(command.expected_node_version)?,
                    worker_id,
                    to_i64(next_version)?,
                    to_i64(now)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "coordination_node",
                &node_id,
                next_version,
                "coordination_node_provisioned",
                &command.actor,
                now,
            )?;
            let node = select_node(&transaction, &node_id)?;
            transaction.commit()?;
            Ok(CoordinationNodeCommandResult {
                command_id: command.command_id,
                node,
                replayed: false,
            })
        })
        .await
}

pub(super) async fn begin_prompt(
    store: &SqliteProjectStore,
    node_id: &str,
    command: SendCoordinationNodePrompt,
    source: super::TokenSpendCommandSource,
) -> Result<BeginCoordinationNodePrompt, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            super::token_spend_store::ensure_command_source_enabled(&transaction, source)?;
            if let Some(existing) = select_prompt_command(&transaction, &command.command_id)? {
                if !existing.matches(&node_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status.as_str() {
                    "succeeded" => Ok(BeginCoordinationNodePrompt::Replayed(
                        existing.acknowledgement()?,
                    )),
                    "pending" => Err(ProjectStoreError::CommandInProgress),
                    "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
                        existing.error_message.unwrap_or_default(),
                    )),
                    "ambiguous" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
                        existing.error_message.unwrap_or_default(),
                    )),
                    _ => Err(ProjectStoreError::CommandNotFound),
                };
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let node = select_active_node(&transaction, &node_id)?;
            validate_workstream_command(&node, command.expected_node_version, &command.worker_id)?;
            reject_pending_node_intervention(&transaction, &node_id)?;
            let runtime = node
                .worker
                .as_ref()
                .and_then(|worker| worker.runtime.as_ref())
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_prompt', ?2, 'pending',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO coordination_node_prompt_commands (
                    command_id, node_id, worker_id, expected_node_version,
                    prompt_text, runtime_session, runtime_pane_id,
                    result_runtime_status, submitted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
                params![
                    command.command_id,
                    node_id,
                    command.worker_id,
                    to_i64(command.expected_node_version)?,
                    command.text,
                    runtime.session,
                    runtime.pane_id,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginCoordinationNodePrompt::Started {
                command,
                node: Box::new(node),
            })
        })
        .await
}

pub(super) async fn succeed_prompt(
    store: &SqliteProjectStore,
    command_id: &str,
    runtime_status: &str,
) -> Result<CoordinationNodePromptAcknowledgement, ProjectStoreError> {
    let command_id = required_command_id(command_id)?;
    let runtime_status = required_runtime_status(runtime_status)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status == "succeeded" {
                return existing.acknowledgement();
            }
            if existing.status != "pending" {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let node = select_node(&transaction, &existing.node_id)?;
            validate_workstream_command(
                &node,
                existing.expected_node_version,
                &existing.worker_id,
            )?;
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE coordination_node_prompt_commands
                    SET result_runtime_status = ?1, submitted_at_unix_ms = ?2
                  WHERE command_id = ?3",
                params![runtime_status, to_i64(now)?, command_id],
            )?;
            mark_command_succeeded(&transaction, &command_id, now)?;
            let acknowledgement = select_prompt_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?
                .acknowledgement()?;
            transaction.commit()?;
            Ok(acknowledgement)
        })
        .await
}

pub(super) async fn fail_prompt(
    store: &SqliteProjectStore,
    command_id: &str,
    message: &str,
    ambiguous: bool,
) -> Result<(), ProjectStoreError> {
    fail_delivery_command(
        store,
        command_id,
        message,
        ambiguous,
        "coordination_node_prompt_commands",
    )
    .await
}

pub(super) async fn latest_delivered_command_id(
    store: &SqliteProjectStore,
    node_id: &str,
) -> Result<Option<String>, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    store
        .run(move |connection| {
            if !node_exists(connection, &node_id)? {
                return Err(ProjectStoreError::CoordinationNodeNotFound);
            }
            connection
                .query_row(
                    "SELECT cnpc.command_id
                       FROM coordination_node_prompt_commands cnpc
                       JOIN command_acknowledgements ca
                         ON ca.id = cnpc.command_id
                      WHERE cnpc.node_id = ?1 AND ca.status = 'succeeded'
                      ORDER BY cnpc.submitted_at_unix_ms DESC,
                               cnpc.command_id DESC
                      LIMIT 1",
                    [&node_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(Into::into)
        })
        .await
}

pub(super) async fn list_routes(
    store: &SqliteProjectStore,
    node_id: &str,
    limit: usize,
) -> Result<CoordinationNodeRoutes, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    if !(1..=MAX_ROUTE_LIST_LIMIT).contains(&limit) {
        return Err(ProjectStoreError::RouteListLimitInvalid {
            max: MAX_ROUTE_LIST_LIMIT,
        });
    }
    store
        .run(move |connection| {
            if !node_exists(connection, &node_id)? {
                return Err(ProjectStoreError::CoordinationNodeNotFound);
            }
            let mut statement = connection.prepare(
                "SELECT cnrc.command_id, cnrc.node_id, ca.actor,
                        cnrc.worker_id, cnrc.expected_node_version,
                        cnrc.target_project_id,
                        cnrc.target_orchestrator_worker_id,
                        cnrc.expected_project_version, cnrc.prompt_text,
                        ca.status, ca.error_message,
                        cnrc.result_runtime_status,
                        ca.created_at_unix_ms, ca.updated_at_unix_ms,
                        cnrc.submitted_at_unix_ms
                   FROM coordination_node_route_commands cnrc
                   JOIN command_acknowledgements ca
                     ON ca.id = cnrc.command_id
                  WHERE cnrc.node_id = ?1
                  ORDER BY ca.created_at_unix_ms DESC, cnrc.command_id DESC
                  LIMIT ?2",
            )?;
            let routes = statement
                .query_map(params![node_id, i64::try_from(limit)?], route_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CoordinationNodeRoutes { routes })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn begin_route(
    store: &SqliteProjectStore,
    node_id: &str,
    command: SendCoordinationNodeRoute,
) -> Result<BeginCoordinationNodeRoute, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_route_command(&transaction, &command.command_id)? {
                if !route_matches(&existing, &node_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return match existing.status {
                    CoordinationDeliveryStatus::Submitted => {
                        Ok(BeginCoordinationNodeRoute::Replayed(existing))
                    }
                    CoordinationDeliveryStatus::Pending => {
                        Err(ProjectStoreError::CommandInProgress)
                    }
                    CoordinationDeliveryStatus::Failed => {
                        Err(ProjectStoreError::CommandPreviouslyFailed(
                            existing.error_message.unwrap_or_default(),
                        ))
                    }
                    CoordinationDeliveryStatus::Ambiguous => {
                        Err(ProjectStoreError::CommandOutcomeAmbiguous(
                            existing.error_message.unwrap_or_default(),
                        ))
                    }
                };
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let node = select_active_node(&transaction, &node_id)?;
            validate_workstream_command(&node, command.expected_node_version, &command.worker_id)?;
            if node
                .attached_project_ids
                .binary_search(&command.target_project_id)
                .is_err()
            {
                return Err(ProjectStoreError::CoordinationNodeProjectNotAttached);
            }
            reject_pending_node_intervention(&transaction, &node_id)?;
            reject_project_orchestrator_intervention(
                &transaction,
                &command.target_project_id,
                None,
            )?;
            let project = select_project(&transaction, &command.target_project_id)?;
            if project.version != command.expected_project_version {
                return Err(ProjectStoreError::ProjectVersionConflict {
                    current_version: project.version,
                });
            }
            let workflow_profile =
                super::orchestrator_workflow_profile_store::select_executable_revision(
                    &transaction,
                    &project.workflow_profile.profile_id,
                    project.workflow_profile.profile_version,
                )?;
            if project.orchestrator.id != command.target_orchestrator_worker_id {
                return Err(ProjectStoreError::OrchestratorNotCurrent {
                    current_worker_id: project.orchestrator.id,
                });
            }
            let runtime = project
                .orchestrator
                .runtime
                .as_ref()
                .ok_or(ProjectStoreError::RuntimeBindingMissing)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_route', ?2, 'pending',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO coordination_node_route_commands (
                    command_id, node_id, worker_id, expected_node_version,
                    target_project_id, target_orchestrator_worker_id,
                    expected_project_version, prompt_text, runtime_session,
                    runtime_pane_id, result_runtime_status,
                    submitted_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL
                 )",
                params![
                    command.command_id,
                    node_id,
                    command.worker_id,
                    to_i64(command.expected_node_version)?,
                    command.target_project_id,
                    command.target_orchestrator_worker_id,
                    to_i64(command.expected_project_version)?,
                    command.text,
                    runtime.session,
                    runtime.pane_id,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginCoordinationNodeRoute::Started {
                command,
                node: Box::new(node),
                target_project: Box::new(project),
                workflow_profile: Box::new(workflow_profile),
            })
        })
        .await
}

pub(super) async fn succeed_route(
    store: &SqliteProjectStore,
    command_id: &str,
    runtime_status: &str,
) -> Result<CoordinationNodeRoute, ProjectStoreError> {
    let command_id = required_command_id(command_id)?;
    let runtime_status = required_runtime_status(runtime_status)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = select_route_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            if existing.status == CoordinationDeliveryStatus::Submitted {
                return Ok(existing);
            }
            if existing.status != CoordinationDeliveryStatus::Pending {
                return Err(ProjectStoreError::CommandNotPending);
            }
            let node = select_node(&transaction, &existing.node_id)?;
            validate_workstream_command(
                &node,
                existing.expected_node_version,
                &existing.worker_id,
            )?;
            if node
                .attached_project_ids
                .binary_search(&existing.target_project_id)
                .is_err()
            {
                return Err(ProjectStoreError::CoordinationNodeProjectNotAttached);
            }
            let project = select_project(&transaction, &existing.target_project_id)?;
            if project.version != existing.expected_project_version
                || project.orchestrator.id != existing.target_orchestrator_worker_id
            {
                return Err(ProjectStoreError::RuntimeBindingVersionConflict);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE coordination_node_route_commands
                    SET result_runtime_status = ?1, submitted_at_unix_ms = ?2
                  WHERE command_id = ?3",
                params![runtime_status, to_i64(now)?, command_id],
            )?;
            mark_command_succeeded(&transaction, &command_id, now)?;
            let route = select_route_command(&transaction, &command_id)?
                .ok_or(ProjectStoreError::CommandNotFound)?;
            transaction.commit()?;
            Ok(route)
        })
        .await
}

pub(super) async fn fail_route(
    store: &SqliteProjectStore,
    command_id: &str,
    message: &str,
    ambiguous: bool,
) -> Result<(), ProjectStoreError> {
    fail_delivery_command(
        store,
        command_id,
        message,
        ambiguous,
        "coordination_node_route_commands",
    )
    .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn create_snapshot(
    store: &SqliteProjectStore,
    node_id: &str,
    snapshot_id: &str,
    folder_path: String,
    mut project_folders: Vec<SnapshotProjectFolder>,
    command: RequestCoordinationSnapshot,
) -> Result<CoordinationSnapshot, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let snapshot_id = canonical_snapshot_id(snapshot_id)?;
    let command = command.normalize()?;
    project_folders.sort_by(|left, right| left.project_id.cmp(&right.project_id));
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT cs.id, cs.node_id, cs.expected_node_version,
                            cs.requested_by
                       FROM coordination_snapshots cs
                      WHERE cs.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            super::row_u64(row, 2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((existing_snapshot_id, existing_node_id, version, actor)) = existing {
                if existing_node_id != node_id
                    || version != command.expected_node_version
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                let mut snapshot = select_snapshot(&transaction, &node_id, &existing_snapshot_id)?;
                snapshot.replayed = true;
                return Ok(snapshot);
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let node = select_active_node(&transaction, &node_id)?;
            if node.kind != CoordinationNodeKind::KnowledgeStore {
                return Err(ProjectStoreError::CoordinationNodeKindMismatch);
            }
            if node.version != command.expected_node_version {
                return Err(ProjectStoreError::CoordinationNodeVersionConflict {
                    current_version: node.version,
                });
            }
            let folder_project_ids = project_folders
                .iter()
                .map(|folder| folder.project_id.as_str())
                .collect::<Vec<_>>();
            let attached_project_ids = node
                .attached_project_ids
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            if folder_project_ids != attached_project_ids {
                return Err(ProjectStoreError::SnapshotAttachmentMismatch);
            }
            if transaction.query_row(
                "SELECT EXISTS (SELECT 1 FROM coordination_snapshots WHERE id = ?1)",
                [&snapshot_id],
                |row| row.get::<_, bool>(0),
            )? {
                return Err(ProjectStoreError::CoordinationSnapshotIdConflict);
            }
            let projects = project_folders
                .iter()
                .map(|folder| {
                    reject_project_orchestrator_intervention(
                        &transaction,
                        &folder.project_id,
                        None,
                    )?;
                    select_project(&transaction, &folder.project_id)
                        .map(|project| (folder, project))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_snapshot_request', ?2, 'succeeded',
                    NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO coordination_snapshots (
                    id, node_id, command_id, expected_node_version,
                    folder_path, requested_by, created_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    snapshot_id,
                    node_id,
                    command.command_id,
                    to_i64(command.expected_node_version)?,
                    folder_path,
                    command.actor,
                    to_i64(now)?,
                ],
            )?;
            for (folder, project) in projects {
                transaction.execute(
                    "INSERT INTO coordination_snapshot_projects (
                        snapshot_id, project_id, project_version,
                        orchestrator_worker_id, folder_path,
                        collection_status, delivery_status, delivery_error,
                        result_runtime_status, submitted_at_unix_ms,
                        collected_at_unix_ms
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, 'pending', 'pending',
                        NULL, NULL, NULL, NULL
                     )",
                    params![
                        snapshot_id,
                        project.id,
                        to_i64(project.version)?,
                        project.orchestrator.id,
                        folder.folder_path,
                    ],
                )?;
            }
            insert_lifecycle_event(
                &transaction,
                "coordination_snapshot",
                &snapshot_id,
                1,
                "coordination_snapshot_requested",
                &command.actor,
                now,
            )?;
            let snapshot = select_snapshot(&transaction, &node_id, &snapshot_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
        .await
}

pub(super) async fn list_snapshots(
    store: &SqliteProjectStore,
    node_id: &str,
) -> Result<CoordinationSnapshots, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    store
        .run(move |connection| {
            if !node_exists(connection, &node_id)? {
                return Err(ProjectStoreError::CoordinationNodeNotFound);
            }
            let mut statement = connection.prepare(
                "SELECT id
                   FROM coordination_snapshots
                  WHERE node_id = ?1
                  ORDER BY created_at_unix_ms DESC, id DESC",
            )?;
            let ids = statement
                .query_map([&node_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let snapshots = ids
                .into_iter()
                .map(|id| select_snapshot(connection, &node_id, &id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CoordinationSnapshots { snapshots })
        })
        .await
}

pub(super) async fn get_snapshot(
    store: &SqliteProjectStore,
    node_id: &str,
    snapshot_id: &str,
) -> Result<CoordinationSnapshot, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let snapshot_id = canonical_snapshot_id(snapshot_id)?;
    store
        .run(move |connection| {
            // Every collection refresh ends with this read, so expiry is
            // applied lazily here after the folder check had its chance.
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            abandon_expired_snapshot_collections(
                &transaction,
                unix_time_ms()?,
                Some(&snapshot_id),
                None,
            )?;
            let snapshot = select_snapshot(&transaction, &node_id, &snapshot_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
        .await
}

/// Mark uncollected project snapshots older than
/// [`SNAPSHOT_COLLECTION_EXPIRY_MS`] as abandoned. A delivery that is still
/// pending is never expired; startup recovery first marks an interrupted one
/// ambiguous.
pub(super) fn abandon_expired_snapshot_collections(
    connection: &Connection,
    now: u64,
    snapshot_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<usize, ProjectStoreError> {
    let cutoff = now.saturating_sub(SNAPSHOT_COLLECTION_EXPIRY_MS);
    connection
        .execute(
            "UPDATE coordination_snapshot_projects
                SET collection_status = 'abandoned',
                    abandoned_at_unix_ms = ?1,
                    abandoned_reason = 'expired'
              WHERE collection_status = 'pending'
                AND delivery_status <> 'pending'
                AND COALESCE(
                        submitted_at_unix_ms,
                        (
                            SELECT snapshot.created_at_unix_ms
                              FROM coordination_snapshots snapshot
                             WHERE snapshot.id =
                                   coordination_snapshot_projects.snapshot_id
                        )
                    ) <= ?2
                AND (?3 IS NULL OR snapshot_id = ?3)
                AND (?4 IS NULL OR project_id = ?4)",
            params![to_i64(now)?, to_i64(cutoff)?, snapshot_id, project_id],
        )
        .map_err(Into::into)
}

/// Snapshot work for one project that continues in the background, after
/// applying expiry. Archive and delete report it but never wait for it.
pub(super) fn project_background_status(
    connection: &Connection,
    project_id: &str,
    now: u64,
) -> Result<ProjectBackgroundStatus, ProjectStoreError> {
    abandon_expired_snapshot_collections(connection, now, None, Some(project_id))?;
    connection
        .query_row(
            "SELECT COALESCE(SUM(collection_status = 'pending'), 0),
                    COALESCE(SUM(collection_status = 'abandoned'), 0)
               FROM coordination_snapshot_projects
              WHERE project_id = ?1",
            [project_id],
            |row| {
                Ok(ProjectBackgroundStatus {
                    snapshots_pending: super::row_u64(row, 0)?,
                    snapshots_abandoned: super::row_u64(row, 1)?,
                })
            },
        )
        .map_err(Into::into)
}

pub(super) async fn record_snapshot_delivery(
    store: &SqliteProjectStore,
    snapshot_id: &str,
    project_id: &str,
    result: SnapshotDeliveryResult,
) -> Result<(), ProjectStoreError> {
    let snapshot_id = canonical_snapshot_id(snapshot_id)?;
    let project_id = canonical_project_id(project_id)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = transaction
                .query_row(
                    "SELECT delivery_status
                       FROM coordination_snapshot_projects
                      WHERE snapshot_id = ?1 AND project_id = ?2",
                    params![snapshot_id, project_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or(ProjectStoreError::SnapshotProjectNotFound)?;
            if current != "pending" {
                return Ok(());
            }
            match result {
                SnapshotDeliveryResult::Submitted { runtime_status } => {
                    let runtime_status = required_runtime_status(&runtime_status)?;
                    transaction.execute(
                        "UPDATE coordination_snapshot_projects
                            SET delivery_status = 'submitted',
                                result_runtime_status = ?1,
                                submitted_at_unix_ms = ?2
                          WHERE snapshot_id = ?3 AND project_id = ?4
                            AND delivery_status = 'pending'",
                        params![
                            runtime_status,
                            to_i64(unix_time_ms()?)?,
                            snapshot_id,
                            project_id,
                        ],
                    )?;
                }
                SnapshotDeliveryResult::Failed { message, ambiguous } => {
                    let message = required_failure_message(&message)?;
                    transaction.execute(
                        "UPDATE coordination_snapshot_projects
                            SET delivery_status = ?1, delivery_error = ?2
                          WHERE snapshot_id = ?3 AND project_id = ?4
                            AND delivery_status = 'pending'",
                        params![
                            if ambiguous { "ambiguous" } else { "failed" },
                            message,
                            snapshot_id,
                            project_id,
                        ],
                    )?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
        .await
}

pub(super) async fn record_snapshot_collected(
    store: &SqliteProjectStore,
    snapshot_id: &str,
    project_id: &str,
) -> Result<(), ProjectStoreError> {
    let snapshot_id = canonical_snapshot_id(snapshot_id)?;
    let project_id = canonical_project_id(project_id)?;
    store
        .run(move |connection| {
            // The folder is the source of truth, so files that arrive after
            // expiry still collect an abandoned project snapshot.
            let rows = connection.execute(
                "UPDATE coordination_snapshot_projects
                    SET collection_status = 'collected',
                        collected_at_unix_ms = ?1,
                        abandoned_at_unix_ms = NULL,
                        abandoned_reason = NULL
                  WHERE snapshot_id = ?2 AND project_id = ?3
                    AND collection_status IN ('pending', 'abandoned')",
                params![to_i64(unix_time_ms()?)?, snapshot_id, project_id],
            )?;
            if rows == 0 {
                let exists = connection.query_row(
                    "SELECT EXISTS (
                        SELECT 1 FROM coordination_snapshot_projects
                         WHERE snapshot_id = ?1 AND project_id = ?2
                     )",
                    params![snapshot_id, project_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !exists {
                    return Err(ProjectStoreError::SnapshotProjectNotFound);
                }
            }
            Ok(())
        })
        .await
}

pub(super) async fn disposition_preview(
    store: &SqliteProjectStore,
    node_id: &str,
) -> Result<CoordinationNodeDispositionPreview, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    store
        .run(move |connection| {
            let node = select_active_node(connection, &node_id)?;
            let worker = node
                .worker
                .as_ref()
                .map(|worker| {
                    let profile_name = match (&worker.profile_id, worker.profile_version) {
                        (Some(profile_id), Some(profile_version)) => connection
                            .query_row(
                                "SELECT name FROM worker_profile_revisions
                                  WHERE profile_id = ?1 AND version = ?2",
                                params![profile_id, to_i64(profile_version)?],
                                |row| row.get::<_, String>(0),
                            )
                            .optional()?,
                        _ => None,
                    };
                    Ok::<_, ProjectStoreError>(CoordinationNodeDispositionWorker {
                        worker_id: worker.id.clone(),
                        worker_version: worker.version,
                        profile_name,
                        runtime_present: worker.runtime.is_some(),
                        will_end: worker.desired_state == WorkerDesiredState::Running,
                    })
                })
                .transpose()?;
            let attached_projects = node
                .attached_project_ids
                .iter()
                .map(|project_id| {
                    Ok(CoordinationNodeDispositionProject {
                        project_id: project_id.clone(),
                        name: connection.query_row(
                            "SELECT name FROM projects WHERE id = ?1",
                            [project_id],
                            |row| row.get::<_, String>(0),
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, ProjectStoreError>>()?;
            let automations = {
                let mut statement = connection.prepare(
                    "SELECT id, name, state
                       FROM automations
                      WHERE scope_kind = 'workstream_coordination_node'
                        AND scope_node_id = ?1
                      ORDER BY created_at_unix_ms, id",
                )?;
                statement
                    .query_map([&node_id], |row| {
                        Ok(CoordinationNodeDispositionAutomation {
                            automation_id: row.get(0)?,
                            name: row.get(1)?,
                            state: match row.get::<_, String>(2)?.as_str() {
                                "active" => AutomationState::Active,
                                "paused" => AutomationState::Paused,
                                value => {
                                    return Err(super::enum_conversion_error(
                                        2,
                                        "automation state",
                                        value,
                                    ));
                                }
                            },
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(CoordinationNodeDispositionPreview {
                supported: disposition_supported(node.kind),
                blockers: pending_node_interventions(connection, &node_id)?,
                node_id: node.id,
                name: node.name,
                kind: node.kind,
                node_version: node.version,
                worker,
                attached_projects,
                automations,
            })
        })
        .await
}

pub(super) async fn archive_node(
    store: &SqliteProjectStore,
    node_id: &str,
    command: ArchiveCoordinationNode,
) -> Result<ArchivedCoordinationNode, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_archived_node_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&node_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                let cleanup_pending =
                    super::runtime_cleanup_pending(&transaction, &command.command_id)?;
                let paused_automation_ids =
                    paused_automation_ids(&transaction, &command.command_id)?;
                transaction.commit()?;
                return Ok(ArchivedCoordinationNode {
                    command_id: existing.command_id,
                    node_id: existing.node_id,
                    kind: existing.kind,
                    worker_id: existing.worker_id,
                    paused_automation_ids,
                    archived_at_unix_ms: existing.archived_at_unix_ms,
                    cleanup_pending,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            if archived_node_exists(&transaction, &node_id)? {
                return Err(ProjectStoreError::CoordinationNodeAlreadyArchived);
            }
            // Unknown IDs and unsupported kinds are rejected before anything
            // is written.
            if !disposition_supported(node_kind(&transaction, &node_id)?) {
                return Err(ProjectStoreError::CoordinationNodeKindNotSupported);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_archive', ?2, 'pending', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            let archived = archive_node_tx(
                &transaction,
                &node_id,
                &command.command_id,
                &command.actor,
                &command.preconditions(),
                now,
            )?;
            mark_command_succeeded(&transaction, &command.command_id, now)?;
            transaction.commit()?;
            Ok(ArchivedCoordinationNode {
                command_id: command.command_id,
                node_id,
                kind: archived.kind,
                worker_id: archived.worker_id,
                paused_automation_ids: archived.paused_automation_ids,
                archived_at_unix_ms: now,
                cleanup_pending: archived.cleanup_pending,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn delete_node(
    store: &SqliteProjectStore,
    node_id: &str,
    command: DeleteCoordinationNode,
) -> Result<DeletedCoordinationNode, ProjectStoreError> {
    let node_id = canonical_node_id(node_id)?;
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_deleted_node_command(&transaction, &command.command_id)?
            {
                if !existing.matches(&node_id, &command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                let cleanup_pending = existing
                    .worker_id
                    .as_deref()
                    .map(|worker_id| super::worker_runtime_cleanup_pending(&transaction, worker_id))
                    .transpose()?
                    .unwrap_or(false);
                let paused_automation_ids = if existing.archive_applied {
                    paused_automation_ids(&transaction, &command.command_id)?
                } else {
                    Vec::new()
                };
                transaction.commit()?;
                return Ok(DeletedCoordinationNode {
                    command_id: existing.command_id,
                    node_id: existing.node_id,
                    kind: existing.kind,
                    worker_id: existing.worker_id,
                    paused_automation_ids,
                    deleted_at_unix_ms: existing.deleted_at_unix_ms,
                    cleanup_pending,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            if deleted_node_exists(&transaction, &node_id)? {
                return Err(ProjectStoreError::CoordinationNodeAlreadyDeleted);
            }

            // An archive by any command, including another tab's, wins over
            // this command's archive preconditions.
            let archived_target = select_archived_node_target(&transaction, &node_id)?;
            let kind = node_kind(&transaction, &node_id)?;
            if !disposition_supported(kind) {
                return Err(ProjectStoreError::CoordinationNodeKindNotSupported);
            }
            if archived_target.is_none() && command.archive.is_none() {
                return Err(ProjectStoreError::CoordinationNodeNotArchived);
            }
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'coordination_node_delete', ?2, 'succeeded', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            let archive_applied = archived_target.is_none();
            let (worker_id, paused_automation_ids) = if let Some(target) = archived_target {
                (target.worker_id, Vec::new())
            } else {
                let archive = command
                    .archive
                    .as_ref()
                    .ok_or(ProjectStoreError::CoordinationNodeNotArchived)?;
                let archived = archive_node_tx(
                    &transaction,
                    &node_id,
                    &command.command_id,
                    &command.actor,
                    archive,
                    now,
                )?;
                (archived.worker_id, archived.paused_automation_ids)
            };
            let node_version = transaction.query_row(
                "SELECT version FROM coordination_nodes WHERE id = ?1",
                [&node_id],
                |row| super::row_u64(row, 0),
            )?;
            transaction.execute(
                "INSERT INTO deleted_coordination_nodes (
                    command_id, node_id, actor, archive_json, archive_applied,
                    node_version, deleted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    command.command_id,
                    node_id,
                    command.actor,
                    command
                        .archive
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(ProjectStoreError::StoredResultJson)?,
                    archive_applied,
                    to_i64(node_version)?,
                    to_i64(now)?,
                ],
            )?;
            // The dedicated worker belongs to the node, so it leaves Yard
            // views with it, as a deleted project's orchestrator does.
            // Member projects and their attachment rows are not touched.
            if let Some(worker_id) = worker_id.as_deref()
                && !super::deleted_worker_exists(&transaction, worker_id)?
            {
                let worker_version = transaction.query_row(
                    "SELECT version FROM workers WHERE id = ?1",
                    [worker_id],
                    |row| super::row_u64(row, 0),
                )?;
                transaction.execute(
                    "INSERT INTO deleted_workers (
                        worker_id, command_id, expected_worker_version,
                        deleted_at_unix_ms
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        worker_id,
                        command.command_id,
                        to_i64(worker_version)?,
                        to_i64(now)?,
                    ],
                )?;
            }
            insert_lifecycle_event(
                &transaction,
                "coordination_node",
                &node_id,
                node_version,
                "coordination_node_deleted",
                &command.actor,
                now,
            )?;
            let cleanup_pending = worker_id
                .as_deref()
                .map(|worker_id| super::worker_runtime_cleanup_pending(&transaction, worker_id))
                .transpose()?
                .unwrap_or(false);
            transaction.commit()?;
            Ok(DeletedCoordinationNode {
                command_id: command.command_id,
                node_id,
                kind,
                worker_id,
                paused_automation_ids,
                deleted_at_unix_ms: now,
                cleanup_pending,
                replayed: false,
            })
        })
        .await
}

struct ArchivedNodeTransition {
    kind: CoordinationNodeKind,
    worker_id: Option<String>,
    paused_automation_ids: Vec<String>,
    cleanup_pending: bool,
}

/// Archive an active workstream inside the caller's transaction.
///
/// The caller owns the command: `command_id` must already be in
/// `command_acknowledgements`, and the archive row, retired worker runtime,
/// cleanup job, and paused automations are all recorded under it. Attached
/// projects, their bindings and assignments, and the attachment rows are not
/// touched.
#[allow(clippy::too_many_lines)]
fn archive_node_tx(
    transaction: &Transaction<'_>,
    node_id: &str,
    command_id: &str,
    actor: &str,
    preconditions: &CoordinationNodeArchivePreconditions,
    now: u64,
) -> Result<ArchivedNodeTransition, ProjectStoreError> {
    let node = select_active_node(transaction, node_id)?;
    if !disposition_supported(node.kind) {
        return Err(ProjectStoreError::CoordinationNodeKindNotSupported);
    }
    if node.version != preconditions.expected_node_version {
        return Err(ProjectStoreError::CoordinationNodeVersionConflict {
            current_version: node.version,
        });
    }
    if let Some(expected_worker_version) = preconditions.expected_worker_version {
        let worker = node
            .worker
            .as_ref()
            .ok_or(ProjectStoreError::CoordinationNodeNotProvisioned)?;
        if worker.version != expected_worker_version {
            return Err(ProjectStoreError::WorkerVersionConflict {
                current_version: worker.version,
            });
        }
    }
    let blockers = pending_node_interventions(transaction, node_id)?;
    if !blockers.is_empty() {
        let pending_prompts = blockers
            .iter()
            .filter(|blocker| blocker.kind == CoordinationNodeDispositionBlockerKind::Prompt)
            .count();
        return Err(ProjectStoreError::CoordinationNodeDispositionBlocked {
            pending_prompts,
            pending_routes: blockers.len() - pending_prompts,
        });
    }
    let next_node_version = node
        .version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;

    let (result_worker_version, cleanup_pending) = match node.worker.as_ref() {
        Some(worker) if worker.desired_state == WorkerDesiredState::Running => {
            let next_worker_version = worker
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            // Ending the dedicated worker is a worker session end: its runtime
            // is retired and cleanup succeeds only once Herdr no longer shows
            // it. Yard never closes the tab; the user does.
            let cleanup_pending = if let Some(runtime) = worker.runtime.as_ref() {
                super::insert_retired_runtime_binding(
                    transaction,
                    command_id,
                    &worker.id,
                    "worker_session_end",
                    runtime,
                    now,
                )?;
                super::insert_runtime_cleanup_job(
                    transaction,
                    command_id,
                    &worker.id,
                    "worker_session_end",
                    runtime,
                    super::RuntimeCleanupExpectation {
                        worker_version: Some(next_worker_version),
                        binding_state: "detached",
                    },
                    now,
                )?;
                let runtime_rows = transaction.execute(
                    "DELETE FROM worker_runtime_bindings
                      WHERE worker_id = ?1 AND version = ?2",
                    params![worker.id, to_i64(runtime.version)?],
                )?;
                if runtime_rows != 1 {
                    return Err(ProjectStoreError::WorkerRuntimeVersionConflict {
                        current_version: None,
                    });
                }
                true
            } else {
                false
            };
            let worker_rows = transaction.execute(
                "UPDATE workers
                    SET ended_at_unix_ms = ?1, version = ?2,
                        updated_at_unix_ms = ?1
                  WHERE id = ?3 AND version = ?4
                    AND ended_at_unix_ms IS NULL",
                params![
                    to_i64(now)?,
                    to_i64(next_worker_version)?,
                    worker.id,
                    to_i64(worker.version)?,
                ],
            )?;
            if worker_rows != 1 {
                return Err(ProjectStoreError::WorkerVersionConflict {
                    current_version: worker.version,
                });
            }
            insert_lifecycle_event(
                transaction,
                "worker",
                &worker.id,
                next_worker_version,
                "coordination_node_archive_worker_ended",
                actor,
                now,
            )?;
            (Some(next_worker_version), cleanup_pending)
        }
        Some(worker) => (Some(worker.version), false),
        None => (None, false),
    };
    let worker_id = node.worker.as_ref().map(|worker| worker.id.clone());
    transaction.execute(
        "INSERT INTO archived_coordination_nodes (
            command_id, node_id, actor, expected_node_version,
            expected_worker_version, result_node_version, worker_id,
            result_worker_version, archived_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            command_id,
            node_id,
            actor,
            to_i64(preconditions.expected_node_version)?,
            preconditions
                .expected_worker_version
                .map(to_i64)
                .transpose()?,
            to_i64(next_node_version)?,
            worker_id,
            result_worker_version.map(to_i64).transpose()?,
            to_i64(now)?,
        ],
    )?;
    pause_node_automations(transaction, command_id, node_id, actor, now)?;
    let node_rows = transaction.execute(
        "UPDATE coordination_nodes
            SET version = ?1, updated_at_unix_ms = ?2
          WHERE id = ?3 AND version = ?4",
        params![
            to_i64(next_node_version)?,
            to_i64(now)?,
            node_id,
            to_i64(node.version)?,
        ],
    )?;
    if node_rows != 1 {
        return Err(ProjectStoreError::CoordinationNodeVersionConflict {
            current_version: node.version,
        });
    }
    insert_lifecycle_event(
        transaction,
        "coordination_node",
        node_id,
        next_node_version,
        "coordination_node_archived",
        actor,
        now,
    )?;
    Ok(ArchivedNodeTransition {
        kind: node.kind,
        worker_id,
        paused_automation_ids: paused_automation_ids(transaction, command_id)?,
        cleanup_pending,
    })
}

/// Pause the node's active automations so no scheduled run targets the
/// archived node. Already paused automations are left as they are.
fn pause_node_automations(
    transaction: &Transaction<'_>,
    command_id: &str,
    node_id: &str,
    actor: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let automations = {
        let mut statement = transaction.prepare(
            "SELECT id, version
               FROM automations
              WHERE scope_kind = 'workstream_coordination_node'
                AND scope_node_id = ?1
                AND state = 'active'
              ORDER BY created_at_unix_ms, id",
        )?;
        statement
            .query_map([node_id], |row| {
                Ok((row.get::<_, String>(0)?, super::row_u64(row, 1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (automation_id, current_version) in automations {
        let next_version = current_version
            .checked_add(1)
            .ok_or(ProjectStoreError::VersionOverflow)?;
        let rows = transaction.execute(
            "UPDATE automations
                SET state = 'paused', next_run_at_unix_ms = NULL,
                    version = ?1, updated_at_unix_ms = ?2
              WHERE id = ?3 AND version = ?4 AND state = 'active'",
            params![
                to_i64(next_version)?,
                to_i64(now)?,
                automation_id,
                to_i64(current_version)?,
            ],
        )?;
        if rows != 1 {
            return Err(ProjectStoreError::AutomationVersionConflict { current_version });
        }
        transaction.execute(
            "INSERT INTO archived_coordination_node_automations (
                command_id, automation_id, result_automation_version
             ) VALUES (?1, ?2, ?3)",
            params![command_id, automation_id, to_i64(next_version)?],
        )?;
        insert_lifecycle_event(
            transaction,
            "automation",
            &automation_id,
            next_version,
            "automation_paused",
            actor,
            now,
        )?;
    }
    Ok(())
}

/// Only workstreams have a disposition today. Knowledge stores still own
/// snapshot collection, which archive would have to settle first.
const fn disposition_supported(kind: CoordinationNodeKind) -> bool {
    matches!(kind, CoordinationNodeKind::Workstream)
}

/// Pending (not ambiguous) node prompts and routes. Restart recovery turns an
/// interrupted one ambiguous, so each of these is still being delivered.
fn pending_node_interventions(
    connection: &Connection,
    node_id: &str,
) -> Result<Vec<CoordinationNodeDispositionBlocker>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT 'prompt', prompt.command_id, command.created_at_unix_ms
           FROM coordination_node_prompt_commands prompt
           JOIN command_acknowledgements command
             ON command.id = prompt.command_id
          WHERE prompt.node_id = ?1 AND command.status = 'pending'
         UNION ALL
         SELECT 'route', route.command_id, command.created_at_unix_ms
           FROM coordination_node_route_commands route
           JOIN command_acknowledgements command
             ON command.id = route.command_id
          WHERE route.node_id = ?1 AND command.status = 'pending'
          ORDER BY 3, 2",
    )?;
    statement
        .query_map([node_id], |row| {
            Ok(CoordinationNodeDispositionBlocker {
                kind: if row.get::<_, String>(0)? == "prompt" {
                    CoordinationNodeDispositionBlockerKind::Prompt
                } else {
                    CoordinationNodeDispositionBlockerKind::Route
                },
                command_id: row.get(1)?,
                started_at_unix_ms: super::row_u64(row, 2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Read a node that can still change. An archived node answers
/// `CoordinationNodeArchived`, so its mutations fail as a conflict; a deleted
/// node reads as not found.
pub(super) fn select_active_node(
    connection: &Connection,
    node_id: &str,
) -> Result<CoordinationNode, ProjectStoreError> {
    reject_inactive_node(connection, node_id)?;
    select_node(connection, node_id)
}

pub(super) fn reject_inactive_node(
    connection: &Connection,
    node_id: &str,
) -> Result<(), ProjectStoreError> {
    if deleted_node_exists(connection, node_id)? {
        Err(ProjectStoreError::CoordinationNodeNotFound)
    } else if archived_node_exists(connection, node_id)? {
        Err(ProjectStoreError::CoordinationNodeArchived)
    } else {
        Ok(())
    }
}

fn archived_node_exists(connection: &Connection, node_id: &str) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM archived_coordination_nodes WHERE node_id = ?1
             )",
            [node_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn deleted_node_exists(connection: &Connection, node_id: &str) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM deleted_coordination_nodes WHERE node_id = ?1
             )",
            [node_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

struct ArchivedNodeDeletionTarget {
    worker_id: Option<String>,
}

/// The node's archive row, whichever command wrote it, or `None` while the
/// node is still active.
fn select_archived_node_target(
    connection: &Connection,
    node_id: &str,
) -> Result<Option<ArchivedNodeDeletionTarget>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT worker_id FROM archived_coordination_nodes WHERE node_id = ?1",
            [node_id],
            |row| {
                Ok(ArchivedNodeDeletionTarget {
                    worker_id: row.get(0)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn node_kind(
    connection: &Connection,
    node_id: &str,
) -> Result<CoordinationNodeKind, ProjectStoreError> {
    connection
        .query_row(
            "SELECT kind FROM coordination_nodes WHERE id = ?1",
            [node_id],
            |row| node_kind_from_row(row, 0),
        )
        .optional()?
        .ok_or(ProjectStoreError::CoordinationNodeNotFound)
}

fn paused_automation_ids(
    connection: &Connection,
    command_id: &str,
) -> Result<Vec<String>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT automation_id
           FROM archived_coordination_node_automations
          WHERE command_id = ?1
          ORDER BY automation_id",
    )?;
    statement
        .query_map([command_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

struct StoredArchivedNode {
    command_id: String,
    node_id: String,
    kind: CoordinationNodeKind,
    actor: String,
    expected_node_version: u64,
    expected_worker_version: Option<u64>,
    worker_id: Option<String>,
    archived_at_unix_ms: u64,
}

impl StoredArchivedNode {
    fn matches(&self, node_id: &str, command: &ArchiveCoordinationNode) -> bool {
        self.node_id == node_id
            && self.actor == command.actor
            && self.expected_node_version == command.expected_node_version
            && self.expected_worker_version == command.expected_worker_version
    }
}

/// An archive command's stored row. A delete that archived an active node
/// shares its command ID, so the type filter keeps it from replaying here.
fn select_archived_node_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredArchivedNode>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT archived.command_id, archived.node_id, node.kind,
                    archived.actor, archived.expected_node_version,
                    archived.expected_worker_version, archived.worker_id,
                    archived.archived_at_unix_ms
               FROM archived_coordination_nodes archived
               JOIN command_acknowledgements command
                 ON command.id = archived.command_id
               JOIN coordination_nodes node ON node.id = archived.node_id
              WHERE archived.command_id = ?1
                AND command.command_type = 'coordination_node_archive'",
            [command_id],
            |row| {
                Ok(StoredArchivedNode {
                    command_id: row.get(0)?,
                    node_id: row.get(1)?,
                    kind: node_kind_from_row(row, 2)?,
                    actor: row.get(3)?,
                    expected_node_version: super::row_u64(row, 4)?,
                    expected_worker_version: super::row_optional_u64(row, 5)?,
                    worker_id: row.get(6)?,
                    archived_at_unix_ms: super::row_u64(row, 7)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

struct StoredDeletedNode {
    command_id: String,
    node_id: String,
    kind: CoordinationNodeKind,
    actor: String,
    archive: Option<CoordinationNodeArchivePreconditions>,
    archive_applied: bool,
    worker_id: Option<String>,
    deleted_at_unix_ms: u64,
}

impl StoredDeletedNode {
    fn matches(&self, node_id: &str, command: &DeleteCoordinationNode) -> bool {
        self.node_id == node_id && self.actor == command.actor && self.archive == command.archive
    }
}

fn select_deleted_node_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<StoredDeletedNode>, ProjectStoreError> {
    let stored = connection
        .query_row(
            "SELECT deleted.command_id, deleted.node_id, node.kind,
                    deleted.actor, deleted.archive_json,
                    deleted.archive_applied, archived.worker_id,
                    deleted.deleted_at_unix_ms
               FROM deleted_coordination_nodes deleted
               JOIN command_acknowledgements command
                 ON command.id = deleted.command_id
               JOIN archived_coordination_nodes archived
                 ON archived.node_id = deleted.node_id
               JOIN coordination_nodes node ON node.id = deleted.node_id
              WHERE deleted.command_id = ?1
                AND command.command_type = 'coordination_node_delete'",
            [command_id],
            |row| {
                Ok((
                    StoredDeletedNode {
                        command_id: row.get(0)?,
                        node_id: row.get(1)?,
                        kind: node_kind_from_row(row, 2)?,
                        actor: row.get(3)?,
                        archive: None,
                        archive_applied: row.get(5)?,
                        worker_id: row.get(6)?,
                        deleted_at_unix_ms: super::row_u64(row, 7)?,
                    },
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(|(mut stored, archive_json)| {
            stored.archive = archive_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(ProjectStoreError::StoredResultJson)?;
            Ok(stored)
        })
        .transpose()
}

pub(super) fn select_node(
    connection: &Connection,
    node_id: &str,
) -> Result<CoordinationNode, ProjectStoreError> {
    // A deleted node reads as not found everywhere, command replays
    // included: its dedicated worker was tombstoned with it.
    if deleted_node_exists(connection, node_id)? {
        return Err(ProjectStoreError::CoordinationNodeNotFound);
    }
    let base = connection
        .query_row(
            "SELECT n.id, n.name, n.kind, n.worker_id,
                    n.workstream_cwd, n.knowledge_path, n.version,
                    n.created_by, n.created_at_unix_ms,
                    n.updated_at_unix_ms, p.canvas_x, p.canvas_y,
                    p.canvas_width, p.canvas_height, p.version,
                    p.updated_at_unix_ms
               FROM coordination_nodes n
               JOIN coordination_node_placements p ON p.node_id = n.id
              WHERE n.id = ?1",
            [node_id],
            |row| {
                Ok(NodeBase {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    kind: node_kind_from_row(row, 2)?,
                    worker_id: row.get(3)?,
                    cwd: row.get(4)?,
                    folder_path: row.get(5)?,
                    version: super::row_u64(row, 6)?,
                    created_by: row.get(7)?,
                    created_at_unix_ms: super::row_u64(row, 8)?,
                    updated_at_unix_ms: super::row_u64(row, 9)?,
                    placement: CoordinationNodePlacement {
                        geometry: yard_domain::CanvasPlacement {
                            x: row.get(10)?,
                            y: row.get(11)?,
                            width: row.get(12)?,
                            height: row.get(13)?,
                        },
                        version: super::row_u64(row, 14)?,
                        updated_at_unix_ms: super::row_u64(row, 15)?,
                    },
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::CoordinationNodeNotFound)?;
    let worker = base
        .worker_id
        .as_deref()
        .map(|worker_id| {
            select_worker_candidate(connection, worker_id)?
                .map(|candidate| candidate.worker)
                .ok_or(ProjectStoreError::WorkerNotFound)
        })
        .transpose()?;
    let mut statement = connection.prepare(
        "SELECT attached.project_id
           FROM coordination_node_projects attached
           JOIN project_workspace_bindings binding
             ON binding.project_id = attached.project_id
          WHERE attached.node_id = ?1
          ORDER BY attached.project_id",
    )?;
    let attached_project_ids = statement
        .query_map([node_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CoordinationNode {
        id: base.id,
        name: base.name,
        kind: base.kind,
        placement: base.placement,
        attached_project_ids,
        worker,
        cwd: base.cwd,
        folder_path: base.folder_path,
        version: base.version,
        created_by: base.created_by,
        created_at_unix_ms: base.created_at_unix_ms,
        updated_at_unix_ms: base.updated_at_unix_ms,
    })
}

fn select_snapshot(
    connection: &Connection,
    node_id: &str,
    snapshot_id: &str,
) -> Result<CoordinationSnapshot, ProjectStoreError> {
    let (command_id, folder_path, requested_by, created_at_unix_ms) = connection
        .query_row(
            "SELECT command_id, folder_path, requested_by, created_at_unix_ms
               FROM coordination_snapshots
              WHERE node_id = ?1 AND id = ?2",
            params![node_id, snapshot_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    super::row_u64(row, 3)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::CoordinationSnapshotNotFound)?;
    let mut statement = connection.prepare(
        "SELECT project_id, project_version, orchestrator_worker_id,
                folder_path, collection_status, delivery_status,
                delivery_error, result_runtime_status,
                submitted_at_unix_ms, collected_at_unix_ms,
                abandoned_at_unix_ms, abandoned_reason
           FROM coordination_snapshot_projects
          WHERE snapshot_id = ?1
          ORDER BY project_id",
    )?;
    let projects = statement
        .query_map([snapshot_id], snapshot_project_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let count = |status| {
        projects
            .iter()
            .filter(|project| project.collection_status == status)
            .count()
    };
    Ok(CoordinationSnapshot {
        id: snapshot_id.to_owned(),
        node_id: node_id.to_owned(),
        command_id,
        folder_path,
        progress: SnapshotCollectionProgress {
            completed: count(SnapshotCollectionStatus::Collected),
            total: projects.len(),
            abandoned: count(SnapshotCollectionStatus::Abandoned),
        },
        projects,
        requested_by,
        created_at_unix_ms,
        replayed: false,
    })
}

fn snapshot_project_from_row(row: &Row<'_>) -> rusqlite::Result<SnapshotProjectCollection> {
    Ok(SnapshotProjectCollection {
        project_id: row.get(0)?,
        project_version: super::row_u64(row, 1)?,
        orchestrator_worker_id: row.get(2)?,
        folder_path: row.get(3)?,
        collection_status: match row.get::<_, String>(4)?.as_str() {
            "pending" => SnapshotCollectionStatus::Pending,
            "collected" => SnapshotCollectionStatus::Collected,
            "abandoned" => SnapshotCollectionStatus::Abandoned,
            value => {
                return Err(super::enum_conversion_error(
                    4,
                    "snapshot collection",
                    value,
                ));
            }
        },
        delivery_status: delivery_status_from_row(row, 5)?,
        delivery_error: row.get(6)?,
        runtime_status: row.get(7)?,
        submitted_at_unix_ms: super::row_optional_u64(row, 8)?,
        collected_at_unix_ms: super::row_optional_u64(row, 9)?,
        abandoned_at_unix_ms: super::row_optional_u64(row, 10)?,
        abandoned_reason: row
            .get::<_, Option<String>>(11)?
            .map(|value| match value.as_str() {
                "expired" => Ok(SnapshotAbandonmentReason::Expired),
                value => Err(super::enum_conversion_error(
                    11,
                    "snapshot abandonment",
                    value,
                )),
            })
            .transpose()?,
    })
}

fn select_prompt_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<PromptRecord>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT cnpc.node_id, cnpc.worker_id,
                    cnpc.expected_node_version, cnpc.prompt_text,
                    ca.actor, ca.status, ca.error_message,
                    cnpc.result_runtime_status, cnpc.submitted_at_unix_ms
               FROM coordination_node_prompt_commands cnpc
               JOIN command_acknowledgements ca ON ca.id = cnpc.command_id
              WHERE cnpc.command_id = ?1",
            [command_id],
            |row| {
                Ok(PromptRecord {
                    command_id: command_id.to_owned(),
                    node_id: row.get(0)?,
                    worker_id: row.get(1)?,
                    expected_node_version: super::row_u64(row, 2)?,
                    text: row.get(3)?,
                    actor: row.get(4)?,
                    status: row.get(5)?,
                    error_message: row.get(6)?,
                    runtime_status: row.get(7)?,
                    submitted_at_unix_ms: super::row_optional_u64(row, 8)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn select_route_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<CoordinationNodeRoute>, ProjectStoreError> {
    connection
        .query_row(
            "SELECT cnrc.command_id, cnrc.node_id, ca.actor,
                    cnrc.worker_id, cnrc.expected_node_version,
                    cnrc.target_project_id,
                    cnrc.target_orchestrator_worker_id,
                    cnrc.expected_project_version, cnrc.prompt_text,
                    ca.status, ca.error_message,
                    cnrc.result_runtime_status,
                    ca.created_at_unix_ms, ca.updated_at_unix_ms,
                    cnrc.submitted_at_unix_ms
               FROM coordination_node_route_commands cnrc
               JOIN command_acknowledgements ca ON ca.id = cnrc.command_id
              WHERE cnrc.command_id = ?1",
            [command_id],
            route_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn route_from_row(row: &Row<'_>) -> rusqlite::Result<CoordinationNodeRoute> {
    Ok(CoordinationNodeRoute {
        command_id: row.get(0)?,
        node_id: row.get(1)?,
        actor: row.get(2)?,
        worker_id: row.get(3)?,
        expected_node_version: super::row_u64(row, 4)?,
        target_project_id: row.get(5)?,
        target_orchestrator_worker_id: row.get(6)?,
        expected_project_version: super::row_u64(row, 7)?,
        text: row.get(8)?,
        status: delivery_status_from_row(row, 9)?,
        error_message: row.get(10)?,
        runtime_status: row.get(11)?,
        created_at_unix_ms: super::row_u64(row, 12)?,
        updated_at_unix_ms: super::row_u64(row, 13)?,
        submitted_at_unix_ms: super::row_optional_u64(row, 14)?,
    })
}

fn delivery_status_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<CoordinationDeliveryStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "pending" => Ok(CoordinationDeliveryStatus::Pending),
        "succeeded" | "submitted" => Ok(CoordinationDeliveryStatus::Submitted),
        "failed" => Ok(CoordinationDeliveryStatus::Failed),
        "ambiguous" => Ok(CoordinationDeliveryStatus::Ambiguous),
        value => Err(super::enum_conversion_error(
            index,
            "coordination delivery status",
            value,
        )),
    }
}

async fn fail_delivery_command(
    store: &SqliteProjectStore,
    command_id: &str,
    message: &str,
    ambiguous: bool,
    command_table: &'static str,
) -> Result<(), ProjectStoreError> {
    let command_id = required_command_id(command_id)?;
    let message = required_failure_message(message)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let exists = transaction.query_row(
                &format!("SELECT EXISTS (SELECT 1 FROM {command_table} WHERE command_id = ?1)"),
                [&command_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(ProjectStoreError::CommandNotFound);
            }
            let changed = transaction.execute(
                "UPDATE command_acknowledgements
                    SET status = ?1, error_message = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending'",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    message,
                    to_i64(unix_time_ms()?)?,
                    command_id,
                ],
            )?;
            if changed == 0 {
                return Err(ProjectStoreError::CommandNotPending);
            }
            transaction.commit()?;
            Ok(())
        })
        .await
}

fn validate_workstream_command(
    node: &CoordinationNode,
    expected_version: u64,
    worker_id: &str,
) -> Result<(), ProjectStoreError> {
    if node.kind != CoordinationNodeKind::Workstream {
        return Err(ProjectStoreError::CoordinationNodeKindMismatch);
    }
    if node.version != expected_version {
        return Err(ProjectStoreError::CoordinationNodeVersionConflict {
            current_version: node.version,
        });
    }
    let worker = node
        .worker
        .as_ref()
        .ok_or(ProjectStoreError::CoordinationNodeNotProvisioned)?;
    if worker.id != worker_id {
        return Err(ProjectStoreError::CoordinationNodeWorkerChanged {
            current_worker_id: worker.id.clone(),
        });
    }
    Ok(())
}

fn reject_pending_node_intervention(
    connection: &Connection,
    node_id: &str,
) -> Result<(), ProjectStoreError> {
    let pending = connection.query_row(
        "SELECT EXISTS (
            SELECT 1
              FROM coordination_node_prompt_commands cnpc
              JOIN command_acknowledgements ca ON ca.id = cnpc.command_id
             WHERE cnpc.node_id = ?1 AND ca.status = 'pending'
            UNION ALL
            SELECT 1
              FROM coordination_node_route_commands cnrc
              JOIN command_acknowledgements ca ON ca.id = cnrc.command_id
             WHERE cnrc.node_id = ?1 AND ca.status = 'pending'
         )",
        [node_id],
        |row| row.get::<_, bool>(0),
    )?;
    if pending {
        Err(ProjectStoreError::CoordinationNodeInterventionInProgress)
    } else {
        Ok(())
    }
}

fn route_matches(
    route: &CoordinationNodeRoute,
    node_id: &str,
    command: &SendCoordinationNodeRoute,
) -> bool {
    route.node_id == node_id
        && route.actor == command.actor
        && route.worker_id == command.worker_id
        && route.expected_node_version == command.expected_node_version
        && route.target_project_id == command.target_project_id
        && route.expected_project_version == command.expected_project_version
        && route.target_orchestrator_worker_id == command.target_orchestrator_worker_id
        && route.text == command.text
}

fn mark_command_succeeded(
    connection: &Connection,
    command_id: &str,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let rows = connection.execute(
        "UPDATE command_acknowledgements
            SET status = 'succeeded', error_message = NULL,
                updated_at_unix_ms = ?1
          WHERE id = ?2 AND status = 'pending'",
        params![to_i64(now)?, command_id],
    )?;
    if rows != 1 {
        return Err(ProjectStoreError::CommandNotPending);
    }
    Ok(())
}

fn replace_attachments(
    connection: &Connection,
    node_id: &str,
    project_ids: &[String],
) -> Result<(), ProjectStoreError> {
    connection.execute(
        "DELETE FROM coordination_node_projects WHERE node_id = ?1",
        [node_id],
    )?;
    for project_id in project_ids {
        connection.execute(
            "INSERT INTO coordination_node_projects (node_id, project_id)
             VALUES (?1, ?2)",
            params![node_id, project_id],
        )?;
    }
    Ok(())
}

fn validate_projects(
    connection: &Connection,
    project_ids: &[String],
) -> Result<(), ProjectStoreError> {
    for project_id in project_ids {
        if !project_exists(connection, project_id)? {
            return Err(ProjectStoreError::AttachedProjectNotFound {
                project_id: project_id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_managed_paths(
    kind: CoordinationNodeKind,
    workstream_cwd: Option<&String>,
    knowledge_path: Option<&String>,
) -> Result<(), ProjectStoreError> {
    match kind {
        CoordinationNodeKind::Workstream
            if workstream_cwd.is_some_and(|path| !path.is_empty()) && knowledge_path.is_none() =>
        {
            Ok(())
        }
        CoordinationNodeKind::KnowledgeStore
            if knowledge_path.is_some_and(|path| !path.is_empty()) && workstream_cwd.is_none() =>
        {
            Ok(())
        }
        _ => Err(ProjectStoreError::CoordinationNodeManagedPathInvalid),
    }
}

fn same_f64(left: f64, right: f64) -> bool {
    left.to_bits() == right.to_bits()
}

fn reject_reused_command(
    connection: &Connection,
    command_id: &str,
) -> Result<(), ProjectStoreError> {
    if command_id_exists(connection, command_id)? {
        Err(ProjectStoreError::IdempotencyConflict)
    } else {
        Ok(())
    }
}

fn node_exists(connection: &Connection, node_id: &str) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM coordination_nodes WHERE id = ?1)",
            [node_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn node_kind_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<CoordinationNodeKind> {
    match row.get::<_, String>(index)?.as_str() {
        "workstream" => Ok(CoordinationNodeKind::Workstream),
        "knowledge_store" => Ok(CoordinationNodeKind::KnowledgeStore),
        value => Err(super::enum_conversion_error(
            index,
            "coordination node kind",
            value,
        )),
    }
}

const fn node_kind_value(kind: CoordinationNodeKind) -> &'static str {
    match kind {
        CoordinationNodeKind::Workstream => "workstream",
        CoordinationNodeKind::KnowledgeStore => "knowledge_store",
    }
}

pub(super) fn canonical_node_id(value: &str) -> Result<String, ProjectStoreError> {
    canonical_coordination_uuid("node_id", value).map_err(Into::into)
}

fn canonical_snapshot_id(value: &str) -> Result<String, ProjectStoreError> {
    canonical_coordination_uuid("snapshot_id", value).map_err(Into::into)
}

fn canonical_project_id(value: &str) -> Result<String, ProjectStoreError> {
    canonical_coordination_uuid("project_id", value).map_err(Into::into)
}

fn project_ids_key(project_ids: &[String]) -> String {
    project_ids.join(",")
}

fn required_failure_message(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProjectStoreError::CommandFailureMessageRequired)
    } else {
        Ok(value.to_owned())
    }
}

struct NodeBase {
    id: String,
    name: String,
    kind: CoordinationNodeKind,
    worker_id: Option<String>,
    cwd: Option<String>,
    folder_path: Option<String>,
    version: u64,
    created_by: String,
    created_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    placement: CoordinationNodePlacement,
}

struct PromptRecord {
    command_id: String,
    node_id: String,
    worker_id: String,
    expected_node_version: u64,
    text: String,
    actor: String,
    status: String,
    error_message: Option<String>,
    runtime_status: Option<String>,
    submitted_at_unix_ms: Option<u64>,
}

impl PromptRecord {
    fn matches(&self, node_id: &str, command: &SendCoordinationNodePrompt) -> bool {
        self.node_id == node_id
            && self.worker_id == command.worker_id
            && self.expected_node_version == command.expected_node_version
            && self.text == command.text
            && self.actor == command.actor
    }

    fn acknowledgement(self) -> Result<CoordinationNodePromptAcknowledgement, ProjectStoreError> {
        Ok(CoordinationNodePromptAcknowledgement {
            command_id: self.command_id,
            node_id: self.node_id,
            worker_id: self.worker_id,
            runtime_status: self
                .runtime_status
                .ok_or(ProjectStoreError::PromptAcknowledgementNotFound)?,
            submitted_at_unix_ms: self
                .submitted_at_unix_ms
                .ok_or(ProjectStoreError::PromptAcknowledgementNotFound)?,
        })
    }
}

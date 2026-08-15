use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use yard_domain::{
    Automation, AutomationCommandResult, AutomationPlacement, AutomationRun,
    AutomationRunCommandResult, AutomationRunStatus, AutomationRunTrigger, AutomationRuns,
    AutomationScope, AutomationState, Automations, CreateAutomation, DailySchedule,
    RunAutomationNow, SetAutomationPaused, UpdateAutomation, UpdateAutomationPlacement,
    canonical_automation_uuid,
};

use super::{
    ProjectStoreError, SqliteProjectStore, command_id_exists, insert_lifecycle_event,
    project_exists, required_runtime_status, row_optional_u64, row_u64, to_i64, unix_time_ms,
};

const MAX_AUTOMATION_LIST_LIMIT: usize = 500;
const MAX_ID_BYTES: usize = 120;
const MAX_ACTOR_BYTES: usize = 120;

pub(super) async fn list_automations(
    store: &SqliteProjectStore,
) -> Result<Automations, ProjectStoreError> {
    store
        .run(|connection| {
            let mut statement = connection.prepare(
                "SELECT id
                   FROM automations
                  ORDER BY created_at_unix_ms, id",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let automations = ids
                .into_iter()
                .map(|id| select_automation(connection, &id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Automations { automations })
        })
        .await
}

pub(super) async fn get_automation(
    store: &SqliteProjectStore,
    automation_id: &str,
) -> Result<Automation, ProjectStoreError> {
    let automation_id = canonical_id("automation_id", automation_id)?;
    store
        .run(move |connection| select_automation(connection, &automation_id))
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn create_automation(
    store: &SqliteProjectStore,
    command: CreateAutomation,
    next_run_at_unix_ms: u64,
) -> Result<AutomationCommandResult, ProjectStoreError> {
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_create_command(&transaction, &command.command_id)? {
                if !existing.matches(&command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(AutomationCommandResult {
                    command_id: command.command_id,
                    automation: select_automation(&transaction, &existing.automation_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            if automation_exists(&transaction, &command.automation_id)? {
                return Err(ProjectStoreError::AutomationIdConflict);
            }
            validate_scope_and_projects(
                &transaction,
                &command.scope,
                &command.selected_project_ids,
            )?;

            let now = unix_time_ms()?;
            let (scope_kind, scope_project_id, scope_node_id) = scope_columns(&command.scope);
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'automation_create', ?2, 'succeeded', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "INSERT INTO automations (
                    id, name, scope_kind, scope_project_id, scope_node_id,
                    schedule_hour, schedule_minute, schedule_timezone,
                    prompt_template, state, next_run_at_unix_ms, version,
                    created_by, created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    'active', ?10, 1, ?11, ?12, ?12
                 )",
                params![
                    command.automation_id,
                    command.name,
                    scope_kind,
                    scope_project_id,
                    scope_node_id,
                    i64::from(command.schedule.hour),
                    i64::from(command.schedule.minute),
                    command.schedule.timezone,
                    command.prompt_template,
                    to_i64(next_run_at_unix_ms)?,
                    command.actor,
                    to_i64(now)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO automation_placements (
                    automation_id, canvas_x, canvas_y, canvas_width,
                    canvas_height, version, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![
                    command.automation_id,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                    to_i64(now)?,
                ],
            )?;
            insert_selected_projects(
                &transaction,
                "automation_selected_projects",
                "automation_id",
                &command.automation_id,
                &command.selected_project_ids,
            )?;
            transaction.execute(
                "INSERT INTO automation_create_commands (
                    command_id, automation_id, name, scope_kind,
                    scope_project_id, scope_node_id, schedule_hour,
                    schedule_minute, schedule_timezone, prompt_template,
                    next_run_at_unix_ms, canvas_x, canvas_y, canvas_width,
                    canvas_height, result_version
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15, 1
                 )",
                params![
                    command.command_id,
                    command.automation_id,
                    command.name,
                    scope_kind,
                    scope_project_id,
                    scope_node_id,
                    i64::from(command.schedule.hour),
                    i64::from(command.schedule.minute),
                    command.schedule.timezone,
                    command.prompt_template,
                    to_i64(next_run_at_unix_ms)?,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                ],
            )?;
            insert_selected_projects(
                &transaction,
                "automation_create_command_projects",
                "command_id",
                &command.command_id,
                &command.selected_project_ids,
            )?;
            insert_lifecycle_event(
                &transaction,
                "automation",
                &command.automation_id,
                1,
                "automation_created",
                &command.actor,
                now,
            )?;
            let automation = select_automation(&transaction, &command.automation_id)?;
            transaction.commit()?;
            Ok(AutomationCommandResult {
                command_id: command.command_id,
                automation,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn update_automation(
    store: &SqliteProjectStore,
    command: UpdateAutomation,
    next_run_at_unix_ms: Option<u64>,
) -> Result<AutomationCommandResult, ProjectStoreError> {
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = select_update_command(&transaction, &command.command_id)? {
                if !existing.matches(&command) {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(AutomationCommandResult {
                    command_id: command.command_id,
                    automation: select_automation(&transaction, &command.automation_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let current = select_automation(&transaction, &command.automation_id)?;
            if current.version != command.expected_version {
                return Err(ProjectStoreError::AutomationVersionConflict {
                    current_version: current.version,
                });
            }
            validate_next_run(current.state, next_run_at_unix_ms)?;
            validate_scope_and_projects(
                &transaction,
                &command.scope,
                &command.selected_project_ids,
            )?;
            let next_version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            let (scope_kind, scope_project_id, scope_node_id) = scope_columns(&command.scope);
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'automation_update', ?2, 'succeeded', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE automations
                    SET name = ?1, scope_kind = ?2, scope_project_id = ?3,
                        scope_node_id = ?4, schedule_hour = ?5,
                        schedule_minute = ?6, schedule_timezone = ?7,
                        prompt_template = ?8, next_run_at_unix_ms = ?9,
                        version = ?10, updated_at_unix_ms = ?11
                  WHERE id = ?12 AND version = ?13",
                params![
                    command.name,
                    scope_kind,
                    scope_project_id,
                    scope_node_id,
                    i64::from(command.schedule.hour),
                    i64::from(command.schedule.minute),
                    command.schedule.timezone,
                    command.prompt_template,
                    optional_i64(next_run_at_unix_ms)?,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    command.automation_id,
                    to_i64(current.version)?,
                ],
            )?;
            replace_automation_projects(
                &transaction,
                &command.automation_id,
                &command.selected_project_ids,
            )?;
            transaction.execute(
                "INSERT INTO automation_update_commands (
                    command_id, automation_id, expected_version, name,
                    scope_kind, scope_project_id, scope_node_id,
                    schedule_hour, schedule_minute, schedule_timezone,
                    prompt_template, requested_next_run_at_unix_ms,
                    result_version
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13
                 )",
                params![
                    command.command_id,
                    command.automation_id,
                    to_i64(command.expected_version)?,
                    command.name,
                    scope_kind,
                    scope_project_id,
                    scope_node_id,
                    i64::from(command.schedule.hour),
                    i64::from(command.schedule.minute),
                    command.schedule.timezone,
                    command.prompt_template,
                    optional_i64(next_run_at_unix_ms)?,
                    to_i64(next_version)?,
                ],
            )?;
            insert_selected_projects(
                &transaction,
                "automation_update_command_projects",
                "command_id",
                &command.command_id,
                &command.selected_project_ids,
            )?;
            insert_lifecycle_event(
                &transaction,
                "automation",
                &command.automation_id,
                next_version,
                "automation_updated",
                &command.actor,
                now,
            )?;
            let automation = select_automation(&transaction, &command.automation_id)?;
            transaction.commit()?;
            Ok(AutomationCommandResult {
                command_id: command.command_id,
                automation,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn update_automation_placement(
    store: &SqliteProjectStore,
    command: UpdateAutomationPlacement,
) -> Result<AutomationCommandResult, ProjectStoreError> {
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT apc.automation_id, apc.expected_version,
                            apc.canvas_x, apc.canvas_y, apc.canvas_width,
                            apc.canvas_height, ca.actor
                       FROM automation_placement_commands apc
                       JOIN command_acknowledgements ca
                         ON ca.id = apc.command_id
                      WHERE apc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row_u64(row, 1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((automation_id, version, x, y, width, height, actor)) = existing {
                if automation_id != command.automation_id
                    || version != command.expected_version
                    || !same_f64(x, command.placement.x)
                    || !same_f64(y, command.placement.y)
                    || !same_f64(width, command.placement.width)
                    || !same_f64(height, command.placement.height)
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(AutomationCommandResult {
                    command_id: command.command_id,
                    automation: select_automation(&transaction, &command.automation_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let current = select_automation(&transaction, &command.automation_id)?;
            if current.placement.version != command.expected_version {
                return Err(ProjectStoreError::AutomationPlacementVersionConflict {
                    current_version: current.placement.version,
                });
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
                    ?1, 'automation_placement', ?2, 'succeeded', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE automation_placements
                    SET canvas_x = ?1, canvas_y = ?2, canvas_width = ?3,
                        canvas_height = ?4, version = ?5,
                        updated_at_unix_ms = ?6
                  WHERE automation_id = ?7 AND version = ?8",
                params![
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    command.automation_id,
                    to_i64(current.placement.version)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO automation_placement_commands (
                    command_id, automation_id, expected_version, canvas_x,
                    canvas_y, canvas_width, canvas_height, result_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    command.command_id,
                    command.automation_id,
                    to_i64(command.expected_version)?,
                    command.placement.x,
                    command.placement.y,
                    command.placement.width,
                    command.placement.height,
                    to_i64(next_version)?,
                ],
            )?;
            let automation = select_automation(&transaction, &command.automation_id)?;
            transaction.commit()?;
            Ok(AutomationCommandResult {
                command_id: command.command_id,
                automation,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn set_automation_paused(
    store: &SqliteProjectStore,
    command: SetAutomationPaused,
    next_run_at_unix_ms: Option<u64>,
) -> Result<AutomationCommandResult, ProjectStoreError> {
    let command = command.normalize()?;
    if command.paused != next_run_at_unix_ms.is_none() {
        return Err(ProjectStoreError::AutomationNextRunInvalid);
    }
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT apc.automation_id, apc.expected_version,
                            apc.paused, apc.requested_next_run_at_unix_ms,
                            ca.actor
                       FROM automation_pause_commands apc
                       JOIN command_acknowledgements ca
                         ON ca.id = apc.command_id
                      WHERE apc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row_u64(row, 1)?,
                            row.get::<_, bool>(2)?,
                            row_optional_u64(row, 3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((automation_id, version, paused, _next_run, actor)) = existing {
                if automation_id != command.automation_id
                    || version != command.expected_version
                    || paused != command.paused
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(AutomationCommandResult {
                    command_id: command.command_id,
                    automation: select_automation(&transaction, &command.automation_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let current = select_automation(&transaction, &command.automation_id)?;
            if current.version != command.expected_version {
                return Err(ProjectStoreError::AutomationVersionConflict {
                    current_version: current.version,
                });
            }
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
                    ?1, 'automation_pause', ?2, 'succeeded', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            transaction.execute(
                "UPDATE automations
                    SET state = ?1, next_run_at_unix_ms = ?2,
                        version = ?3, updated_at_unix_ms = ?4
                  WHERE id = ?5 AND version = ?6",
                params![
                    if command.paused { "paused" } else { "active" },
                    optional_i64(next_run_at_unix_ms)?,
                    to_i64(next_version)?,
                    to_i64(now)?,
                    command.automation_id,
                    to_i64(current.version)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO automation_pause_commands (
                    command_id, automation_id, expected_version, paused,
                    requested_next_run_at_unix_ms, result_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    command.command_id,
                    command.automation_id,
                    to_i64(command.expected_version)?,
                    command.paused,
                    optional_i64(next_run_at_unix_ms)?,
                    to_i64(next_version)?,
                ],
            )?;
            insert_lifecycle_event(
                &transaction,
                "automation",
                &command.automation_id,
                next_version,
                if command.paused {
                    "automation_paused"
                } else {
                    "automation_resumed"
                },
                &command.actor,
                now,
            )?;
            let automation = select_automation(&transaction, &command.automation_id)?;
            transaction.commit()?;
            Ok(AutomationCommandResult {
                command_id: command.command_id,
                automation,
                replayed: false,
            })
        })
        .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn create_manual_automation_run(
    store: &SqliteProjectStore,
    command: RunAutomationNow,
    run_id: &str,
    dispatch_command_id: &str,
) -> Result<AutomationRunCommandResult, ProjectStoreError> {
    let command = command.normalize()?;
    let run_id = canonical_id("run_id", run_id)?;
    let dispatch_command_id =
        required_text("dispatch_command_id", dispatch_command_id, MAX_ID_BYTES)?;
    if dispatch_command_id == command.command_id {
        return Err(ProjectStoreError::AutomationDispatchCommandIdConflict);
    }
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT arnc.automation_id, arnc.expected_version,
                            arnc.run_id, ca.actor
                       FROM automation_run_now_commands arnc
                       JOIN command_acknowledgements ca
                         ON ca.id = arnc.command_id
                      WHERE arnc.command_id = ?1",
                    [&command.command_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row_u64(row, 1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((automation_id, expected_version, existing_run_id, actor)) = existing {
                if automation_id != command.automation_id
                    || expected_version != command.expected_version
                    || actor != command.actor
                {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(AutomationRunCommandResult {
                    command_id: command.command_id,
                    run: select_run(&transaction, &command.automation_id, &existing_run_id)?,
                    replayed: true,
                });
            }
            reject_reused_command(&transaction, &command.command_id)?;
            let automation = select_automation(&transaction, &command.automation_id)?;
            if automation.version != command.expected_version {
                return Err(ProjectStoreError::AutomationVersionConflict {
                    current_version: automation.version,
                });
            }
            validate_new_run_identity(&transaction, &run_id, &dispatch_command_id)?;
            reject_pending_run(&transaction, &command.automation_id)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "INSERT INTO command_acknowledgements (
                    id, command_type, actor, status, error_message,
                    created_at_unix_ms, updated_at_unix_ms
                 ) VALUES (
                    ?1, 'automation_run_now', ?2, 'succeeded', NULL, ?3, ?3
                 )",
                params![command.command_id, command.actor, to_i64(now)?],
            )?;
            insert_run(
                &transaction,
                &automation,
                &NewRun {
                    id: &run_id,
                    dispatch_command_id: &dispatch_command_id,
                    trigger: AutomationRunTrigger::Manual,
                    requested_by: &command.actor,
                    scheduled_for_unix_ms: None,
                    next_due_after_claim_unix_ms: None,
                },
                now,
            )?;
            transaction.execute(
                "INSERT INTO automation_run_now_commands (
                    command_id, automation_id, expected_version, run_id
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    command.command_id,
                    command.automation_id,
                    to_i64(command.expected_version)?,
                    run_id,
                ],
            )?;
            let run = select_run(&transaction, &command.automation_id, &run_id)?;
            transaction.commit()?;
            Ok(AutomationRunCommandResult {
                command_id: command.command_id,
                run,
                replayed: false,
            })
        })
        .await
}

pub(super) async fn list_due_automations(
    store: &SqliteProjectStore,
    now_unix_ms: u64,
    limit: usize,
) -> Result<Automations, ProjectStoreError> {
    validate_limit(limit)?;
    store
        .run(move |connection| {
            let mut statement = connection.prepare(
                "SELECT a.id
                   FROM automations a
                  WHERE a.state = 'active'
                    AND a.next_run_at_unix_ms <= ?1
                    AND NOT EXISTS (
                        SELECT 1
                          FROM automation_runs ar
                         WHERE ar.automation_id = a.id
                           AND ar.status = 'pending'
                    )
                  ORDER BY a.next_run_at_unix_ms, a.id
                  LIMIT ?2",
            )?;
            let ids = statement
                .query_map(
                    params![to_i64(now_unix_ms)?, i64::try_from(limit)?],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let automations = ids
                .into_iter()
                .map(|id| select_automation(connection, &id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Automations { automations })
        })
        .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn claim_scheduled_automation_run(
    store: &SqliteProjectStore,
    automation_id: &str,
    expected_due_unix_ms: u64,
    next_due_unix_ms: u64,
    run_id: &str,
    dispatch_command_id: &str,
    requested_by: &str,
) -> Result<AutomationRun, ProjectStoreError> {
    let automation_id = canonical_id("automation_id", automation_id)?;
    let run_id = canonical_id("run_id", run_id)?;
    let dispatch_command_id =
        required_text("dispatch_command_id", dispatch_command_id, MAX_ID_BYTES)?;
    let requested_by = required_text("requested_by", requested_by, MAX_ACTOR_BYTES)?;
    if next_due_unix_ms <= expected_due_unix_ms {
        return Err(ProjectStoreError::AutomationNextRunInvalid);
    }
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    "SELECT id
                       FROM automation_runs
                      WHERE automation_id = ?1
                        AND trigger = 'scheduled'
                        AND scheduled_for_unix_ms = ?2",
                    params![automation_id, to_i64(expected_due_unix_ms)?],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(existing_run_id) = existing {
                let run = select_run(&transaction, &automation_id, &existing_run_id)?;
                if run.requested_by != requested_by {
                    return Err(ProjectStoreError::IdempotencyConflict);
                }
                return Ok(run);
            }
            let automation = select_automation(&transaction, &automation_id)?;
            if automation.state != AutomationState::Active {
                return Err(ProjectStoreError::AutomationPaused);
            }
            if automation.next_run_at_unix_ms != Some(expected_due_unix_ms) {
                return Err(ProjectStoreError::AutomationScheduleConflict {
                    current_next_run_at: automation.next_run_at_unix_ms,
                });
            }
            validate_new_run_identity(&transaction, &run_id, &dispatch_command_id)?;
            reject_pending_run(&transaction, &automation_id)?;
            let next_automation_version = automation
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE automations
                    SET next_run_at_unix_ms = ?1, version = ?2,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND state = 'active'
                    AND next_run_at_unix_ms = ?5 AND version = ?6",
                params![
                    to_i64(next_due_unix_ms)?,
                    to_i64(next_automation_version)?,
                    to_i64(now)?,
                    automation_id,
                    to_i64(expected_due_unix_ms)?,
                    to_i64(automation.version)?,
                ],
            )?;
            insert_run(
                &transaction,
                &automation,
                &NewRun {
                    id: &run_id,
                    dispatch_command_id: &dispatch_command_id,
                    trigger: AutomationRunTrigger::Scheduled,
                    requested_by: &requested_by,
                    scheduled_for_unix_ms: Some(expected_due_unix_ms),
                    next_due_after_claim_unix_ms: Some(next_due_unix_ms),
                },
                now,
            )?;
            let run = select_run(&transaction, &automation_id, &run_id)?;
            transaction.commit()?;
            Ok(run)
        })
        .await
}

pub(super) async fn list_pending_automation_runs(
    store: &SqliteProjectStore,
    limit: usize,
) -> Result<AutomationRuns, ProjectStoreError> {
    validate_limit(limit)?;
    store
        .run(move |connection| {
            let mut statement = connection.prepare(
                "SELECT ar.automation_id, ar.id
                   FROM automation_runs ar
                  WHERE ar.status = 'pending'
                    AND NOT EXISTS (
                        SELECT 1
                          FROM command_acknowledgements ca
                         WHERE ca.id = ar.dispatch_command_id
                    )
                  ORDER BY ar.created_at_unix_ms, ar.id
                  LIMIT ?1",
            )?;
            let ids = statement
                .query_map([i64::try_from(limit)?], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let runs = ids
                .into_iter()
                .map(|(automation_id, run_id)| select_run(connection, &automation_id, &run_id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AutomationRuns { runs })
        })
        .await
}

pub(super) async fn mark_automation_run_submitted(
    store: &SqliteProjectStore,
    run_id: &str,
    runtime_status: &str,
    submitted_at_unix_ms: u64,
) -> Result<AutomationRun, ProjectStoreError> {
    let run_id = canonical_id("run_id", run_id)?;
    let runtime_status = required_runtime_status(runtime_status)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = select_run_by_id(&transaction, &run_id)?;
            if current.status == AutomationRunStatus::Submitted {
                if current.runtime_status.as_deref() == Some(&runtime_status)
                    && current.submitted_at_unix_ms == Some(submitted_at_unix_ms)
                {
                    return Ok(current);
                }
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            if current.status != AutomationRunStatus::Pending {
                return Err(ProjectStoreError::CommandNotPending);
            }
            if submitted_at_unix_ms < current.created_at_unix_ms {
                return Err(ProjectStoreError::AutomationSubmittedAtInvalid);
            }
            validate_dispatch_acknowledgement(
                &transaction,
                &current.dispatch_command_id,
                "succeeded",
            )?;
            let next_version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            transaction.execute(
                "UPDATE automation_runs
                    SET status = 'submitted', version = ?1,
                        runtime_status = ?2, submitted_at_unix_ms = ?3,
                        updated_at_unix_ms = ?3
                  WHERE id = ?4 AND status = 'pending' AND version = ?5",
                params![
                    to_i64(next_version)?,
                    runtime_status,
                    to_i64(submitted_at_unix_ms)?,
                    run_id,
                    to_i64(current.version)?,
                ],
            )?;
            let run = select_run_by_id(&transaction, &run_id)?;
            transaction.commit()?;
            Ok(run)
        })
        .await
}

pub(super) async fn mark_automation_run_failed(
    store: &SqliteProjectStore,
    run_id: &str,
    message: &str,
    ambiguous: bool,
) -> Result<AutomationRun, ProjectStoreError> {
    let run_id = canonical_id("run_id", run_id)?;
    let message = required_failure_message(message)?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = select_run_by_id(&transaction, &run_id)?;
            let target = if ambiguous {
                AutomationRunStatus::Ambiguous
            } else {
                AutomationRunStatus::Failed
            };
            if current.status == target {
                if current.error_message.as_deref() == Some(&message) {
                    return Ok(current);
                }
                return Err(ProjectStoreError::IdempotencyConflict);
            }
            if current.status != AutomationRunStatus::Pending {
                return Err(ProjectStoreError::CommandNotPending);
            }
            validate_optional_failure_acknowledgement(
                &transaction,
                &current.dispatch_command_id,
                if ambiguous { "ambiguous" } else { "failed" },
            )?;
            let next_version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            transaction.execute(
                "UPDATE automation_runs
                    SET status = ?1, version = ?2, error_message = ?3,
                        updated_at_unix_ms = ?4
                  WHERE id = ?5 AND status = 'pending' AND version = ?6",
                params![
                    if ambiguous { "ambiguous" } else { "failed" },
                    to_i64(next_version)?,
                    message,
                    to_i64(now)?,
                    run_id,
                    to_i64(current.version)?,
                ],
            )?;
            let run = select_run_by_id(&transaction, &run_id)?;
            transaction.commit()?;
            Ok(run)
        })
        .await
}

pub(super) async fn list_automation_runs(
    store: &SqliteProjectStore,
    automation_id: &str,
    limit: usize,
) -> Result<AutomationRuns, ProjectStoreError> {
    let automation_id = canonical_id("automation_id", automation_id)?;
    validate_limit(limit)?;
    store
        .run(move |connection| {
            if !automation_exists(connection, &automation_id)? {
                return Err(ProjectStoreError::AutomationNotFound);
            }
            let mut statement = connection.prepare(
                "SELECT id
                   FROM automation_runs
                  WHERE automation_id = ?1
                  ORDER BY created_at_unix_ms DESC, id DESC
                  LIMIT ?2",
            )?;
            let ids = statement
                .query_map(params![automation_id, i64::try_from(limit)?], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let runs = ids
                .into_iter()
                .map(|run_id| select_run(connection, &automation_id, &run_id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AutomationRuns { runs })
        })
        .await
}

pub(super) async fn get_automation_run(
    store: &SqliteProjectStore,
    automation_id: &str,
    run_id: &str,
) -> Result<AutomationRun, ProjectStoreError> {
    let automation_id = canonical_id("automation_id", automation_id)?;
    let run_id = canonical_id("run_id", run_id)?;
    store
        .run(move |connection| select_run(connection, &automation_id, &run_id))
        .await
}

fn select_automation(
    connection: &Connection,
    automation_id: &str,
) -> Result<Automation, ProjectStoreError> {
    let mut automation = connection
        .query_row(
            "SELECT a.id, a.name, a.scope_kind, a.scope_project_id,
                    a.scope_node_id, a.schedule_hour, a.schedule_minute,
                    a.schedule_timezone, a.prompt_template, a.state,
                    a.next_run_at_unix_ms, a.version, a.created_by,
                    a.created_at_unix_ms, a.updated_at_unix_ms,
                    ap.canvas_x, ap.canvas_y, ap.canvas_width,
                    ap.canvas_height, ap.version, ap.updated_at_unix_ms
               FROM automations a
               JOIN automation_placements ap ON ap.automation_id = a.id
              WHERE a.id = ?1",
            [automation_id],
            |row| {
                Ok(Automation {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    scope: scope_from_row(row, 2)?,
                    schedule: DailySchedule {
                        hour: row_u8(row, 5)?,
                        minute: row_u8(row, 6)?,
                        timezone: row.get(7)?,
                    },
                    prompt_template: row.get(8)?,
                    state: automation_state_from_row(row, 9)?,
                    next_run_at_unix_ms: row_optional_u64(row, 10)?,
                    version: row_u64(row, 11)?,
                    created_by: row.get(12)?,
                    created_at_unix_ms: row_u64(row, 13)?,
                    updated_at_unix_ms: row_u64(row, 14)?,
                    placement: AutomationPlacement {
                        geometry: yard_domain::CanvasPlacement {
                            x: row.get(15)?,
                            y: row.get(16)?,
                            width: row.get(17)?,
                            height: row.get(18)?,
                        },
                        version: row_u64(row, 19)?,
                        updated_at_unix_ms: row_u64(row, 20)?,
                    },
                    selected_project_ids: Vec::new(),
                    latest_run: None,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::AutomationNotFound)?;
    automation.selected_project_ids = select_project_ids(
        connection,
        "automation_selected_projects",
        "automation_id",
        automation_id,
    )?;
    automation.latest_run = select_latest_run(connection, automation_id)?;
    Ok(automation)
}

fn select_latest_run(
    connection: &Connection,
    automation_id: &str,
) -> Result<Option<AutomationRun>, ProjectStoreError> {
    let run_id = connection
        .query_row(
            "SELECT id
               FROM automation_runs
              WHERE automation_id = ?1
              ORDER BY created_at_unix_ms DESC, id DESC
              LIMIT 1",
            [automation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    run_id
        .map(|run_id| select_run(connection, automation_id, &run_id))
        .transpose()
}

fn select_run(
    connection: &Connection,
    automation_id: &str,
    run_id: &str,
) -> Result<AutomationRun, ProjectStoreError> {
    let mut run = connection
        .query_row(
            "SELECT id, automation_id, scope_kind, scope_project_id,
                    scope_node_id, automation_version, version, trigger,
                    status, prompt_template, dispatch_command_id,
                    requested_by, scheduled_for_unix_ms, runtime_status,
                    error_message, submitted_at_unix_ms,
                    created_at_unix_ms, updated_at_unix_ms
               FROM automation_runs
              WHERE automation_id = ?1 AND id = ?2",
            params![automation_id, run_id],
            run_from_row,
        )
        .optional()?
        .ok_or(ProjectStoreError::AutomationRunNotFound)?;
    run.selected_project_ids =
        select_project_ids(connection, "automation_run_projects", "run_id", run_id)?;
    Ok(run)
}

fn select_run_by_id(
    connection: &Connection,
    run_id: &str,
) -> Result<AutomationRun, ProjectStoreError> {
    let automation_id = connection
        .query_row(
            "SELECT automation_id FROM automation_runs WHERE id = ?1",
            [run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(ProjectStoreError::AutomationRunNotFound)?;
    select_run(connection, &automation_id, run_id)
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<AutomationRun> {
    Ok(AutomationRun {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        scope_snapshot: scope_from_row(row, 2)?,
        automation_version: row_u64(row, 5)?,
        version: row_u64(row, 6)?,
        trigger: match row.get::<_, String>(7)?.as_str() {
            "manual" => AutomationRunTrigger::Manual,
            "scheduled" => AutomationRunTrigger::Scheduled,
            value => return Err(super::enum_conversion_error(7, "automation trigger", value)),
        },
        status: automation_run_status_from_row(row, 8)?,
        prompt_template: row.get(9)?,
        selected_project_ids: Vec::new(),
        dispatch_command_id: row.get(10)?,
        requested_by: row.get(11)?,
        scheduled_for_unix_ms: row_optional_u64(row, 12)?,
        runtime_status: row.get(13)?,
        error_message: row.get(14)?,
        submitted_at_unix_ms: row_optional_u64(row, 15)?,
        created_at_unix_ms: row_u64(row, 16)?,
        updated_at_unix_ms: row_u64(row, 17)?,
    })
}

struct NewRun<'a> {
    id: &'a str,
    dispatch_command_id: &'a str,
    trigger: AutomationRunTrigger,
    requested_by: &'a str,
    scheduled_for_unix_ms: Option<u64>,
    next_due_after_claim_unix_ms: Option<u64>,
}

fn insert_run(
    transaction: &Transaction<'_>,
    automation: &Automation,
    run: &NewRun<'_>,
    now: u64,
) -> Result<(), ProjectStoreError> {
    let (scope_kind, scope_project_id, scope_node_id) = scope_columns(&automation.scope);
    transaction.execute(
        "INSERT INTO automation_runs (
            id, automation_id, scope_kind, scope_project_id, scope_node_id,
            automation_version, version, trigger, status, prompt_template,
            dispatch_command_id, requested_by, scheduled_for_unix_ms,
            next_due_after_claim_unix_ms, runtime_status, error_message,
            submitted_at_unix_ms, created_at_unix_ms, updated_at_unix_ms
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 'pending', ?8, ?9,
            ?10, ?11, ?12, NULL, NULL, NULL, ?13, ?13
        )",
        params![
            run.id,
            automation.id,
            scope_kind,
            scope_project_id,
            scope_node_id,
            to_i64(automation.version)?,
            trigger_value(run.trigger),
            automation.prompt_template,
            run.dispatch_command_id,
            run.requested_by,
            optional_i64(run.scheduled_for_unix_ms)?,
            optional_i64(run.next_due_after_claim_unix_ms)?,
            to_i64(now)?,
        ],
    )?;
    insert_selected_projects(
        transaction,
        "automation_run_projects",
        "run_id",
        run.id,
        &automation.selected_project_ids,
    )
}

fn validate_scope_and_projects(
    connection: &Connection,
    scope: &AutomationScope,
    selected_project_ids: &[String],
) -> Result<(), ProjectStoreError> {
    match scope {
        AutomationScope::YardOrchestrator => {}
        AutomationScope::ProjectOrchestrator { project_id } => {
            if !project_exists(connection, project_id)? {
                return Err(ProjectStoreError::AutomationScopeProjectNotFound);
            }
        }
        AutomationScope::WorkstreamCoordinationNode { node_id } => {
            let kind = connection
                .query_row(
                    "SELECT kind FROM coordination_nodes WHERE id = ?1",
                    [node_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or(ProjectStoreError::AutomationScopeNodeNotFound)?;
            if kind != "workstream" {
                return Err(ProjectStoreError::AutomationScopeNodeKindMismatch);
            }
        }
    }
    for project_id in selected_project_ids {
        if !project_exists(connection, project_id)? {
            return Err(ProjectStoreError::AutomationSelectedProjectNotFound {
                project_id: project_id.clone(),
            });
        }
        if let AutomationScope::WorkstreamCoordinationNode { node_id } = scope {
            let attached = connection.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM coordination_node_projects
                     WHERE node_id = ?1 AND project_id = ?2
                 )",
                params![node_id, project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !attached {
                return Err(ProjectStoreError::AutomationProjectNotAttached);
            }
        }
    }
    Ok(())
}

fn validate_next_run(
    state: AutomationState,
    next_run_at_unix_ms: Option<u64>,
) -> Result<(), ProjectStoreError> {
    if (state == AutomationState::Active) == next_run_at_unix_ms.is_some() {
        Ok(())
    } else {
        Err(ProjectStoreError::AutomationNextRunInvalid)
    }
}

fn validate_new_run_identity(
    connection: &Connection,
    run_id: &str,
    dispatch_command_id: &str,
) -> Result<(), ProjectStoreError> {
    let run_exists = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM automation_runs WHERE id = ?1)",
        [run_id],
        |row| row.get::<_, bool>(0),
    )?;
    if run_exists {
        return Err(ProjectStoreError::AutomationRunIdConflict);
    }
    let dispatch_exists = connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM automation_runs WHERE dispatch_command_id = ?1
            UNION ALL
            SELECT 1 FROM command_acknowledgements WHERE id = ?1
         )",
        [dispatch_command_id],
        |row| row.get::<_, bool>(0),
    )?;
    if dispatch_exists {
        return Err(ProjectStoreError::AutomationDispatchCommandIdConflict);
    }
    Ok(())
}

fn reject_pending_run(
    connection: &Connection,
    automation_id: &str,
) -> Result<(), ProjectStoreError> {
    let pending = connection.query_row(
        "SELECT EXISTS (
            SELECT 1
              FROM automation_runs
             WHERE automation_id = ?1 AND status = 'pending'
         )",
        [automation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if pending {
        Err(ProjectStoreError::AutomationRunInProgress)
    } else {
        Ok(())
    }
}

fn validate_dispatch_acknowledgement(
    connection: &Connection,
    dispatch_command_id: &str,
    expected_status: &str,
) -> Result<(), ProjectStoreError> {
    let acknowledgement = connection
        .query_row(
            "SELECT status, error_message
               FROM command_acknowledgements
              WHERE id = ?1",
            [dispatch_command_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .ok_or(ProjectStoreError::CommandNotFound)?;
    if acknowledgement.0 == expected_status {
        return Ok(());
    }
    match acknowledgement.0.as_str() {
        "pending" => Err(ProjectStoreError::CommandInProgress),
        "failed" => Err(ProjectStoreError::CommandPreviouslyFailed(
            acknowledgement.1.unwrap_or_default(),
        )),
        "ambiguous" => Err(ProjectStoreError::CommandOutcomeAmbiguous(
            acknowledgement.1.unwrap_or_default(),
        )),
        _ => Err(ProjectStoreError::CommandNotPending),
    }
}

fn validate_optional_failure_acknowledgement(
    connection: &Connection,
    dispatch_command_id: &str,
    expected_status: &str,
) -> Result<(), ProjectStoreError> {
    let status = connection
        .query_row(
            "SELECT status FROM command_acknowledgements WHERE id = ?1",
            [dispatch_command_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if status
        .as_deref()
        .is_none_or(|status| status == expected_status)
    {
        Ok(())
    } else {
        Err(ProjectStoreError::CommandNotPending)
    }
}

fn replace_automation_projects(
    transaction: &Transaction<'_>,
    automation_id: &str,
    project_ids: &[String],
) -> Result<(), ProjectStoreError> {
    transaction.execute(
        "DELETE FROM automation_selected_projects WHERE automation_id = ?1",
        [automation_id],
    )?;
    insert_selected_projects(
        transaction,
        "automation_selected_projects",
        "automation_id",
        automation_id,
        project_ids,
    )
}

fn insert_selected_projects(
    transaction: &Transaction<'_>,
    table: &'static str,
    key_column: &'static str,
    key: &str,
    project_ids: &[String],
) -> Result<(), ProjectStoreError> {
    let sql = format!("INSERT INTO {table} ({key_column}, project_id) VALUES (?1, ?2)");
    for project_id in project_ids {
        transaction.execute(&sql, params![key, project_id])?;
    }
    Ok(())
}

fn select_project_ids(
    connection: &Connection,
    table: &'static str,
    key_column: &'static str,
    key: &str,
) -> Result<Vec<String>, ProjectStoreError> {
    let sql = format!("SELECT project_id FROM {table} WHERE {key_column} = ?1 ORDER BY project_id");
    let mut statement = connection.prepare(&sql)?;
    statement
        .query_map([key], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn select_create_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<CreateCommandRecord>, ProjectStoreError> {
    let record = connection
        .query_row(
            "SELECT acc.automation_id, acc.name, acc.scope_kind,
                    acc.scope_project_id, acc.scope_node_id,
                    acc.schedule_hour, acc.schedule_minute,
                    acc.schedule_timezone, acc.prompt_template,
                    acc.canvas_x, acc.canvas_y,
                    acc.canvas_width, acc.canvas_height, ca.actor
               FROM automation_create_commands acc
               JOIN command_acknowledgements ca ON ca.id = acc.command_id
              WHERE acc.command_id = ?1",
            [command_id],
            |row| {
                Ok(CreateCommandRecord {
                    automation_id: row.get(0)?,
                    name: row.get(1)?,
                    scope: scope_from_row(row, 2)?,
                    schedule: DailySchedule {
                        hour: row_u8(row, 5)?,
                        minute: row_u8(row, 6)?,
                        timezone: row.get(7)?,
                    },
                    prompt_template: row.get(8)?,
                    placement: yard_domain::CanvasPlacement {
                        x: row.get(9)?,
                        y: row.get(10)?,
                        width: row.get(11)?,
                        height: row.get(12)?,
                    },
                    actor: row.get(13)?,
                    selected_project_ids: Vec::new(),
                })
            },
        )
        .optional()?;
    record
        .map(|mut record| {
            record.selected_project_ids = select_project_ids(
                connection,
                "automation_create_command_projects",
                "command_id",
                command_id,
            )?;
            Ok(record)
        })
        .transpose()
}

fn select_update_command(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<UpdateCommandRecord>, ProjectStoreError> {
    let record = connection
        .query_row(
            "SELECT auc.automation_id, auc.expected_version, auc.name,
                    auc.scope_kind, auc.scope_project_id, auc.scope_node_id,
                    auc.schedule_hour, auc.schedule_minute,
                    auc.schedule_timezone, auc.prompt_template,
                    ca.actor
               FROM automation_update_commands auc
               JOIN command_acknowledgements ca ON ca.id = auc.command_id
              WHERE auc.command_id = ?1",
            [command_id],
            |row| {
                Ok(UpdateCommandRecord {
                    automation_id: row.get(0)?,
                    expected_version: row_u64(row, 1)?,
                    name: row.get(2)?,
                    scope: scope_from_row(row, 3)?,
                    schedule: DailySchedule {
                        hour: row_u8(row, 6)?,
                        minute: row_u8(row, 7)?,
                        timezone: row.get(8)?,
                    },
                    prompt_template: row.get(9)?,
                    actor: row.get(10)?,
                    selected_project_ids: Vec::new(),
                })
            },
        )
        .optional()?;
    record
        .map(|mut record| {
            record.selected_project_ids = select_project_ids(
                connection,
                "automation_update_command_projects",
                "command_id",
                command_id,
            )?;
            Ok(record)
        })
        .transpose()
}

struct CreateCommandRecord {
    automation_id: String,
    name: String,
    scope: AutomationScope,
    schedule: DailySchedule,
    prompt_template: String,
    placement: yard_domain::CanvasPlacement,
    actor: String,
    selected_project_ids: Vec<String>,
}

impl CreateCommandRecord {
    fn matches(&self, command: &CreateAutomation) -> bool {
        self.name == command.name
            && self.scope == command.scope
            && self.schedule == command.schedule
            && self.prompt_template == command.prompt_template
            && same_placement(&self.placement, &command.placement)
            && self.actor == command.actor
            && self.selected_project_ids == command.selected_project_ids
    }
}

struct UpdateCommandRecord {
    automation_id: String,
    expected_version: u64,
    name: String,
    scope: AutomationScope,
    schedule: DailySchedule,
    prompt_template: String,
    actor: String,
    selected_project_ids: Vec<String>,
}

impl UpdateCommandRecord {
    fn matches(&self, command: &UpdateAutomation) -> bool {
        self.automation_id == command.automation_id
            && self.expected_version == command.expected_version
            && self.name == command.name
            && self.scope == command.scope
            && self.schedule == command.schedule
            && self.prompt_template == command.prompt_template
            && self.actor == command.actor
            && self.selected_project_ids == command.selected_project_ids
    }
}

fn automation_exists(
    connection: &Connection,
    automation_id: &str,
) -> Result<bool, ProjectStoreError> {
    connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM automations WHERE id = ?1)",
            [automation_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
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

fn scope_columns(scope: &AutomationScope) -> (&'static str, Option<&str>, Option<&str>) {
    match scope {
        AutomationScope::YardOrchestrator => ("yard_orchestrator", None, None),
        AutomationScope::ProjectOrchestrator { project_id } => {
            ("project_orchestrator", Some(project_id), None)
        }
        AutomationScope::WorkstreamCoordinationNode { node_id } => {
            ("workstream_coordination_node", None, Some(node_id))
        }
    }
}

fn scope_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<AutomationScope> {
    let kind = row.get::<_, String>(index)?;
    let project_id = row.get::<_, Option<String>>(index + 1)?;
    let node_id = row.get::<_, Option<String>>(index + 2)?;
    match (kind.as_str(), project_id, node_id) {
        ("yard_orchestrator", None, None) => Ok(AutomationScope::YardOrchestrator),
        ("project_orchestrator", Some(project_id), None) => {
            Ok(AutomationScope::ProjectOrchestrator { project_id })
        }
        ("workstream_coordination_node", None, Some(node_id)) => {
            Ok(AutomationScope::WorkstreamCoordinationNode { node_id })
        }
        (value, _, _) => Err(super::enum_conversion_error(
            index,
            "automation scope",
            value,
        )),
    }
}

fn automation_state_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<AutomationState> {
    match row.get::<_, String>(index)?.as_str() {
        "active" => Ok(AutomationState::Active),
        "paused" => Ok(AutomationState::Paused),
        value => Err(super::enum_conversion_error(
            index,
            "automation state",
            value,
        )),
    }
}

fn automation_run_status_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<AutomationRunStatus> {
    match row.get::<_, String>(index)?.as_str() {
        "pending" => Ok(AutomationRunStatus::Pending),
        "submitted" => Ok(AutomationRunStatus::Submitted),
        "failed" => Ok(AutomationRunStatus::Failed),
        "ambiguous" => Ok(AutomationRunStatus::Ambiguous),
        value => Err(super::enum_conversion_error(
            index,
            "automation run status",
            value,
        )),
    }
}

const fn trigger_value(trigger: AutomationRunTrigger) -> &'static str {
    match trigger {
        AutomationRunTrigger::Manual => "manual",
        AutomationRunTrigger::Scheduled => "scheduled",
    }
}

fn validate_limit(limit: usize) -> Result<(), ProjectStoreError> {
    if (1..=MAX_AUTOMATION_LIST_LIMIT).contains(&limit) {
        Ok(())
    } else {
        Err(ProjectStoreError::AutomationListLimitInvalid {
            max: MAX_AUTOMATION_LIST_LIMIT,
        })
    }
}

fn canonical_id(field: &'static str, value: &str) -> Result<String, ProjectStoreError> {
    canonical_automation_uuid(field, value).map_err(Into::into)
}

fn required_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(yard_domain::AutomationValidationError::Required { field }.into());
    }
    if value.len() > max_bytes {
        return Err(yard_domain::AutomationValidationError::TooLong { field, max_bytes }.into());
    }
    Ok(value.to_owned())
}

fn required_failure_message(value: &str) -> Result<String, ProjectStoreError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProjectStoreError::CommandFailureMessageRequired)
    } else {
        Ok(value.to_owned())
    }
}

fn row_u8(row: &Row<'_>, index: usize) -> rusqlite::Result<u8> {
    let value = row.get::<_, i64>(index)?;
    u8::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn optional_i64(value: Option<u64>) -> Result<Option<i64>, ProjectStoreError> {
    value.map(to_i64).transpose()
}

fn same_f64(left: f64, right: f64) -> bool {
    left.to_bits() == right.to_bits()
}

fn same_placement(
    left: &yard_domain::CanvasPlacement,
    right: &yard_domain::CanvasPlacement,
) -> bool {
    same_f64(left.x, right.x)
        && same_f64(left.y, right.y)
        && same_f64(left.width, right.width)
        && same_f64(left.height, right.height)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use uuid::Uuid;
    use yard_domain::{
        AutomationRunStatus, AutomationScope, AutomationState, CanvasPlacement,
        CoordinationNodeKind, CreateAutomation, CreateCoordinationNode, CreateProject,
        DailySchedule, ObservedStatus, ProjectRuntimeBinding, RunAutomationNow,
        RuntimeObservationState, RuntimeProcessState, SetAutomationPaused, UpdateAutomation,
        UpdateAutomationPlacement, WorkerRuntimeBinding,
    };

    use super::{ProjectStoreError, SqliteProjectStore, to_i64};
    use crate::YardStore;

    fn project_draft(
        workspace_id: &str,
        terminal_id: &str,
    ) -> (CreateProject, WorkerRuntimeBinding) {
        (
            CreateProject {
                name: "Automation project".to_owned(),
                runtime: ProjectRuntimeBinding {
                    adapter: "herdr".to_owned(),
                    session: "default".to_owned(),
                    workspace_id: workspace_id.to_owned(),
                },
                orchestrator_observed_worker_id: terminal_id.to_owned(),
                placement: CanvasPlacement {
                    x: 10.0,
                    y: 20.0,
                    width: 322.0,
                    height: 240.0,
                },
            },
            WorkerRuntimeBinding {
                adapter: "herdr".to_owned(),
                session: "default".to_owned(),
                workspace_id: workspace_id.to_owned(),
                terminal_id: terminal_id.to_owned(),
                tab_id: Some(format!("tab-{terminal_id}")),
                pane_id: format!("pane-{terminal_id}"),
                provider_session: None,
                owns_tab: false,
                observation_state: RuntimeObservationState::Observed,
                process_state: RuntimeProcessState::Running,
                status: ObservedStatus::Idle,
                state_change_sequence: 1,
                revision: 1,
                version: 1,
                last_observed_at_unix_ms: 1,
            },
        )
    }

    fn placement() -> CanvasPlacement {
        CanvasPlacement {
            x: 100.0,
            y: 120.0,
            width: 240.0,
            height: 140.0,
        }
    }

    fn create_command(
        command_id: &str,
        automation_id: &str,
        scope: AutomationScope,
        selected_project_ids: Vec<String>,
    ) -> CreateAutomation {
        CreateAutomation {
            command_id: command_id.to_owned(),
            actor: "local-user".to_owned(),
            automation_id: automation_id.to_owned(),
            name: "Daily status".to_owned(),
            scope,
            placement: placement(),
            schedule: DailySchedule {
                hour: 9,
                minute: 30,
                timezone: "America/Los_Angeles".to_owned(),
            },
            selected_project_ids,
            prompt_template: "Summarize current status and blockers.".to_owned(),
        }
    }

    async fn open_store(temp: &TempDir) -> SqliteProjectStore {
        SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
            .await
            .unwrap()
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn automation_commands_are_idempotent_versioned_and_pauseable() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (draft, runtime) = project_draft("automation-project", "automation-terminal");
        let project = store.create_project(draft, runtime).await.unwrap();
        let automation_id = Uuid::now_v7().to_string();
        let create = create_command(
            "create-automation",
            &automation_id,
            AutomationScope::YardOrchestrator,
            vec![project.id.clone()],
        );
        let created = store.create_automation(create.clone(), 100).await.unwrap();
        assert_eq!(created.automation.name, "Daily status");
        assert_eq!(created.automation.version, 1);
        assert_eq!(created.automation.placement.version, 1);
        let replayed_create = store
            .create_automation(
                CreateAutomation {
                    automation_id: Uuid::now_v7().to_string(),
                    ..create.clone()
                },
                101,
            )
            .await
            .unwrap();
        assert!(replayed_create.replayed);
        assert_eq!(replayed_create.automation.id, automation_id);
        assert_eq!(replayed_create.automation.next_run_at_unix_ms, Some(100));
        assert!(matches!(
            store
                .create_automation(
                    CreateAutomation {
                        name: "Different client input".to_owned(),
                        ..create
                    },
                    100,
                )
                .await
                .unwrap_err(),
            ProjectStoreError::IdempotencyConflict
        ));

        let update = UpdateAutomation {
            command_id: "update-automation".to_owned(),
            actor: "local-user".to_owned(),
            automation_id: automation_id.clone(),
            expected_version: created.automation.version,
            name: "Morning status".to_owned(),
            scope: AutomationScope::YardOrchestrator,
            schedule: DailySchedule {
                hour: 10,
                minute: 15,
                timezone: "UTC".to_owned(),
            },
            selected_project_ids: vec![project.id],
            prompt_template: "Summarize status.".to_owned(),
        };
        let updated = store
            .update_automation(update.clone(), Some(150))
            .await
            .unwrap();
        let replayed_update = store.update_automation(update, Some(151)).await.unwrap();
        assert!(replayed_update.replayed);
        assert_eq!(replayed_update.automation.next_run_at_unix_ms, Some(150));

        let moved = store
            .update_automation_placement(UpdateAutomationPlacement {
                command_id: "move-automation".to_owned(),
                actor: "local-user".to_owned(),
                automation_id: automation_id.clone(),
                expected_version: 1,
                placement: CanvasPlacement {
                    x: 500.0,
                    ..placement()
                },
            })
            .await
            .unwrap();
        assert_eq!(moved.automation.version, updated.automation.version);
        assert_eq!(moved.automation.placement.version, 2);

        let paused = store
            .set_automation_paused(
                SetAutomationPaused {
                    command_id: "pause-automation".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    expected_version: updated.automation.version,
                    paused: true,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paused.automation.state, AutomationState::Paused);
        assert_eq!(paused.automation.next_run_at_unix_ms, None);

        let run_id = Uuid::now_v7().to_string();
        let manual = RunAutomationNow {
            command_id: "run-paused-automation".to_owned(),
            actor: "local-user".to_owned(),
            automation_id: automation_id.clone(),
            expected_version: paused.automation.version,
        };
        let claimed = store
            .create_manual_automation_run(manual.clone(), &run_id, "dispatch-paused")
            .await
            .unwrap();
        assert_eq!(claimed.run.status, AutomationRunStatus::Pending);
        assert_eq!(claimed.run.automation_version, paused.automation.version);
        assert!({
            let replayed = store
                .create_manual_automation_run(
                    manual,
                    &Uuid::now_v7().to_string(),
                    "new-generated-dispatch",
                )
                .await
                .unwrap();
            assert_eq!(replayed.run.id, run_id);
            assert_eq!(replayed.run.dispatch_command_id, "dispatch-paused");
            replayed.replayed
        });
        store
            .mark_automation_run_failed(&run_id, "runtime unavailable", false)
            .await
            .unwrap();

        let resume = SetAutomationPaused {
            command_id: "resume-automation".to_owned(),
            actor: "local-user".to_owned(),
            automation_id: automation_id.clone(),
            expected_version: paused.automation.version,
            paused: false,
        };
        let resumed = store
            .set_automation_paused(resume.clone(), Some(100))
            .await
            .unwrap();
        assert_eq!(resumed.automation.state, AutomationState::Active);
        assert_eq!(resumed.automation.next_run_at_unix_ms, Some(100));
        let replayed_resume = store
            .set_automation_paused(resume, Some(101))
            .await
            .unwrap();
        assert!(replayed_resume.replayed);
        assert_eq!(replayed_resume.automation.next_run_at_unix_ms, Some(100));
        assert_eq!(
            store
                .list_due_automations(99, 10)
                .await
                .unwrap()
                .automations
                .len(),
            0
        );
        assert_eq!(
            store
                .list_due_automations(100, 10)
                .await
                .unwrap()
                .automations
                .len(),
            1
        );

        let scheduled_run_id = Uuid::now_v7().to_string();
        let scheduled = store
            .claim_scheduled_automation_run(
                &automation_id,
                100,
                200,
                &scheduled_run_id,
                "dispatch-scheduled",
                "scheduler",
            )
            .await
            .unwrap();
        assert_eq!(scheduled.scheduled_for_unix_ms, Some(100));
        assert_eq!(scheduled.automation_version, resumed.automation.version);
        assert_eq!(
            store
                .get_automation(&automation_id)
                .await
                .unwrap()
                .next_run_at_unix_ms,
            Some(200)
        );
        assert_eq!(
            store
                .claim_scheduled_automation_run(
                    &automation_id,
                    100,
                    201,
                    &Uuid::now_v7().to_string(),
                    "new-generated-scheduled-dispatch",
                    "scheduler",
                )
                .await
                .unwrap()
                .id,
            scheduled_run_id
        );
    }

    #[tokio::test]
    async fn automation_scope_enforces_workstream_attachments_and_kind() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let (draft_a, runtime_a) = project_draft("scope-a", "terminal-a");
        let project_a = store.create_project(draft_a, runtime_a).await.unwrap();
        let (draft_b, runtime_b) = project_draft("scope-b", "terminal-b");
        let project_b = store.create_project(draft_b, runtime_b).await.unwrap();
        let node_id = Uuid::now_v7().to_string();
        store
            .create_coordination_node(
                &node_id,
                Some(temp.path().join(&node_id).to_string_lossy().into_owned()),
                None,
                CreateCoordinationNode {
                    command_id: "create-workstream".to_owned(),
                    actor: "local-user".to_owned(),
                    name: "Workstream".to_owned(),
                    kind: CoordinationNodeKind::Workstream,
                    placement: placement(),
                    attached_project_ids: vec![project_a.id.clone()],
                },
            )
            .await
            .unwrap();

        let error = store
            .create_automation(
                create_command(
                    "invalid-workstream-automation",
                    &Uuid::now_v7().to_string(),
                    AutomationScope::WorkstreamCoordinationNode {
                        node_id: node_id.clone(),
                    },
                    vec![project_b.id],
                ),
                100,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ProjectStoreError::AutomationProjectNotAttached
        ));

        let knowledge_id = Uuid::now_v7().to_string();
        store
            .create_coordination_node(
                &knowledge_id,
                None,
                Some(
                    temp.path()
                        .join(&knowledge_id)
                        .to_string_lossy()
                        .into_owned(),
                ),
                CreateCoordinationNode {
                    command_id: "create-knowledge".to_owned(),
                    actor: "local-user".to_owned(),
                    name: "Knowledge".to_owned(),
                    kind: CoordinationNodeKind::KnowledgeStore,
                    placement: placement(),
                    attached_project_ids: vec![project_a.id],
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .create_automation(
                    create_command(
                        "invalid-knowledge-automation",
                        &Uuid::now_v7().to_string(),
                        AutomationScope::WorkstreamCoordinationNode {
                            node_id: knowledge_id,
                        },
                        Vec::new(),
                    ),
                    100,
                )
                .await
                .unwrap_err(),
            ProjectStoreError::AutomationScopeNodeKindMismatch
        ));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn dispatch_acknowledgements_are_prompt_owned_and_recovered() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let automation_id = Uuid::now_v7().to_string();
        let created = store
            .create_automation(
                create_command(
                    "create-recovery-automation",
                    &automation_id,
                    AutomationScope::YardOrchestrator,
                    Vec::new(),
                ),
                100,
            )
            .await
            .unwrap()
            .automation;
        let submitted_run_id = Uuid::now_v7().to_string();
        let submitted = store
            .create_manual_automation_run(
                RunAutomationNow {
                    command_id: "claim-submitted".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    expected_version: created.version,
                },
                &submitted_run_id,
                "dispatch-submitted",
            )
            .await
            .unwrap()
            .run;
        let submitted_at_unix_ms = submitted.created_at_unix_ms + 1;
        store
            .run(move |connection| {
                connection.execute(
                    "INSERT INTO command_acknowledgements (
                        id, command_type, actor, status, error_message,
                        created_at_unix_ms, updated_at_unix_ms
                     ) VALUES (
                        'dispatch-submitted', 'yard_orchestrator_prompt',
                        'scheduler', 'succeeded', NULL, ?1, ?1
                     )",
                    [to_i64(submitted_at_unix_ms)?],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let submitted = store
            .mark_automation_run_submitted(&submitted_run_id, "accepted", submitted_at_unix_ms)
            .await
            .unwrap();
        assert_eq!(submitted.status, AutomationRunStatus::Submitted);
        assert_eq!(submitted.version, 2);

        let ambiguous_run_id = Uuid::now_v7().to_string();
        let current_version = store.get_automation(&automation_id).await.unwrap().version;
        let ambiguous = store
            .create_manual_automation_run(
                RunAutomationNow {
                    command_id: "claim-ambiguous".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    expected_version: current_version,
                },
                &ambiguous_run_id,
                "dispatch-ambiguous",
            )
            .await
            .unwrap()
            .run;
        store
            .run(move |connection| {
                connection.execute(
                    "INSERT INTO command_acknowledgements (
                        id, command_type, actor, status, error_message,
                        created_at_unix_ms, updated_at_unix_ms
                     ) VALUES (
                        'dispatch-ambiguous', 'yard_orchestrator_prompt',
                        'scheduler', 'pending', NULL, ?1, ?1
                     )",
                    [to_i64(ambiguous.created_at_unix_ms + 1)?],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        let recovered = reopened
            .get_automation_run(&automation_id, &ambiguous_run_id)
            .await
            .unwrap();
        assert_eq!(recovered.status, AutomationRunStatus::Ambiguous);
        assert_eq!(recovered.version, 2);

        let unstarted_run_id = Uuid::now_v7().to_string();
        let current_version = reopened
            .get_automation(&automation_id)
            .await
            .unwrap()
            .version;
        reopened
            .create_manual_automation_run(
                RunAutomationNow {
                    command_id: "claim-unstarted".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    expected_version: current_version,
                },
                &unstarted_run_id,
                "dispatch-unstarted",
            )
            .await
            .unwrap();
        drop(reopened);

        let reopened = open_store(&temp).await;
        let pending = reopened.list_pending_automation_runs(10).await.unwrap();
        assert_eq!(pending.runs.len(), 1);
        assert_eq!(pending.runs[0].id, unstarted_run_id);
        assert_eq!(
            reopened
                .get_automation_run(&automation_id, &unstarted_run_id)
                .await
                .unwrap()
                .status,
            AutomationRunStatus::Pending
        );
    }

    #[tokio::test]
    async fn automation_resources_and_runs_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let store = open_store(&temp).await;
        let automation_id = Uuid::now_v7().to_string();
        let automation = store
            .create_automation(
                create_command(
                    "create-persisted-automation",
                    &automation_id,
                    AutomationScope::YardOrchestrator,
                    Vec::new(),
                ),
                100,
            )
            .await
            .unwrap()
            .automation;
        let run_id = Uuid::now_v7().to_string();
        store
            .create_manual_automation_run(
                RunAutomationNow {
                    command_id: "claim-persisted-run".to_owned(),
                    actor: "local-user".to_owned(),
                    automation_id: automation_id.clone(),
                    expected_version: automation.version,
                },
                &run_id,
                "dispatch-persisted",
            )
            .await
            .unwrap();
        drop(store);

        let reopened = open_store(&temp).await;
        assert_eq!(
            reopened
                .get_automation(&automation_id)
                .await
                .unwrap()
                .latest_run
                .unwrap()
                .id,
            run_id
        );
        drop(reopened);
        let connection = rusqlite::Connection::open(temp.path().join("yard.sqlite3")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 18);
    }
}

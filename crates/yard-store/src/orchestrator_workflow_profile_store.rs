use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use yard_domain::{
    FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS, FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS,
    OrchestratorWorkflowAdapterContextFile, OrchestratorWorkflowCommand,
    OrchestratorWorkflowProfile, OrchestratorWorkflowProfileSource, OrchestratorWorkflowProfiles,
    ResetOrchestratorWorkflowProfile, UpdateOrchestratorWorkflowProfile,
    YARD_STANDARD_ORCHESTRATOR_PROFILE_DESCRIPTION, YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
    YARD_STANDARD_ORCHESTRATOR_PROFILE_NAME, validate_orchestrator_workflow_commands,
    validate_stored_orchestrator_workflow_commands, yard_standard_orchestrator_commands,
};

use super::{
    ProjectStoreError, SqliteProjectStore, insert_lifecycle_event,
    reject_project_orchestrator_intervention, to_i64, unix_time_ms,
};

const COMPLETE_MATRIX_MIGRATION_ACTOR: &str = "yard:v26-complete-command-matrix";

pub(super) async fn get_current(
    store: &SqliteProjectStore,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    get_current_by_id(store, YARD_STANDARD_ORCHESTRATOR_PROFILE_ID).await
}

pub(super) async fn list(
    store: &SqliteProjectStore,
) -> Result<OrchestratorWorkflowProfiles, ProjectStoreError> {
    store
        .run(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, current_version
                   FROM orchestrator_workflow_profiles
                  ORDER BY id",
            )?;
            let identities = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, super::row_u64(row, 1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let profiles = identities
                .into_iter()
                .map(|(profile_id, version)| select_revision(connection, &profile_id, version))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(OrchestratorWorkflowProfiles { profiles })
        })
        .await
}

pub(super) async fn get_current_by_id(
    store: &SqliteProjectStore,
    profile_id: &str,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let profile_id = profile_id.trim().to_owned();
    if profile_id.is_empty() {
        return Err(ProjectStoreError::OrchestratorWorkflowProfileNotFound);
    }
    store
        .run(move |connection| {
            let version = select_current_version(connection, &profile_id)?;
            select_revision(connection, &profile_id, version)
        })
        .await
}

pub(super) async fn get_revision(
    store: &SqliteProjectStore,
    profile_id: &str,
    version: u64,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let profile_id = profile_id.trim().to_owned();
    if profile_id.is_empty() || version == 0 {
        return Err(ProjectStoreError::OrchestratorWorkflowProfileNotFound);
    }
    store
        .run(move |connection| select_revision(connection, &profile_id, version))
        .await
}

pub(super) async fn update(
    store: &SqliteProjectStore,
    profile_id: &str,
    command: UpdateOrchestratorWorkflowProfile,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let command = command.normalize()?;
    let profile_id = profile_id.trim();
    if profile_id.is_empty() {
        return Err(ProjectStoreError::OrchestratorWorkflowProfileNotFound);
    }
    let current = get_current_by_id(store, profile_id).await?;
    append_revision(
        store,
        RevisionInput {
            profile_id: current.id,
            name: current.name,
            description: current.description,
            expected_version: command.expected_version,
            instructions_markdown: command.instructions_markdown,
            monitor_interval_ms: command.monitor_interval_ms,
            commands: command.commands.unwrap_or(current.commands),
            adapter_context_files: command
                .adapter_context_files
                .unwrap_or(current.adapter_context_files),
            source: OrchestratorWorkflowProfileSource::User,
            actor: command.actor,
        },
    )
    .await
}

pub(super) async fn reset(
    store: &SqliteProjectStore,
    profile_id: &str,
    command: ResetOrchestratorWorkflowProfile,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let command = command.normalize()?;
    if profile_id.trim() != YARD_STANDARD_ORCHESTRATOR_PROFILE_ID {
        return Err(ProjectStoreError::OrchestratorWorkflowProfileNotFound);
    }
    append_revision(
        store,
        RevisionInput {
            profile_id: YARD_STANDARD_ORCHESTRATOR_PROFILE_ID.to_owned(),
            name: YARD_STANDARD_ORCHESTRATOR_PROFILE_NAME.to_owned(),
            description: YARD_STANDARD_ORCHESTRATOR_PROFILE_DESCRIPTION.to_owned(),
            expected_version: command.expected_version,
            instructions_markdown: FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS.to_owned(),
            monitor_interval_ms: FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS,
            commands: yard_standard_orchestrator_commands(),
            adapter_context_files: Vec::new(),
            source: OrchestratorWorkflowProfileSource::Reset,
            actor: command.actor,
        },
    )
    .await
}

struct RevisionInput {
    profile_id: String,
    name: String,
    description: String,
    expected_version: u64,
    instructions_markdown: String,
    monitor_interval_ms: u64,
    commands: Vec<OrchestratorWorkflowCommand>,
    adapter_context_files: Vec<OrchestratorWorkflowAdapterContextFile>,
    source: OrchestratorWorkflowProfileSource,
    actor: String,
}

async fn append_revision(
    store: &SqliteProjectStore,
    input: RevisionInput,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_version = select_current_version(&transaction, &input.profile_id)?;
            if current_version != input.expected_version {
                return Err(
                    ProjectStoreError::OrchestratorWorkflowProfileVersionConflict {
                        current_version,
                    },
                );
            }
            let next_version = current_version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            insert_revision(&transaction, &input, next_version, now)?;
            let changed = transaction.execute(
                "UPDATE orchestrator_workflow_profiles
                    SET current_version = ?1, updated_at_unix_ms = ?2
                  WHERE id = ?3 AND current_version = ?4",
                params![
                    to_i64(next_version)?,
                    to_i64(now)?,
                    input.profile_id,
                    to_i64(current_version)?,
                ],
            )?;
            if changed != 1 {
                return Err(
                    ProjectStoreError::OrchestratorWorkflowProfileVersionConflict {
                        current_version: select_current_version(&transaction, &input.profile_id)?,
                    },
                );
            }
            if input.profile_id == YARD_STANDARD_ORCHESTRATOR_PROFILE_ID {
                transaction.execute(
                    "UPDATE orchestrator_workflow_profile_current
                        SET current_version = ?1, updated_at_unix_ms = ?2
                      WHERE singleton_id = 1
                        AND profile_id = ?3",
                    params![
                        to_i64(next_version)?,
                        to_i64(now)?,
                        YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                    ],
                )?;
            }
            let saved = select_revision(&transaction, &input.profile_id, next_version)?;
            transaction.commit()?;
            Ok(saved)
        })
        .await
}

pub(super) fn seed_factory_profile(connection: &Connection) -> Result<(), ProjectStoreError> {
    let exists = connection
        .query_row(
            "SELECT current_version
               FROM orchestrator_workflow_profile_current
              WHERE singleton_id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if exists {
        return Ok(());
    }

    connection.execute(
        "INSERT INTO orchestrator_workflow_profile_revisions (
            version, instructions_markdown, monitor_interval_ms,
            source, updated_by, created_at_unix_ms
         ) VALUES (1, ?1, ?2, 'factory', 'yard:factory', 0)",
        params![
            FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS,
            to_i64(FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS)?,
        ],
    )?;
    connection.execute(
        "INSERT INTO orchestrator_workflow_profile_current (
            singleton_id, current_version, updated_at_unix_ms
         ) VALUES (1, 1, 0)",
        [],
    )?;
    connection.execute(
        "UPDATE yard_orchestrator
            SET workflow_profile_version = 1
          WHERE workflow_profile_version IS NULL",
        [],
    )?;
    connection.execute(
        "UPDATE yard_orchestrator_configure_commands
            SET workflow_profile_version = 1
          WHERE workflow_profile_version IS NULL",
        [],
    )?;
    Ok(())
}

pub(super) fn upgrade_provider_neutral_profile(
    connection: &Connection,
    preserve_existing_revisions: bool,
) -> Result<(), ProjectStoreError> {
    let commands = serde_json::to_string(&yard_standard_orchestrator_commands())?;
    let context_files =
        serde_json::to_string(&Vec::<OrchestratorWorkflowAdapterContextFile>::new())?;
    if !preserve_existing_revisions {
        connection.execute(
            "UPDATE orchestrator_workflow_profile_revisions
                SET name = ?1, description = ?2, commands_json = ?3,
                    adapter_context_files_json = ?4
              WHERE profile_id = ?5",
            params![
                YARD_STANDARD_ORCHESTRATOR_PROFILE_NAME,
                YARD_STANDARD_ORCHESTRATOR_PROFILE_DESCRIPTION,
                commands,
                context_files,
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
            ],
        )?;
        return Ok(());
    }

    let current_version =
        select_current_version(connection, YARD_STANDARD_ORCHESTRATOR_PROFILE_ID)?;
    let next_version = current_version
        .checked_add(1)
        .ok_or(ProjectStoreError::VersionOverflow)?;
    let now = unix_time_ms()?;
    let input = RevisionInput {
        profile_id: YARD_STANDARD_ORCHESTRATOR_PROFILE_ID.to_owned(),
        name: YARD_STANDARD_ORCHESTRATOR_PROFILE_NAME.to_owned(),
        description: YARD_STANDARD_ORCHESTRATOR_PROFILE_DESCRIPTION.to_owned(),
        expected_version: current_version,
        instructions_markdown: FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS.to_owned(),
        monitor_interval_ms: FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS,
        commands: yard_standard_orchestrator_commands(),
        adapter_context_files: Vec::new(),
        source: OrchestratorWorkflowProfileSource::Factory,
        actor: "yard:v26-migration".to_owned(),
    };
    insert_revision(connection, &input, next_version, now)?;
    connection.execute(
        "UPDATE orchestrator_workflow_profiles
            SET current_version = ?1, updated_at_unix_ms = ?2
          WHERE id = ?3 AND current_version = ?4",
        params![
            to_i64(next_version)?,
            to_i64(now)?,
            YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
            to_i64(current_version)?,
        ],
    )?;
    connection.execute(
        "UPDATE orchestrator_workflow_profile_current
            SET current_version = ?1, updated_at_unix_ms = ?2
          WHERE singleton_id = 1 AND current_version = ?3",
        params![
            to_i64(next_version)?,
            to_i64(now)?,
            to_i64(current_version)?,
        ],
    )?;
    Ok(())
}

pub(super) fn activate_complete_provider_neutral_profile(
    connection: &Transaction<'_>,
) -> Result<(), ProjectStoreError> {
    let canonical_commands = yard_standard_orchestrator_commands();
    let sources = collect_executable_revision_sources(connection)?;
    let mut incomplete_sources = BTreeSet::new();
    for (profile_id, version) in &sources {
        let revision = select_revision(connection, profile_id, *version)?;
        if revision.commands != canonical_commands {
            incomplete_sources.insert((profile_id.clone(), *version));
        }
    }

    let affected_projects = preflight_migration_project_repins(connection, &incomplete_sources)?;
    ensure_completed_yard_configure_history(connection)?;

    let now = unix_time_ms()?;
    let mut successor_versions = BTreeMap::new();
    for (profile_id, source_version) in &incomplete_sources {
        let source = select_revision(connection, profile_id, *source_version)?;
        let next_version = connection.query_row(
            "SELECT MAX(version) + 1
               FROM orchestrator_workflow_profile_revisions
              WHERE profile_id = ?1",
            [profile_id],
            |row| super::row_u64(row, 0),
        )?;
        let input = RevisionInput {
            profile_id: source.id,
            name: source.name,
            description: source.description,
            expected_version: *source_version,
            instructions_markdown: source.instructions_markdown,
            monitor_interval_ms: source.monitor_interval_ms,
            commands: canonical_commands.clone(),
            adapter_context_files: source.adapter_context_files,
            source: source.source,
            actor: COMPLETE_MATRIX_MIGRATION_ACTOR.to_owned(),
        };
        insert_revision(connection, &input, next_version, now)?;
        successor_versions.insert((profile_id.clone(), *source_version), next_version);
    }

    repin_executable_references(connection, &successor_versions, &affected_projects, now)?;
    validate_executable_revision_sources(connection)?;
    Ok(())
}

fn collect_executable_revision_sources(
    connection: &Connection,
) -> Result<BTreeSet<(String, u64)>, ProjectStoreError> {
    let mut statement = connection.prepare(
        "SELECT id, current_version
           FROM orchestrator_workflow_profiles
         UNION
         SELECT profile_id, profile_version
           FROM project_workflow_profile_pins
         UNION
         SELECT command.workflow_profile_id, command.workflow_profile_version
           FROM profile_project_creation_commands command
           JOIN command_acknowledgements acknowledgement
             ON acknowledgement.id = command.command_id
          WHERE acknowledgement.status = 'pending'
         UNION
         SELECT command.workflow_profile_id, command.workflow_profile_version
           FROM workspace_project_creation_commands command
           JOIN command_acknowledgements acknowledgement
             ON acknowledgement.id = command.command_id
          WHERE acknowledgement.status = 'pending'
         UNION
         SELECT profile_id, current_version
           FROM orchestrator_workflow_profile_current
          WHERE singleton_id = 1
         UNION
         SELECT workflow_profile_id, workflow_profile_version
           FROM yard_orchestrator
          WHERE singleton_id = 1
         ORDER BY 1, 2",
    )?;
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, super::row_u64(row, 1)?))
        })?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(Into::into)
}

#[derive(Debug)]
struct MigrationProjectRepin {
    project_id: String,
    source: (String, u64),
    current_project_version: u64,
    next_project_version: u64,
}

fn preflight_migration_project_repins(
    connection: &Connection,
    incomplete_sources: &BTreeSet<(String, u64)>,
) -> Result<Vec<MigrationProjectRepin>, ProjectStoreError> {
    let mut affected = Vec::new();
    for (profile_id, profile_version) in incomplete_sources {
        let mut statement = connection.prepare(
            "SELECT pin.project_id, project.version
               FROM project_workflow_profile_pins pin
               JOIN projects project ON project.id = pin.project_id
              WHERE pin.profile_id = ?1 AND pin.profile_version = ?2
              ORDER BY pin.project_id",
        )?;
        let projects = statement
            .query_map(params![profile_id, to_i64(*profile_version)?], |row| {
                Ok((row.get::<_, String>(0)?, super::row_u64(row, 1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (project_id, current_project_version) in projects {
            reject_project_orchestrator_intervention(connection, &project_id, None).map_err(
                |error| ProjectStoreError::InvalidSchemaV25Lineage {
                    detail: format!(
                        "project {project_id} cannot be repinned while intervention evidence is unfinished: {error}"
                    ),
                },
            )?;
            let recoverable_replacement = connection.query_row(
                "SELECT EXISTS (
                    SELECT 1
                      FROM project_orchestrator_replacement_commands replacement
                      JOIN command_acknowledgements acknowledgement
                        ON acknowledgement.id = replacement.command_id
                     WHERE replacement.project_id = ?1
                       AND acknowledgement.status = 'ambiguous'
                       AND (
                           EXISTS (
                               SELECT 1
                                 FROM orchestrator_replacement_runtime_bindings runtime
                                WHERE runtime.command_id = replacement.command_id
                                  AND runtime.binding_role IN (
                                      'replacement_prepared',
                                      'replacement_started'
                                  )
                                  AND (
                                      runtime.recovery_outcome IS NULL
                                      OR runtime.recovery_outcome IN (
                                          'conflicting_reused',
                                          'present_not_safely_retirable'
                                      )
                                  )
                           )
                           OR (
                               replacement.prepare_tab_label IS NOT NULL
                               AND NOT EXISTS (
                                   SELECT 1
                                     FROM orchestrator_replacement_runtime_bindings runtime
                                    WHERE runtime.command_id = replacement.command_id
                                      AND runtime.binding_role IN (
                                          'replacement_prepared',
                                          'replacement_started'
                                      )
                               )
                               AND (
                                   replacement.prepare_recovery_outcome IS NULL
                                   OR replacement.prepare_recovery_outcome IN (
                                       'conflicting_reused',
                                       'present_not_safely_retirable'
                                   )
                               )
                           )
                       )
                 )",
                [&project_id],
                |row| row.get::<_, bool>(0),
            )?;
            if recoverable_replacement {
                return Err(ProjectStoreError::InvalidSchemaV25Lineage {
                    detail: format!(
                        "project {project_id} has ambiguous replacement evidence that must be resolved before workflow migration"
                    ),
                });
            }
            affected.push(MigrationProjectRepin {
                project_id,
                source: (profile_id.clone(), *profile_version),
                current_project_version,
                next_project_version: current_project_version
                    .checked_add(1)
                    .ok_or(ProjectStoreError::VersionOverflow)?,
            });
        }
    }
    Ok(affected)
}

fn ensure_completed_yard_configure_history(
    connection: &Connection,
) -> Result<(), ProjectStoreError> {
    let invalid = connection.query_row(
        "SELECT EXISTS (
            SELECT 1
              FROM yard_orchestrator_configure_commands configure
              JOIN command_acknowledgements acknowledgement
                ON acknowledgement.id = configure.command_id
             WHERE acknowledgement.status <> 'succeeded'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if invalid {
        return Err(ProjectStoreError::InvalidSchemaV25Lineage {
            detail: "Yard configure replay history contains a non-completed command row".to_owned(),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn repin_executable_references(
    connection: &Transaction<'_>,
    successors: &BTreeMap<(String, u64), u64>,
    affected_projects: &[MigrationProjectRepin],
    now: u64,
) -> Result<(), ProjectStoreError> {
    for ((profile_id, source_version), successor_version) in successors {
        connection.execute(
            "UPDATE orchestrator_workflow_profiles
                SET current_version = ?1, updated_at_unix_ms = ?2
              WHERE id = ?3 AND current_version = ?4",
            params![
                to_i64(*successor_version)?,
                to_i64(now)?,
                profile_id,
                to_i64(*source_version)?,
            ],
        )?;
        if profile_id == YARD_STANDARD_ORCHESTRATOR_PROFILE_ID {
            connection.execute(
                "UPDATE orchestrator_workflow_profile_current
                    SET current_version = ?1, updated_at_unix_ms = ?2
                  WHERE singleton_id = 1
                    AND profile_id = ?3
                    AND current_version = ?4",
                params![
                    to_i64(*successor_version)?,
                    to_i64(now)?,
                    profile_id,
                    to_i64(*source_version)?,
                ],
            )?;
        }
        for table in [
            "profile_project_creation_commands",
            "workspace_project_creation_commands",
        ] {
            connection.execute(
                &format!(
                    "UPDATE {table}
                        SET workflow_profile_version = ?1
                      WHERE workflow_profile_id = ?2
                        AND workflow_profile_version = ?3
                        AND EXISTS (
                            SELECT 1
                              FROM command_acknowledgements acknowledgement
                             WHERE acknowledgement.id = {table}.command_id
                               AND acknowledgement.status = 'pending'
                        )"
                ),
                params![
                    to_i64(*successor_version)?,
                    profile_id,
                    to_i64(*source_version)?,
                ],
            )?;
        }
        if profile_id == YARD_STANDARD_ORCHESTRATOR_PROFILE_ID {
            connection.execute(
                "UPDATE yard_orchestrator
                    SET workflow_profile_version = ?1,
                        version = version + 1,
                        updated_at_unix_ms = ?2
                  WHERE singleton_id = 1
                    AND workflow_profile_id = ?3
                    AND workflow_profile_version = ?4",
                params![
                    to_i64(*successor_version)?,
                    to_i64(now)?,
                    profile_id,
                    to_i64(*source_version)?,
                ],
            )?;
        }
    }

    for project in affected_projects {
        let successor_version = successors.get(&project.source).ok_or_else(|| {
            ProjectStoreError::InvalidSchemaV25Lineage {
                detail: format!("project {} has no workflow successor", project.project_id),
            }
        })?;
        let pin_rows = connection.execute(
            "UPDATE project_workflow_profile_pins
                SET profile_version = ?1, pinned_by = ?2, pinned_at_unix_ms = ?3
              WHERE project_id = ?4 AND profile_id = ?5 AND profile_version = ?6",
            params![
                to_i64(*successor_version)?,
                COMPLETE_MATRIX_MIGRATION_ACTOR,
                to_i64(now)?,
                project.project_id,
                project.source.0,
                to_i64(project.source.1)?,
            ],
        )?;
        let project_rows = connection.execute(
            "UPDATE projects
                SET version = ?1, updated_at_unix_ms = ?2
              WHERE id = ?3 AND version = ?4",
            params![
                to_i64(project.next_project_version)?,
                to_i64(now)?,
                project.project_id,
                to_i64(project.current_project_version)?,
            ],
        )?;
        if pin_rows != 1 || project_rows != 1 {
            return Err(ProjectStoreError::InvalidSchemaV25Lineage {
                detail: format!(
                    "project {} changed during workflow migration",
                    project.project_id
                ),
            });
        }
        insert_lifecycle_event(
            connection,
            "project",
            &project.project_id,
            project.next_project_version,
            "workflow_profile_pinned",
            COMPLETE_MATRIX_MIGRATION_ACTOR,
            now,
        )?;
    }
    Ok(())
}

fn validate_executable_revision_sources(connection: &Connection) -> Result<(), ProjectStoreError> {
    for (profile_id, version) in collect_executable_revision_sources(connection)? {
        select_executable_revision(connection, &profile_id, version).map_err(|error| {
            ProjectStoreError::InvalidSchemaV25Lineage {
                detail: format!(
                    "live workflow reference {profile_id}@{version} is not executable: {error}"
                ),
            }
        })?;
    }
    Ok(())
}

pub(super) fn select_current_version(
    connection: &Connection,
    profile_id: &str,
) -> Result<u64, ProjectStoreError> {
    connection
        .query_row(
            "SELECT current_version
               FROM orchestrator_workflow_profiles
              WHERE id = ?1",
            [profile_id],
            |row| super::row_u64(row, 0),
        )
        .optional()?
        .ok_or(ProjectStoreError::OrchestratorWorkflowProfileNotFound)
}

pub(super) fn select_revision(
    connection: &Connection,
    profile_id: &str,
    version: u64,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let profile = connection
        .query_row(
            "SELECT version, instructions_markdown, monitor_interval_ms,
                    source, updated_by, created_at_unix_ms,
                    profile_id, name, description, commands_json,
                    adapter_context_files_json
               FROM orchestrator_workflow_profile_revisions
              WHERE profile_id = ?1 AND version = ?2",
            params![profile_id, to_i64(version)?],
            |row| {
                let source = match row.get::<_, String>(3)?.as_str() {
                    "factory" => OrchestratorWorkflowProfileSource::Factory,
                    "user" => OrchestratorWorkflowProfileSource::User,
                    "reset" => OrchestratorWorkflowProfileSource::Reset,
                    value => {
                        return Err(rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            format!("unknown workflow profile source {value}").into(),
                        ));
                    }
                };
                let commands = serde_json::from_str::<Vec<OrchestratorWorkflowCommand>>(
                    &row.get::<_, String>(9)?,
                )
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                let adapter_context_files = serde_json::from_str::<
                    Vec<OrchestratorWorkflowAdapterContextFile>,
                >(&row.get::<_, String>(10)?)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        10,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(OrchestratorWorkflowProfile {
                    id: row.get(6)?,
                    name: row.get(7)?,
                    description: row.get(8)?,
                    version: super::row_u64(row, 0)?,
                    instructions_markdown: row.get(1)?,
                    monitor_interval_ms: super::row_u64(row, 2)?,
                    commands,
                    adapter_context_files,
                    source,
                    updated_by: row.get(4)?,
                    created_at_unix_ms: super::row_u64(row, 5)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::OrchestratorWorkflowProfileNotFound)?;
    validate_stored_orchestrator_workflow_commands(&profile.commands)?;
    Ok(profile)
}

pub(super) fn select_executable_revision(
    connection: &Connection,
    profile_id: &str,
    version: u64,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let profile = select_revision(connection, profile_id, version)?;
    validate_orchestrator_workflow_commands(&profile.commands)?;
    Ok(profile)
}

fn insert_revision(
    connection: &Connection,
    input: &RevisionInput,
    version: u64,
    created_at_unix_ms: u64,
) -> Result<(), ProjectStoreError> {
    validate_orchestrator_workflow_commands(&input.commands)?;
    let source = match input.source {
        OrchestratorWorkflowProfileSource::Factory => "factory",
        OrchestratorWorkflowProfileSource::User => "user",
        OrchestratorWorkflowProfileSource::Reset => "reset",
    };
    let commands = serde_json::to_string(&input.commands)?;
    let adapter_context_files = serde_json::to_string(&input.adapter_context_files)?;
    connection.execute(
        "INSERT INTO orchestrator_workflow_profile_revisions (
            version, instructions_markdown, monitor_interval_ms,
            source, updated_by, created_at_unix_ms, profile_id, name,
            description, commands_json, adapter_context_files_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            to_i64(version)?,
            input.instructions_markdown,
            to_i64(input.monitor_interval_ms)?,
            source,
            input.actor,
            to_i64(created_at_unix_ms)?,
            input.profile_id,
            input.name,
            input.description,
            commands,
            adapter_context_files,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;
    use yard_domain::{
        FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS,
        FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS, OrchestratorWorkflowAdapterContextFile,
        ResetOrchestratorWorkflowProfile, UpdateOrchestratorWorkflowProfile,
        YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
    };

    use super::super::{SqliteProjectStore, YardStore};

    #[tokio::test]
    async fn defaults_survive_reopen_and_edits_reset_as_immutable_revisions() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&path).await.unwrap();
        let factory = store.get_orchestrator_workflow_profile().await.unwrap();
        assert_eq!(factory.version, 1);
        assert_eq!(factory.id, YARD_STANDARD_ORCHESTRATOR_PROFILE_ID);
        assert_eq!(factory.name, "Yard Standard Orchestrator");
        assert_eq!(factory.commands.len(), 9);
        assert!(factory.adapter_context_files.is_empty());
        assert_eq!(
            factory.monitor_interval_ms,
            FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS
        );
        assert_eq!(
            factory.instructions_markdown,
            FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS
        );

        let edited = store
            .update_orchestrator_workflow_profile(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                UpdateOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: factory.version,
                    instructions_markdown: "# Custom workflow".to_owned(),
                    monitor_interval_ms: 900_000,
                    commands: None,
                    adapter_context_files: Some(vec![OrchestratorWorkflowAdapterContextFile {
                        adapter_id: "example.adapter".to_owned(),
                        path: ".yard/context.md".to_owned(),
                    }]),
                },
            )
            .await
            .unwrap();
        assert_eq!(edited.version, 2);
        assert_eq!(edited.instructions_markdown, "# Custom workflow");
        assert_eq!(edited.adapter_context_files.len(), 1);
        assert_eq!(
            store
                .get_orchestrator_workflow_profile_revision(
                    YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                    1,
                )
                .await
                .unwrap()
                .instructions_markdown,
            FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS
        );
        drop(store);

        let reopened = SqliteProjectStore::open(&path).await.unwrap();
        assert_eq!(
            reopened.get_orchestrator_workflow_profile().await.unwrap(),
            edited
        );
        assert_eq!(
            reopened
                .list_orchestrator_workflow_profiles()
                .await
                .unwrap()
                .profiles,
            vec![edited.clone()]
        );
        let reset = reopened
            .reset_orchestrator_workflow_profile(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                ResetOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: edited.version,
                },
            )
            .await
            .unwrap();
        assert_eq!(reset.version, 3);
        assert_eq!(
            reset.instructions_markdown,
            FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS
        );
        assert_eq!(
            reset.monitor_interval_ms,
            FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS
        );
    }

    #[tokio::test]
    async fn update_and_reset_reject_stale_versions() {
        let temp = TempDir::new().unwrap();
        let store = SqliteProjectStore::open(temp.path().join("yard.sqlite3"))
            .await
            .unwrap();
        let current = store.get_orchestrator_workflow_profile().await.unwrap();
        store
            .update_orchestrator_workflow_profile(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                UpdateOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: current.version,
                    instructions_markdown: "# Custom workflow".to_owned(),
                    monitor_interval_ms: 900_000,
                    commands: None,
                    adapter_context_files: None,
                },
            )
            .await
            .unwrap();

        let error = store
            .reset_orchestrator_workflow_profile(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                ResetOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: current.version,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            super::ProjectStoreError::OrchestratorWorkflowProfileVersionConflict {
                current_version: 2
            }
        ));
    }

    #[tokio::test]
    async fn independent_profile_streams_each_start_at_revision_one() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&path).await.unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO orchestrator_workflow_profile_revisions (
                    profile_id, version, name, description, instructions_markdown,
                    monitor_interval_ms, commands_json, adapter_context_files_json,
                    source, updated_by, created_at_unix_ms
                 )
                 SELECT 'yard:independent', 1, 'Independent workflow',
                        'Independent test workflow', instructions_markdown,
                        monitor_interval_ms, commands_json, adapter_context_files_json,
                        'user', 'local-user', 1
                   FROM orchestrator_workflow_profile_revisions
                  WHERE profile_id = 'yard:standard-orchestrator' AND version = 1;
                 INSERT INTO orchestrator_workflow_profiles (
                    id, current_version, updated_at_unix_ms
                 ) VALUES ('yard:independent', 1, 1);",
            )
            .unwrap();
        drop(connection);

        let independent = store
            .update_orchestrator_workflow_profile(
                "yard:independent",
                UpdateOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: 1,
                    instructions_markdown: "# Independent revision two".to_owned(),
                    monitor_interval_ms: 600_000,
                    commands: None,
                    adapter_context_files: None,
                },
            )
            .await
            .unwrap();
        let standard = store
            .update_orchestrator_workflow_profile(
                YARD_STANDARD_ORCHESTRATOR_PROFILE_ID,
                UpdateOrchestratorWorkflowProfile {
                    actor: "local-user".to_owned(),
                    expected_version: 1,
                    instructions_markdown: "# Standard revision two".to_owned(),
                    monitor_interval_ms: 600_000,
                    commands: None,
                    adapter_context_files: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(independent.version, 2);
        assert_eq!(independent.name, "Independent workflow");
        assert_eq!(standard.version, 2);
        assert_eq!(
            store
                .get_orchestrator_workflow_profile_revision("yard:independent", 1)
                .await
                .unwrap()
                .version,
            1
        );
        let connection = Connection::open(path).unwrap();
        let revision_one_streams: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                   FROM orchestrator_workflow_profile_revisions
                  WHERE version = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let immutable = connection.execute(
            "UPDATE orchestrator_workflow_profile_revisions
                SET instructions_markdown = '# Mutated'
              WHERE profile_id = 'yard:independent' AND version = 1",
            [],
        );
        assert_eq!(revision_one_streams, 2);
        assert!(immutable.is_err());
    }
}

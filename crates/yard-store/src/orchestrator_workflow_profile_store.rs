use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use yard_domain::{
    FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS, FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS,
    OrchestratorWorkflowProfile, OrchestratorWorkflowProfileSource,
    ResetOrchestratorWorkflowProfile, UpdateOrchestratorWorkflowProfile,
};

use super::{ProjectStoreError, SqliteProjectStore, to_i64, unix_time_ms};

pub(super) async fn get_current(
    store: &SqliteProjectStore,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    store
        .run(|connection| {
            let version = select_current_version(connection)?;
            select_revision(connection, version)
        })
        .await
}

pub(super) async fn get_revision(
    store: &SqliteProjectStore,
    version: u64,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    if version == 0 {
        return Err(ProjectStoreError::OrchestratorWorkflowProfileNotFound);
    }
    store
        .run(move |connection| select_revision(connection, version))
        .await
}

pub(super) async fn update(
    store: &SqliteProjectStore,
    command: UpdateOrchestratorWorkflowProfile,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let command = command.normalize()?;
    append_revision(
        store,
        command.expected_version,
        command.instructions_markdown,
        command.monitor_interval_ms,
        OrchestratorWorkflowProfileSource::User,
        command.actor,
    )
    .await
}

pub(super) async fn reset(
    store: &SqliteProjectStore,
    command: ResetOrchestratorWorkflowProfile,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    let command = command.normalize()?;
    append_revision(
        store,
        command.expected_version,
        FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS.to_owned(),
        FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS,
        OrchestratorWorkflowProfileSource::Reset,
        command.actor,
    )
    .await
}

async fn append_revision(
    store: &SqliteProjectStore,
    expected_version: u64,
    instructions_markdown: String,
    monitor_interval_ms: u64,
    source: OrchestratorWorkflowProfileSource,
    actor: String,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_version = select_current_version(&transaction)?;
            if current_version != expected_version {
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
            insert_revision(
                &transaction,
                next_version,
                &instructions_markdown,
                monitor_interval_ms,
                source,
                &actor,
                now,
            )?;
            let changed = transaction.execute(
                "UPDATE orchestrator_workflow_profile_current
                    SET current_version = ?1, updated_at_unix_ms = ?2
                  WHERE singleton_id = 1 AND current_version = ?3",
                params![
                    to_i64(next_version)?,
                    to_i64(now)?,
                    to_i64(current_version)?,
                ],
            )?;
            if changed != 1 {
                return Err(
                    ProjectStoreError::OrchestratorWorkflowProfileVersionConflict {
                        current_version: select_current_version(&transaction)?,
                    },
                );
            }
            let saved = select_revision(&transaction, next_version)?;
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

    insert_revision(
        connection,
        1,
        FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS,
        FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS,
        OrchestratorWorkflowProfileSource::Factory,
        "yard:factory",
        0,
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

pub(super) fn select_current_version(connection: &Connection) -> Result<u64, ProjectStoreError> {
    connection
        .query_row(
            "SELECT current_version
               FROM orchestrator_workflow_profile_current
              WHERE singleton_id = 1",
            [],
            |row| super::row_u64(row, 0),
        )
        .map_err(Into::into)
}

pub(super) fn select_revision(
    connection: &Connection,
    version: u64,
) -> Result<OrchestratorWorkflowProfile, ProjectStoreError> {
    connection
        .query_row(
            "SELECT version, instructions_markdown, monitor_interval_ms,
                    source, updated_by, created_at_unix_ms
               FROM orchestrator_workflow_profile_revisions
              WHERE version = ?1",
            [to_i64(version)?],
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
                Ok(OrchestratorWorkflowProfile {
                    version: super::row_u64(row, 0)?,
                    instructions_markdown: row.get(1)?,
                    monitor_interval_ms: super::row_u64(row, 2)?,
                    source,
                    updated_by: row.get(4)?,
                    created_at_unix_ms: super::row_u64(row, 5)?,
                })
            },
        )
        .optional()?
        .ok_or(ProjectStoreError::OrchestratorWorkflowProfileNotFound)
}

fn insert_revision(
    connection: &Connection,
    version: u64,
    instructions_markdown: &str,
    monitor_interval_ms: u64,
    source: OrchestratorWorkflowProfileSource,
    actor: &str,
    created_at_unix_ms: u64,
) -> Result<(), ProjectStoreError> {
    let source = match source {
        OrchestratorWorkflowProfileSource::Factory => "factory",
        OrchestratorWorkflowProfileSource::User => "user",
        OrchestratorWorkflowProfileSource::Reset => "reset",
    };
    connection.execute(
        "INSERT INTO orchestrator_workflow_profile_revisions (
            version, instructions_markdown, monitor_interval_ms,
            source, updated_by, created_at_unix_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            to_i64(version)?,
            instructions_markdown,
            to_i64(monitor_interval_ms)?,
            source,
            actor,
            to_i64(created_at_unix_ms)?,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use yard_domain::{
        FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS,
        FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS, ResetOrchestratorWorkflowProfile,
        UpdateOrchestratorWorkflowProfile,
    };

    use super::super::{SqliteProjectStore, YardStore};

    #[tokio::test]
    async fn defaults_survive_reopen_and_edits_reset_as_immutable_revisions() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&path).await.unwrap();
        let factory = store.get_orchestrator_workflow_profile().await.unwrap();
        assert_eq!(factory.version, 1);
        assert_eq!(
            factory.monitor_interval_ms,
            FACTORY_ORCHESTRATOR_WORKFLOW_MONITOR_INTERVAL_MS
        );
        assert_eq!(
            factory.instructions_markdown,
            FACTORY_ORCHESTRATOR_WORKFLOW_INSTRUCTIONS
        );

        let edited = store
            .update_orchestrator_workflow_profile(UpdateOrchestratorWorkflowProfile {
                actor: "local-user".to_owned(),
                expected_version: factory.version,
                instructions_markdown: "# Custom workflow".to_owned(),
                monitor_interval_ms: 900_000,
            })
            .await
            .unwrap();
        assert_eq!(edited.version, 2);
        assert_eq!(edited.instructions_markdown, "# Custom workflow");
        assert_eq!(
            store
                .get_orchestrator_workflow_profile_revision(1)
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
        let reset = reopened
            .reset_orchestrator_workflow_profile(ResetOrchestratorWorkflowProfile {
                actor: "local-user".to_owned(),
                expected_version: edited.version,
            })
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
            .update_orchestrator_workflow_profile(UpdateOrchestratorWorkflowProfile {
                actor: "local-user".to_owned(),
                expected_version: current.version,
                instructions_markdown: "# Custom workflow".to_owned(),
                monitor_interval_ms: 900_000,
            })
            .await
            .unwrap();

        let error = store
            .reset_orchestrator_workflow_profile(ResetOrchestratorWorkflowProfile {
                actor: "local-user".to_owned(),
                expected_version: current.version,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            super::ProjectStoreError::OrchestratorWorkflowProfileVersionConflict {
                current_version: 2
            }
        ));
    }
}

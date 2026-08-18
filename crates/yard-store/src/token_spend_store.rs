use rusqlite::{Connection, TransactionBehavior, params};
use yard_domain::{AutomaticSummaryRequestKind, TokenSpendSettings, UpdateTokenSpendSettings};

use super::{ProjectStoreError, SqliteProjectStore, TokenSpendCommandSource, to_i64, unix_time_ms};

const MAX_TARGET_ID_BYTES: usize = 120;

pub(super) async fn get_settings(
    store: &SqliteProjectStore,
) -> Result<TokenSpendSettings, ProjectStoreError> {
    store.run(|connection| select_settings(connection)).await
}

pub(super) async fn update_settings(
    store: &SqliteProjectStore,
    command: UpdateTokenSpendSettings,
) -> Result<TokenSpendSettings, ProjectStoreError> {
    let command = command.normalize()?;
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = select_settings(&transaction)?;
            if current.version != command.expected_version {
                return Err(ProjectStoreError::TokenSpendSettingsVersionConflict {
                    current_version: current.version,
                });
            }
            let next_version = current
                .version
                .checked_add(1)
                .ok_or(ProjectStoreError::VersionOverflow)?;
            let now = unix_time_ms()?;
            let changed = transaction.execute(
                "UPDATE token_spend_settings
                    SET superintendent_auto_requests_project_summaries = ?1,
                        project_orchestrators_auto_request_worker_summaries = ?2,
                        scheduled_automatic_summaries = ?3,
                        version = ?4,
                        updated_by = ?5,
                        updated_at_unix_ms = ?6
                  WHERE id = 1 AND version = ?7",
                params![
                    command.superintendent_auto_requests_project_summaries,
                    command.project_orchestrators_auto_request_worker_summaries,
                    command.scheduled_automatic_summaries,
                    to_i64(next_version)?,
                    command.actor,
                    to_i64(now)?,
                    to_i64(current.version)?,
                ],
            )?;
            if changed != 1 {
                return Err(ProjectStoreError::TokenSpendSettingsVersionConflict {
                    current_version: select_settings(&transaction)?.version,
                });
            }
            let settings = select_settings(&transaction)?;
            transaction.commit()?;
            Ok(settings)
        })
        .await
}

pub(super) async fn claim_automatic_summary_request(
    store: &SqliteProjectStore,
    kind: AutomaticSummaryRequestKind,
    target_id: &str,
    now_unix_ms: u64,
    minimum_interval_ms: u64,
) -> Result<bool, ProjectStoreError> {
    let target_id = target_id.trim();
    if target_id.is_empty() || target_id.len() > MAX_TARGET_ID_BYTES {
        return Err(ProjectStoreError::AutomaticSummaryTargetInvalid);
    }
    let target_id = target_id.to_owned();
    store
        .run(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let settings = select_settings(&transaction)?;
            if !settings.automatic_summary_enabled(kind) {
                return Ok(false);
            }
            let request_kind = match kind {
                AutomaticSummaryRequestKind::SuperintendentProject => "superintendent_project",
                AutomaticSummaryRequestKind::ProjectWorker => "project_worker",
            };
            let claim_before_or_at = now_unix_ms.saturating_sub(minimum_interval_ms);
            let changed = transaction.execute(
                "INSERT INTO automatic_summary_request_watermarks (
                    request_kind, target_id, claimed_at_unix_ms
                 ) VALUES (?1, ?2, ?3)
                 ON CONFLICT(request_kind, target_id) DO UPDATE
                     SET claimed_at_unix_ms = excluded.claimed_at_unix_ms
                   WHERE automatic_summary_request_watermarks.claimed_at_unix_ms <= ?4
                      OR automatic_summary_request_watermarks.claimed_at_unix_ms > ?3",
                params![
                    request_kind,
                    target_id,
                    to_i64(now_unix_ms)?,
                    to_i64(claim_before_or_at)?,
                ],
            )?;
            transaction.commit()?;
            Ok(changed == 1)
        })
        .await
}

pub(super) fn ensure_command_source_enabled(
    connection: &Connection,
    source: TokenSpendCommandSource,
) -> Result<(), ProjectStoreError> {
    let settings = select_settings(connection)?;
    let enabled = match source {
        TokenSpendCommandSource::Manual => true,
        TokenSpendCommandSource::SuperintendentProjectSummary => {
            settings.automatic_summary_enabled(AutomaticSummaryRequestKind::SuperintendentProject)
        }
        TokenSpendCommandSource::ProjectWorkerSummary => {
            settings.automatic_summary_enabled(AutomaticSummaryRequestKind::ProjectWorker)
        }
        TokenSpendCommandSource::ScheduledSummary => settings.scheduled_automatic_summaries,
    };
    if enabled {
        Ok(())
    } else {
        Err(ProjectStoreError::AutomaticTokenSpendDisabled)
    }
}

fn select_settings(connection: &Connection) -> Result<TokenSpendSettings, ProjectStoreError> {
    connection
        .query_row(
            "SELECT superintendent_auto_requests_project_summaries,
                    project_orchestrators_auto_request_worker_summaries,
                    scheduled_automatic_summaries, version, updated_by,
                    updated_at_unix_ms
               FROM token_spend_settings
              WHERE id = 1",
            [],
            |row| {
                Ok(TokenSpendSettings {
                    superintendent_auto_requests_project_summaries: row.get(0)?,
                    project_orchestrators_auto_request_worker_summaries: row.get(1)?,
                    scheduled_automatic_summaries: row.get(2)?,
                    version: super::row_u64(row, 3)?,
                    updated_by: row.get(4)?,
                    updated_at_unix_ms: super::row_u64(row, 5)?,
                })
            },
        )
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use yard_domain::{AutomaticSummaryRequestKind, UpdateTokenSpendSettings};

    use super::super::{SqliteProjectStore, YardStore};

    #[tokio::test]
    async fn settings_default_off_persist_and_claim_only_the_enabled_layer() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&path).await.unwrap();

        let defaults = store.get_token_spend_settings().await.unwrap();
        assert!(!defaults.superintendent_auto_requests_project_summaries);
        assert!(!defaults.project_orchestrators_auto_request_worker_summaries);
        assert!(!defaults.scheduled_automatic_summaries);
        assert!(
            !store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "project-1",
                    1_000,
                    900,
                )
                .await
                .unwrap()
        );

        let superintendent_only = store
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: defaults.version,
                superintendent_auto_requests_project_summaries: true,
                project_orchestrators_auto_request_worker_summaries: false,
                scheduled_automatic_summaries: false,
            })
            .await
            .unwrap();
        assert!(
            store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "project-1",
                    1_000,
                    900,
                )
                .await
                .unwrap()
        );
        assert!(
            !store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "project-1",
                    1_100,
                    900,
                )
                .await
                .unwrap()
        );
        assert!(
            !store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::ProjectWorker,
                    "assignment-1",
                    1_000,
                    900,
                )
                .await
                .unwrap()
        );

        store
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: superintendent_only.version,
                superintendent_auto_requests_project_summaries: false,
                project_orchestrators_auto_request_worker_summaries: true,
                scheduled_automatic_summaries: true,
            })
            .await
            .unwrap();
        assert!(
            !store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "project-2",
                    2_000,
                    900,
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::ProjectWorker,
                    "assignment-1",
                    2_000,
                    900,
                )
                .await
                .unwrap()
        );
        drop(store);

        let reopened = SqliteProjectStore::open(path).await.unwrap();
        let persisted = reopened.get_token_spend_settings().await.unwrap();
        assert!(!persisted.superintendent_auto_requests_project_summaries);
        assert!(persisted.project_orchestrators_auto_request_worker_summaries);
        assert!(persisted.scheduled_automatic_summaries);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn watermarks_are_kind_scoped_durable_and_recover_from_future_clock_values() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = SqliteProjectStore::open(&path).await.unwrap();
        let defaults = store.get_token_spend_settings().await.unwrap();
        let enabled = store
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: defaults.version,
                superintendent_auto_requests_project_summaries: true,
                project_orchestrators_auto_request_worker_summaries: true,
                scheduled_automatic_summaries: false,
            })
            .await
            .unwrap();

        for kind in [
            AutomaticSummaryRequestKind::SuperintendentProject,
            AutomaticSummaryRequestKind::ProjectWorker,
        ] {
            assert!(
                store
                    .claim_automatic_summary_request(kind, "shared-target", 10_000, 1_000)
                    .await
                    .unwrap()
            );
        }
        assert!(
            store
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "persisted-target",
                    10_000,
                    1_000,
                )
                .await
                .unwrap()
        );
        drop(store);

        let reopened = SqliteProjectStore::open(&path).await.unwrap();
        assert!(
            !reopened
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "persisted-target",
                    10_500,
                    1_000,
                )
                .await
                .unwrap()
        );
        reopened
            .run(|connection| {
                connection.execute(
                    "INSERT INTO automatic_summary_request_watermarks (
                        request_kind, target_id, claimed_at_unix_ms
                     ) VALUES ('superintendent_project', 'future-target', 99000)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            reopened
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "future-target",
                    20_000,
                    1_000,
                )
                .await
                .unwrap()
        );
        assert!(
            !reopened
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::SuperintendentProject,
                    "future-target",
                    20_000,
                    1_000,
                )
                .await
                .unwrap()
        );

        assert!(
            reopened
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::ProjectWorker,
                    "toggle-target",
                    30_000,
                    1_000,
                )
                .await
                .unwrap()
        );
        let disabled = reopened
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: enabled.version,
                superintendent_auto_requests_project_summaries: false,
                project_orchestrators_auto_request_worker_summaries: false,
                scheduled_automatic_summaries: false,
            })
            .await
            .unwrap();
        assert!(
            !reopened
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::ProjectWorker,
                    "toggle-target",
                    30_500,
                    1_000,
                )
                .await
                .unwrap()
        );
        reopened
            .update_token_spend_settings(UpdateTokenSpendSettings {
                actor: "local-user".to_owned(),
                expected_version: disabled.version,
                superintendent_auto_requests_project_summaries: false,
                project_orchestrators_auto_request_worker_summaries: true,
                scheduled_automatic_summaries: false,
            })
            .await
            .unwrap();
        assert!(
            !reopened
                .claim_automatic_summary_request(
                    AutomaticSummaryRequestKind::ProjectWorker,
                    "toggle-target",
                    30_500,
                    1_000,
                )
                .await
                .unwrap()
        );
    }
}

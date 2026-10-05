//! The fork schema bridge (schema-bridge.md sections 4-7, tests T1-T12):
//! a pre-rebase fork database (d897fd1 numbering, `user_version` 29..34)
//! gains mainline's 0029..0033 objects and continues as rebased v34..v39;
//! every other lineage keeps its plain chain, and mixed schemas are refused
//! before anything is written.

use std::{fmt::Write as _, path::Path};

use rusqlite::types::Value as SqlValue;
use sha2::{Digest, Sha256};

use super::*;

/// sha256 of the verbatim `git show d897fd1:crates/yard-store/migrations/*`
/// blobs checked in under `tests/fixtures/fork_d897fd1/`.
const FORK_D897FD1_SHA256: &[(&str, &str)] = &[
    (
        "0029_snapshot_expiry_and_delete_commands.sql",
        "869bf1c56eac5b89a35faf240f2ab7fb77195f764976b87180e24b30a6610c92",
    ),
    (
        "0030_coordination_node_dispositions.sql",
        "2c5d412cc4e5025f6f14f6da819404cf7c64e96469a8a30f9de8f9fc358f6690",
    ),
    (
        "0031_assignment_disposition.sql",
        "a3e3ad4e11503744ea0c9084e960026f0d262cdaf4ac64fdd6975c75216033e0",
    ),
    (
        "0032_archive_active_work.sql",
        "f5a2bdbe55bc5e27ff9d47d6d24cc680206e0b64e3aa97afdf9743db1942c1cb",
    ),
    (
        "0033_project_restore.sql",
        "168ba0db15bae213c38b3870c03b5c812a696767db6439da927c7e69c94dfc09",
    ),
    (
        "0034_worker_display_names.sql",
        "2ecf9410a20e9be55211edaee348b59032da064f89cce8e9e6ad354e9e09df25",
    ),
];

/// Verbatim `git show e0f3190:crates/yard-store/migrations/0029..0033_*`
/// blobs: `(file name, sql, sha256)`. Independent of the crate's own copies.
const MAINLINE_E0F3190_MIGRATIONS: &[(&str, &str, &str)] = &[
    (
        "0029_project_repositories.sql",
        include_str!("../../tests/fixtures/mainline_e0f3190/0029_project_repositories.sql"),
        "d9fa8f0fbfbcb15a3bcf7ae3bc40c6ff8bfbb77055098e06426952f49442852d",
    ),
    (
        "0030_pane_management_leases.sql",
        include_str!("../../tests/fixtures/mainline_e0f3190/0030_pane_management_leases.sql"),
        "4ff7897a9a71ce19b5bb8eaabea9ef6d7a91fb8772dc9392b227623656bc1253",
    ),
    (
        "0031_worker_cleanup.sql",
        include_str!("../../tests/fixtures/mainline_e0f3190/0031_worker_cleanup.sql"),
        "357f6de4d750c8ae1ad7dc74850fdc2e4025d098bd6bff6f7c14798fa3567c39",
    ),
    (
        "0032_portable_profile_bundles.sql",
        include_str!("../../tests/fixtures/mainline_e0f3190/0032_portable_profile_bundles.sql"),
        "f6f4ec0ec43056281787807deed3f466362db5bc57db5d4af0afbab3b1e5f18b",
    ),
    (
        "0033_ephemeral_summary_workers.sql",
        include_str!("../../tests/fixtures/mainline_e0f3190/0033_ephemeral_summary_workers.sql"),
        "1d5b2c62ef46c00a279477668739dfe5a0f3ee8983f490267c17447c5fd1ad13",
    ),
];

/// Schema dump of a fresh database opened by the untouched e0f3190
/// `ProjectStore::open` (S0 item 5).
const MAINLINE_E0F3190_SCHEMA: &str =
    include_str!("../../tests/fixtures/mainline_e0f3190/schema.sql");
const MAINLINE_E0F3190_SCHEMA_SHA256: &str =
    "cfe5044eecd546b10b1b106ee111afe3804739516a704dcaf2d747042c0256f1";

/// Our renumbered migrations, in the order of `FORK_D897FD1_MIGRATIONS`.
const RENUMBERED_MIGRATIONS: &[(i64, &str)] = &[
    (34, crate::SNAPSHOT_EXPIRY_AND_DELETE_COMMANDS_MIGRATION),
    (35, crate::COORDINATION_NODE_DISPOSITIONS_MIGRATION),
    (36, crate::ASSIGNMENT_DISPOSITION_MIGRATION),
    (37, crate::ARCHIVE_ACTIVE_WORK_MIGRATION),
    (38, crate::PROJECT_RESTORE_MIGRATION),
    (39, crate::WORKER_DISPLAY_NAMES_MIGRATION),
];

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        })
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The top-level comma-separated items of a `CREATE TABLE (...)` body.
fn table_definitions(sql: &str) -> Vec<String> {
    let body = &sql[sql.find('(').unwrap() + 1..sql.rfind(')').unwrap()];
    let mut items = Vec::new();
    let (mut depth, mut quoted, mut start) = (0_i32, false, 0);
    for (index, character) in body.char_indices() {
        match character {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                items.push(collapse_whitespace(&body[start..index]));
                start = index + 1;
            }
            _ => {}
        }
    }
    items.push(collapse_whitespace(&body[start..]));
    items
}

/// SQL with whitespace collapsed and identifier quotes dropped (outside
/// string literals): `ALTER TABLE .. RENAME TO x` stores `CREATE TABLE "x"`,
/// so a table rebuilt by a downgrade helper differs from mainline's only there.
fn normalized_sql(sql: &str) -> String {
    let mut quoted = false;
    let unquoted = sql
        .chars()
        .filter(|character| {
            if *character == '\'' {
                quoted = !quoted;
            }
            quoted || *character != '"'
        })
        .collect::<String>();
    collapse_whitespace(&unquoted)
}

/// `schema_snapshot` without `sqlite_sequence`, normalized by
/// `normalized_sql`, and with the `workers` CREATE replaced by its sorted
/// column/constraint definitions: the fork path adds `display_name` before
/// mainline's two columns, a fresh database after them, so only the column
/// order may differ.
fn normalized_schema(connection: &Connection) -> Vec<(String, String, String, String)> {
    schema_snapshot(connection)
        .into_iter()
        .filter(|(_, name, _, _)| name != "sqlite_sequence")
        .map(|(kind, name, table, sql)| {
            let sql = sql.unwrap_or_default();
            let sql = if kind == "table" && name == "workers" {
                let mut definitions = table_definitions(&normalized_sql(&sql));
                definitions.sort();
                definitions.join("\n")
            } else {
                normalized_sql(&sql)
            };
            (kind, name, table, sql)
        })
        .collect()
}

/// The S0 dump format (`steps/S0_schemadump_main.rs`), byte for byte.
fn schema_dump(connection: &Connection) -> String {
    let mut dump = format!(
        "-- normalized schema of a fresh DB opened by yard-store @ e0f3190\n\
         -- PRAGMA user_version = {}\n",
        user_version(connection)
    );
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (kind, name, table, sql) = row.unwrap();
        let sql = collapse_whitespace(&sql.unwrap_or_default());
        writeln!(dump, "-- {kind} {name} on {table}\n{sql};").unwrap();
    }
    dump
}

async fn fresh_normalized_schema() -> Vec<(String, String, String, String)> {
    let temp = TempDir::new().unwrap();
    drop(open_store(&temp).await);
    normalized_schema(&Connection::open(temp.path().join("yard.sqlite3")).unwrap())
}

fn user_tables(connection: &Connection) -> Vec<String> {
    connection
        .prepare(
            "SELECT name FROM sqlite_schema
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
              ORDER BY rowid",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn column_names(connection: &Connection, schema: &str, table: &str) -> Vec<String> {
    connection
        .prepare(&format!(
            "SELECT name FROM {schema}.pragma_table_info(?1) ORDER BY cid"
        ))
        .unwrap()
        .query_map([table], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Rows of `table` read by column name and ordered by every column, so a
/// table rebuild (new rowids, new column order) compares equal.
fn named_rows(connection: &Connection, table: &str, columns: &[String]) -> Vec<Vec<SqlValue>> {
    let list = columns.join(", ");
    let order = (1..=columns.len())
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection
        .prepare(&format!("SELECT {list} FROM {table} ORDER BY {order}"))
        .unwrap();
    statement
        .query_map([], |row| {
            (0..columns.len())
                .map(|index| row.get(index))
                .collect::<Result<Vec<SqlValue>, _>>()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// A raw schema edit applied to a test database.
type SchemaEdit = fn(&Connection);

type RowSnapshot = Vec<(String, Vec<String>, Vec<Vec<SqlValue>>)>;

/// Every table's rows, keyed by the table's columns at snapshot time.
fn named_snapshot(connection: &Connection) -> RowSnapshot {
    user_tables(connection)
        .into_iter()
        .map(|table| {
            let columns = column_names(connection, "main", &table);
            let rows = named_rows(connection, &table, &columns);
            (table, columns, rows)
        })
        .collect()
}

/// Every pre-existing table still holds exactly its rows, compared on the
/// columns it had before (new columns are checked by the caller).
fn assert_rows_survive(connection: &Connection, before: &RowSnapshot) {
    for (table, columns, rows) in before {
        assert_eq!(&named_rows(connection, table, columns), rows, "{table}");
    }
}

fn foreign_key_violations(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn count(connection: &Connection, sql: &str) -> i64 {
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// Replace every row of `destination` with the rows of the database at
/// `source`, copying by column name (destination columns only). Turns a
/// schema built from migration files into a populated database without
/// passing the data through lossy downgrade helpers.
fn copy_rows_by_column_name(destination: &Connection, source: &Path) {
    destination
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    destination
        .execute("ATTACH DATABASE ?1 AS source", [source.to_str().unwrap()])
        .unwrap();
    // Triggers guard live writes (immutable revisions, workflow pins), not a
    // bulk copy: lift them for the copy and restore their exact SQL after.
    let triggers: Vec<(String, String)> = destination
        .prepare("SELECT name, sql FROM main.sqlite_schema WHERE type = 'trigger'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (name, _) in &triggers {
        destination
            .execute_batch(&format!("DROP TRIGGER main.{name};"))
            .unwrap();
    }
    let tables = user_tables(destination);
    for table in &tables {
        destination
            .execute_batch(&format!("DELETE FROM main.{table};"))
            .unwrap_or_else(|error| panic!("clear {table}: {error}"));
    }
    for table in &tables {
        let source_columns = column_names(destination, "source", table);
        if source_columns.is_empty() {
            continue;
        }
        let columns = column_names(destination, "main", table)
            .into_iter()
            .filter(|column| source_columns.contains(column))
            .collect::<Vec<_>>()
            .join(", ");
        destination
            .execute_batch(&format!(
                "INSERT INTO main.{table} ({columns}) SELECT {columns} FROM source.{table};"
            ))
            .unwrap_or_else(|error| panic!("copy {table}: {error}"));
    }
    for (_, sql) in &triggers {
        destination.execute_batch(sql).unwrap();
    }
    destination
        .execute_batch("DETACH DATABASE source;")
        .unwrap();
    destination
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    assert_eq!(foreign_key_violations(destination), 0);
}

/// A pure mainline v33 database: the shared 0001..0028 files on the fresh
/// path, then the verbatim e0f3190 0029..0033 fixtures. Uses none of the
/// downgrade helpers, so it is an independent reference for them.
fn build_pure_mainline_v33(connection: &mut Connection) {
    use crate::{
        AGENT_PROFILES_MIGRATION, ARTIFACT_MIGRATION, ASSIGNMENT_PROMPT_MIGRATION,
        COORDINATION_MIGRATION, COORDINATION_NODES_MIGRATION, ORCHESTRATOR_PROMPT_MIGRATION,
        ORCHESTRATOR_REPLACEMENT_MIGRATION, ORCHESTRATOR_REPLACEMENT_RECOVERY_MIGRATION,
        ORCHESTRATOR_WORKFLOW_PROFILE_MIGRATION, PROFILE_PROJECT_CREATION_MIGRATION,
        PROJECT_ORCHESTRATOR_TRANSFER_MIGRATION,
        PROVIDER_NEUTRAL_WORKFLOW_PROFILE_INTEGRITY_MIGRATION,
        PROVIDER_NEUTRAL_WORKFLOW_PROFILES_MIGRATION, RECONCILIATION_WATERMARK_MIGRATION,
        RUNTIME_CLEANUP_MIGRATION, RUNTIME_RECONCILIATION_MIGRATION,
        TOKEN_SPEND_SETTINGS_MIGRATION, WORKER_ALLOCATION_MIGRATION, WORKER_SESSION_END_MIGRATION,
        WORKSPACE_PROJECT_CREATION_MIGRATION, YARD_ORCHESTRATOR_MIGRATION,
        backfill_agent_profile_revisions, ensure_foreign_keys,
        orchestrator_workflow_profile_store as workflow,
    };
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    for sql in [
        INITIAL_MIGRATION,
        PROFILE_ASSIGNMENT_MIGRATION,
        COMPLETION_RECEIPT_MIGRATION,
        ASSIGNMENT_PROMPT_MIGRATION,
        RUNTIME_RECONCILIATION_MIGRATION,
        RECONCILIATION_WATERMARK_MIGRATION,
        WORKER_ALLOCATION_MIGRATION,
        PROFILE_PROJECT_CREATION_MIGRATION,
        WORKER_HANDOFF_MIGRATION,
        RUNTIME_CLEANUP_MIGRATION,
        ARTIFACT_MIGRATION,
        WORKSPACE_PROJECT_CREATION_MIGRATION,
        ORCHESTRATOR_PROMPT_MIGRATION,
        WORKER_SESSION_END_MIGRATION,
        YARD_ORCHESTRATOR_MIGRATION,
        COORDINATION_MIGRATION,
        COORDINATION_NODES_MIGRATION,
        AUTOMATIONS_MIGRATION,
        TOKEN_SPEND_SETTINGS_MIGRATION,
        ORCHESTRATOR_REPLACEMENT_MIGRATION,
    ] {
        transaction.execute_batch(sql).unwrap();
    }
    transaction.execute_batch(AGENT_PROFILES_MIGRATION).unwrap();
    backfill_agent_profile_revisions(&transaction).unwrap();
    transaction
        .execute_batch(ORCHESTRATOR_WORKFLOW_PROFILE_MIGRATION)
        .unwrap();
    workflow::seed_factory_profile(&transaction).unwrap();
    for sql in [
        PROJECT_ORCHESTRATOR_TRANSFER_MIGRATION,
        ORCHESTRATOR_REPLACEMENT_RECOVERY_MIGRATION,
        FINAL_BACKEND_SAFETY_MIGRATION,
        PROVIDER_NEUTRAL_WORKFLOW_PROFILES_MIGRATION,
    ] {
        transaction.execute_batch(sql).unwrap();
    }
    workflow::upgrade_provider_neutral_profile(&transaction, false).unwrap();
    transaction
        .execute_batch(PROVIDER_NEUTRAL_WORKFLOW_PROFILE_INTEGRITY_MIGRATION)
        .unwrap();
    workflow::activate_complete_provider_neutral_profile(&transaction).unwrap();
    transaction
        .execute_batch(PROJECT_ARCHIVING_MIGRATION)
        .unwrap();
    transaction
        .execute_batch(VISIBILITY_DELETIONS_MIGRATION)
        .unwrap();
    assert_eq!(user_version(&transaction), 28);
    for (name, sql, _) in MAINLINE_E0F3190_MIGRATIONS {
        transaction
            .execute_batch(sql)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    ensure_foreign_keys(&transaction).unwrap();
    transaction.commit().unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    assert_eq!(user_version(connection), 33);
}

// T11: fixture integrity.
#[test]
fn fork_and_mainline_fixtures_are_verbatim_and_renumbering_changes_only_the_version() {
    assert_eq!(FORK_D897FD1_MIGRATIONS.len(), FORK_D897FD1_SHA256.len());
    for ((level, name, sql), (sha_name, sha)) in
        FORK_D897FD1_MIGRATIONS.iter().zip(FORK_D897FD1_SHA256)
    {
        assert_eq!(name, sha_name);
        assert!(name.starts_with(&format!("00{level}_")), "{name}");
        assert_eq!(sha256_hex(sql), *sha, "{name}");
    }
    for (name, sql, sha) in MAINLINE_E0F3190_MIGRATIONS {
        assert_eq!(sha256_hex(sql), *sha, "{name}");
    }
    assert_eq!(
        sha256_hex(MAINLINE_E0F3190_SCHEMA),
        MAINLINE_E0F3190_SCHEMA_SHA256
    );
    // Mainline's files are kept unchanged in the rebased chain.
    for ((name, sql, _), crate_sql) in MAINLINE_E0F3190_MIGRATIONS
        .iter()
        .zip(crate::MAINLINE_POST_V28_MIGRATIONS)
    {
        assert_eq!(sql, crate_sql, "{name}");
    }
    // Ours differ from the fork's files only in the final PRAGMA line.
    for ((fork_level, name, fork_sql), (level, sql)) in
        FORK_D897FD1_MIGRATIONS.iter().zip(RENUMBERED_MIGRATIONS)
    {
        assert_eq!(*level, fork_level + crate::FORK_BRIDGE_OFFSET, "{name}");
        let fork_lines = fork_sql.lines().collect::<Vec<_>>();
        let lines = sql.lines().collect::<Vec<_>>();
        assert_eq!(fork_lines.len(), lines.len(), "{name}");
        let differing = fork_lines
            .iter()
            .zip(&lines)
            .filter(|(fork_line, line)| fork_line != line)
            .collect::<Vec<_>>();
        let fork_pragma = format!("PRAGMA user_version = {fork_level};");
        let pragma = format!("PRAGMA user_version = {level};");
        assert_eq!(
            differing,
            [(&fork_pragma.as_str(), &pragma.as_str())],
            "{name}"
        );
        assert_eq!(sql.ends_with('\n'), fork_sql.ends_with('\n'), "{name}");
    }
}

// T12: an independent pure-mainline v33 reference.
#[test]
fn pure_mainline_v33_fixture_matches_the_e0f3190_schema_and_the_downgrade_helpers() {
    let temp = TempDir::new().unwrap();
    let mut pure = Connection::open(temp.path().join("pure.sqlite3")).unwrap();
    build_pure_mainline_v33(&mut pure);
    assert_eq!(schema_dump(&pure), MAINLINE_E0F3190_SCHEMA);
    assert_eq!(
        crate::classify_schema_lineage(&pure, 33).unwrap(),
        crate::SchemaLineage::Mainline(33)
    );

    // T2's helper-built "mainline v33" must be the same schema.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let store_temp = TempDir::new().unwrap();
    drop(runtime.block_on(open_store(&store_temp)));
    let helper = Connection::open(store_temp.path().join("yard.sqlite3")).unwrap();
    downgrade_snapshot_expiry_schema_to_v33(&helper);
    assert_eq!(normalized_schema(&helper), normalized_schema(&pure));
}

/// Open `path` through the public store-open path, then reopen it raw.
async fn reopen_raw(path: &Path) -> Connection {
    drop(SqliteProjectStore::open(path).await.unwrap());
    Connection::open(path).unwrap()
}

fn assert_rebased_head(connection: &Connection) {
    assert_eq!(user_version(connection), SCHEMA_VERSION);
    assert_eq!(
        crate::classify_schema_lineage(connection, SCHEMA_VERSION).unwrap(),
        crate::SchemaLineage::Rebased(SCHEMA_VERSION)
    );
    assert_eq!(foreign_key_violations(connection), 0);
}

// T1 + T9: fresh database, idempotent reopen, newer schema refused.
#[tokio::test]
async fn fresh_database_is_rebased_head_reopens_unchanged_and_refuses_newer() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("yard.sqlite3");
    drop(open_store(&temp).await);
    let connection = Connection::open(&path).unwrap();
    assert_rebased_head(&connection);
    let schema = schema_snapshot(&connection);
    let rows = named_snapshot(&connection);
    drop(connection);

    let connection = reopen_raw(&path).await;
    assert_eq!(schema_snapshot(&connection), schema);
    assert_eq!(named_snapshot(&connection), rows);

    connection
        .execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
        .unwrap();
    drop(connection);
    assert!(matches!(
        SqliteProjectStore::open(&path).await.unwrap_err(),
        ProjectStoreError::UnsupportedSchema { found, supported }
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
    ));
    let connection = Connection::open(&path).unwrap();
    assert_eq!(schema_snapshot(&connection), schema);
}

// T4: every old fork level bridges to v+5 and finishes the rebased chain.
#[tokio::test]
async fn fork_databases_at_every_level_bridge_onto_the_rebased_chain() {
    let fresh = fresh_normalized_schema().await;
    for level in 29..=34 {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        let store = open_store(&temp).await;
        let (project_id, assignment) = create_active_assignment(&store).await;
        drop(store);
        let connection = Connection::open(&path).unwrap();
        build_fork_schema(&connection, level);
        let before = named_snapshot(&connection);
        drop(connection);

        let connection = reopen_raw(&path).await;
        assert_rebased_head(&connection);
        assert_eq!(normalized_schema(&connection), fresh, "fork v{level}");
        assert_rows_survive(&connection, &before);
        drop(connection);
        let store = SqliteProjectStore::open(&path).await.unwrap();
        let listed = listed_assignment(&store, &project_id, &assignment.id).await;
        assert_eq!(
            listed.lifecycle,
            AssignmentLifecycle::Active,
            "fork v{level}"
        );
    }
}

// T5: a rebased database left mid-chain (34..38) resumes without the bridge.
#[tokio::test]
async fn rebased_databases_left_mid_chain_resume() {
    let fresh = fresh_normalized_schema().await;
    let downgrades: [(i64, SchemaEdit); 5] = [
        (38, downgrade_worker_display_names_schema_to_v38),
        (37, downgrade_project_restore_schema_to_v37),
        (36, downgrade_archive_active_work_schema_to_v36),
        (35, downgrade_assignment_disposition_schema_to_v35),
        (34, downgrade_coordination_node_dispositions_schema_to_v34),
    ];
    for (level, downgrade) in downgrades {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        drop(open_store(&temp).await);
        let connection = Connection::open(&path).unwrap();
        downgrade(&connection);
        assert_eq!(user_version(&connection), level);
        assert_eq!(
            crate::classify_schema_lineage(&connection, level).unwrap(),
            crate::SchemaLineage::Rebased(level)
        );
        let policy = table_snapshot(&connection, "worker_cleanup_policy");
        drop(connection);
        let connection = reopen_raw(&path).await;
        assert_rebased_head(&connection);
        assert_eq!(normalized_schema(&connection), fresh, "rebased v{level}");
        // Mainline's objects were not recreated or reseeded.
        assert_eq!(table_snapshot(&connection, "worker_cleanup_policy"), policy);
    }
}

// T6: the bridge is one transaction; a failed foreign-key check leaves the
// fork database byte-for-byte as it was.
#[tokio::test]
async fn failed_fork_bridge_rolls_back_completely() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("yard.sqlite3");
    drop(open_store(&temp).await);
    let connection = Connection::open(&path).unwrap();
    build_fork_schema(&connection, 34);
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO command_acknowledgements (
                id, command_type, actor, status, error_message,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES ('orphan-rename', 'worker_rename', 'local-user', 'succeeded',
                       NULL, 1, 1);
             INSERT INTO worker_rename_commands (
                command_id, worker_id, expected_display_name, display_name,
                previous_display_name, renamed_at_unix_ms
             ) VALUES ('orphan-rename', 'missing-worker', NULL, 'Ghost', NULL, 1);
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
    let schema = schema_snapshot(&connection);
    let rows = named_snapshot(&connection);
    drop(connection);

    let error = SqliteProjectStore::open(&path).await.unwrap_err();
    assert!(
        matches!(error, ProjectStoreError::ForeignKeyCheckFailed),
        "{error:?}"
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(user_version(&connection), 34);
    assert_eq!(schema_snapshot(&connection), schema);
    assert_eq!(named_snapshot(&connection), rows);
    assert!(!crate::table_exists(&connection, "project_repositories").unwrap());
    assert!(!crate::table_has_column(&connection, "workers", "ownership_kind").unwrap());
    assert_eq!(
        crate::classify_schema_lineage(&connection, 34).unwrap(),
        crate::SchemaLineage::Fork(34)
    );
}

/// Opening `path` is refused as an invalid lineage and writes nothing.
async fn assert_refused_unchanged(path: &Path) {
    let connection = Connection::open(path).unwrap();
    let version = user_version(&connection);
    let schema = schema_snapshot(&connection);
    drop(connection);
    let error = SqliteProjectStore::open(path).await.unwrap_err();
    assert!(
        matches!(error, ProjectStoreError::InvalidSchemaLineage { .. }),
        "{error:?}"
    );
    let connection = Connection::open(path).unwrap();
    assert_eq!(user_version(&connection), version);
    assert_eq!(schema_snapshot(&connection), schema);
}

// T7: schemas no chain produces are refused before any write.
#[tokio::test]
async fn ambiguous_schema_lineages_are_refused_unchanged() {
    let cases: [(&str, SchemaEdit); 4] = [
        ("fork v34 with a mainline table", |connection| {
            build_fork_schema(connection, 34);
            connection
                .execute_batch(
                    "CREATE TABLE project_repositories (id TEXT PRIMARY KEY NOT NULL) STRICT;",
                )
                .unwrap();
        }),
        ("mainline v31 with a fork table", |connection| {
            downgrade_snapshot_expiry_schema_to_v33(connection);
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     DROP TABLE summary_worker_commands;
                     ALTER TABLE worker_cleanup_run_items DROP COLUMN pane_instance_id;
                     DROP TABLE profile_launch_audits;
                     DROP TABLE agent_profile_files;
                     PRAGMA user_version = 31;
                     CREATE TABLE worker_rename_commands (command_id TEXT PRIMARY KEY) STRICT;
                     CREATE INDEX worker_rename_commands_by_worker
                         ON worker_rename_commands (command_id);
                     ALTER TABLE workers ADD COLUMN display_name TEXT;
                     PRAGMA foreign_keys = ON;",
                )
                .unwrap();
        }),
        ("fork v33 stamped 34", |connection| {
            build_fork_schema(connection, 33);
            connection
                .execute_batch("PRAGMA user_version = 34;")
                .unwrap();
        }),
        ("mainline v33 stamped 34", |connection| {
            downgrade_snapshot_expiry_schema_to_v33(connection);
            connection
                .execute_batch("PRAGMA user_version = 34;")
                .unwrap();
        }),
    ];
    for (case, plant) in cases {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("yard.sqlite3");
        drop(open_store(&temp).await);
        plant(&Connection::open(&path).unwrap());
        println!("case: {case}");
        assert_refused_unchanged(&path).await;
    }
}

// T8: the 3fdcf5e v28 database is repaired inside 28 -> 29 and finishes.
#[tokio::test]
async fn v28_database_from_3fdcf5e_reaches_the_rebased_head() {
    let fresh = fresh_normalized_schema().await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("yard.sqlite3");
    drop(open_store(&temp).await);
    let connection = Connection::open(&path).unwrap();
    downgrade_snapshot_expiry_schema_to_v33(&connection);
    downgrade_mainline_schema_to_v28(&connection);
    downgrade_visibility_deletions_schema_to_3fdcf5e(&connection);
    assert!(crate::visibility_deletions_schema_is_3fdcf5e(&connection).unwrap());
    drop(connection);
    let connection = reopen_raw(&path).await;
    assert_rebased_head(&connection);
    assert!(!crate::visibility_deletions_schema_is_3fdcf5e(&connection).unwrap());
    assert_eq!(normalized_schema(&connection), fresh);
}

// T10: every command type the store writes is in the acknowledgement CHECK
// list (the closed-list trap: a missing type fails only at first write).
#[tokio::test]
async fn acknowledgement_check_list_covers_every_command_type_the_store_writes() {
    let temp = TempDir::new().unwrap();
    drop(open_store(&temp).await);
    let connection = Connection::open(temp.path().join("yard.sqlite3")).unwrap();
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'command_acknowledgements'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let list = &sql[sql.find("command_type IN (").unwrap()..];
    let list = &list[..list.find(')').unwrap()];
    let allowed = list.split('\'').skip(1).step_by(2).collect::<HashSet<_>>();

    let mut written = BTreeMap::<String, String>::new();
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&source_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let source = source.split("#[cfg(test)]").next().unwrap();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        for (offset, _) in source.match_indices("INSERT INTO command_acknowledgements (") {
            let values = &source[offset..];
            let values = &values[values.find("VALUES").unwrap()..];
            let values = &values[values.find('(').unwrap() + 1..];
            let second = values.split(',').nth(1).unwrap().trim();
            if let Some(literal) = second.strip_prefix('\'') {
                let literal = literal.trim_end_matches('\'');
                written.insert(literal.to_owned(), file.clone());
            } else {
                // Parameterized: the type comes from a `command_type: "…"` intent.
                assert!(second.starts_with('?'), "{file}: {second}");
            }
        }
        for (offset, _) in source.match_indices("command_type: \"") {
            let literal = &source[offset + "command_type: \"".len()..];
            let literal = &literal[..literal.find('"').unwrap()];
            written.insert(literal.to_owned(), file.clone());
        }
    }
    assert!(written.len() >= 35, "{written:?}");
    for (command_type, file) in &written {
        assert!(
            allowed.contains(command_type.as_str()),
            "{file} writes {command_type}, missing from the CHECK list"
        );
    }
    for fork_type in [
        "assignment_disposition",
        "coordination_node_archive",
        "coordination_node_delete",
        "project_restore",
        "worker_rename",
    ] {
        assert!(allowed.contains(fork_type), "{fork_type}");
    }
}

/// Rows only mainline's 0029..0033 can hold that the store API does not
/// write in these tests: a repository link, an installation + pane lease, and
/// an agent profile file.
fn insert_mainline_only_rows(connection: &Connection) {
    let (allocation_id, project_id, worker_id): (String, String, String) = connection
        .query_row(
            "SELECT id, project_id, worker_id FROM worker_allocations ORDER BY id LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let (profile_id, profile_version): (String, i64) = connection
        .query_row(
            "SELECT profile_id, profile_version FROM agent_profile_revisions
              ORDER BY profile_id, profile_version LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO project_repositories (
                id, project_id, root_path, git_common_dir,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES ('repo-1', ?1, '/src/repo', '/src/repo/.git', 1, 1)",
            [&project_id],
        )
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO yard_installation (singleton_id, installation_uuid, created_at_unix_ms)
             VALUES (1, 'installation-1', 1);
             INSERT INTO pane_management_batches (
                command_id, actor, input_hash, status, result_json,
                created_at_unix_ms, updated_at_unix_ms
             ) VALUES ('batch-1', 'local-user', 'hash', 'succeeded', '{}', 1, 2);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO pane_management_leases (
                worker_id, project_id, allocation_id, installation_uuid, herdr_session,
                pane_id, pane_instance_id, owner_id, lease_token, expires_at_unix_ms,
                acquisition_request_id, status, renewal_failures, renew_after_unix_ms,
                last_error_code, created_at_unix_ms, updated_at_unix_ms
             ) VALUES (?1, ?2, ?3, 'installation-1', 'default', 'pane-lease',
                       'instance-lease', 'owner-1', 'token-1', 9, 'acquire-1', 'active',
                       0, 5, NULL, 1, 1)",
            params![worker_id, project_id, allocation_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_profile_files (
                profile_id, profile_version, path, media_type, content, sha256
             ) VALUES (?1, ?2, 'AGENTS.md', 'text/markdown', 'Be brief.', ?3)",
            params![profile_id, profile_version, "a".repeat(64)],
        )
        .unwrap();
}

/// API-written mainline rows: a cleanup run whose item points at a receipt,
/// and a summary command pointing at an assignment.
async fn populate_mainline_features(store: &SqliteProjectStore) -> (String, SummaryFixture) {
    let (_, _, receipt_id) = create_eligible_cleanup_candidate(store, "bridge").await;
    store
        .start_worker_cleanup_run(
            WorkerCleanupRunTrigger::Manual,
            StartWorkerCleanupRun {
                command_id: "bridge-cleanup-run".to_owned(),
                actor: "cleanup-test".to_owned(),
                preview: false,
            },
        )
        .await
        .unwrap();
    let summary = create_summary_fixture(store, "bridge").await;
    (receipt_id, summary)
}

fn assert_mainline_rows_present(connection: &Connection) {
    for table in [
        "project_repositories",
        "yard_installation",
        "pane_management_batches",
        "pane_management_leases",
        "agent_profile_files",
        "worker_cleanup_runs",
        "worker_cleanup_run_items",
        "summary_worker_commands",
    ] {
        assert!(
            count(connection, &format!("SELECT COUNT(*) FROM {table}")) > 0,
            "{table}"
        );
    }
}

// T2: a populated mainline v33 database takes our six migrations.
#[tokio::test]
async fn populated_mainline_v33_database_reaches_the_rebased_head_with_its_rows() {
    let fresh = fresh_normalized_schema().await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("yard.sqlite3");
    let store = open_store(&temp).await;
    let (receipt_id, summary) = populate_mainline_features(&store).await;
    let deleted = create_archived_project(
        &store,
        "bridge-deleted",
        "bridge-deleted-terminal",
        "bridge-archive-before-delete",
    )
    .await;
    store
        .delete_project(
            &deleted.id,
            DeleteProject {
                command_id: "bridge-legacy-delete".to_owned(),
                actor: "local-user".to_owned(),
                archive: None,
            },
        )
        .await
        .unwrap();
    drop(store);

    let connection = Connection::open(&path).unwrap();
    downgrade_snapshot_expiry_schema_to_v33(&connection);
    insert_mainline_only_rows(&connection);
    assert_eq!(
        crate::classify_schema_lineage(&connection, 33).unwrap(),
        crate::SchemaLineage::Mainline(33)
    );
    assert_mainline_rows_present(&connection);
    let before = named_snapshot(&connection);
    drop(connection);

    let connection = reopen_raw(&path).await;
    assert_rebased_head(&connection);
    assert_eq!(normalized_schema(&connection), fresh);
    assert_rows_survive(&connection, &before);
    assert_eq!(
        count(&connection, "SELECT COUNT(*) FROM project_delete_commands"),
        count(&connection, "SELECT COUNT(*) FROM deleted_projects")
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM completion_receipts WHERE detail_level <> 'detailed'"
        ),
        0
    );
    let receipt_level: String = connection
        .query_row(
            "SELECT detail_level FROM completion_receipts WHERE id = ?1",
            [&receipt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt_level, "detailed");
    drop(connection);
    let store = SqliteProjectStore::open(&path).await.unwrap();
    let migrated = listed_assignment(&store, &summary.project.id, &summary.assignment.id).await;
    assert_eq!(migrated.lifecycle, AssignmentLifecycle::Active);
}

// T12 (continued): the independent mainline v33, populated with
// mainline-shaped rows, upgrades to the rebased head with every row intact.
#[tokio::test]
async fn pure_mainline_v33_database_with_mainline_rows_reaches_the_rebased_head() {
    let fresh = fresh_normalized_schema().await;
    let source = TempDir::new().unwrap();
    let store = open_store(&source).await;
    populate_mainline_features(&store).await;
    drop(store);

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pure.sqlite3");
    let mut connection = Connection::open(&path).unwrap();
    build_pure_mainline_v33(&mut connection);
    copy_rows_by_column_name(&connection, &source.path().join("yard.sqlite3"));
    insert_mainline_only_rows(&connection);
    assert_eq!(schema_dump(&connection), MAINLINE_E0F3190_SCHEMA);
    assert_mainline_rows_present(&connection);
    let before = named_snapshot(&connection);
    drop(connection);

    let connection = reopen_raw(&path).await;
    assert_rebased_head(&connection);
    assert_eq!(normalized_schema(&connection), fresh);
    assert_rows_survive(&connection, &before);
}

/// Fork-only state of every kind the live database holds, written through
/// the store API on a rebased database. Returns the renamed worker and the
/// re-archived project.
#[allow(clippy::too_many_lines)]
async fn populate_fork_features(store: &SqliteProjectStore, temp: &TempDir) -> (String, Project) {
    // Runtime snapshots first: the workstream helper reconciles at a later
    // watermark than `create_active_assignment`'s fixed snapshots.
    let (_, named) = create_active_assignment_with_suffix(store, "named").await;
    let (cancel_project, cancelled) = create_active_assignment_with_suffix(store, "cancel").await;
    let (minimal_project, minimal) = create_active_assignment_with_suffix(store, "minimal").await;

    // An archived and a deleted workstream (coordination_node_archive/delete).
    let (archived_node, archived_project) =
        provisioned_workstream(store, temp, "t3-archived").await;
    store
        .archive_coordination_node(
            &archived_node.id,
            node_archive(&archived_node, "t3-node-archive"),
        )
        .await
        .unwrap();
    // (One provisioned workstream per store: its reconcile moves the runtime
    // watermark past the helper's next snapshot.)
    let deleted_node_id = Uuid::now_v7().to_string();
    let deleted_node = store
        .create_coordination_node(
            &deleted_node_id,
            Some(
                temp.path()
                    .join("coordination")
                    .join(&deleted_node_id)
                    .to_string_lossy()
                    .into_owned(),
            ),
            None,
            create_node_command(
                "t3-deleted-create-node",
                CoordinationNodeKind::Workstream,
                vec![archived_project.id.clone()],
            ),
        )
        .await
        .unwrap()
        .node;
    store
        .delete_coordination_node(
            &deleted_node.id,
            node_delete(&deleted_node, "t3-node-delete"),
        )
        .await
        .unwrap();

    // A user-named worker (worker_rename).
    store
        .rename_worker(
            &named.worker.id,
            rename_command("t3-rename", None, Some("Builder")),
        )
        .await
        .unwrap();

    // Archived, restored (rebinding its orchestrator: the archive's cleanup
    // job is cancelled and the retired binding released), then re-archived.
    let project =
        create_archived_project(store, "t3-cycle", "t3-cycle-terminal", "t3-archive-1").await;
    store
        .restore_project(
            &project.id,
            restore_command("t3-restore", "t3-archive-1"),
            rebind_live_runtime(store, &project.id, "t3-archive-1").await,
        )
        .await
        .unwrap();
    let restored = store.get_project(&project.id).await.unwrap();
    store
        .archive_project(&restored.id, archive_command(&restored, "t3-archive-2"))
        .await
        .unwrap();

    // A cancelled assignment, and a minimal receipt with a captured transcript
    // (assignment_disposition).
    let outcome = yard_domain::DispositionOutcome::Cancelled;
    let command = disposition_command("t3-cancel", &cancelled, outcome, false);
    store
        .dispose_assignment(
            &cancel_project,
            &cancelled.id,
            command,
            yard_domain::RequestOrigin::Browser,
        )
        .await
        .unwrap();
    let outcome = yard_domain::DispositionOutcome::Completed;
    let command = disposition_command("t3-minimal", &minimal, outcome, false);
    store
        .dispose_assignment(
            &minimal_project,
            &minimal.id,
            command,
            yard_domain::RequestOrigin::Browser,
        )
        .await
        .unwrap();
    for job in store
        .claim_pending_transcript_captures(Some("t3-minimal"), 10, 30_000)
        .await
        .unwrap()
    {
        store
            .succeed_transcript_capture(&job.id, &job.claim_token, yard_store_text("done"))
            .await
            .unwrap();
    }

    // A deleted project and a deleted worker.
    let gone =
        create_archived_project(store, "t3-gone", "t3-gone-terminal", "t3-gone-archive").await;
    let delete = DeleteProject {
        command_id: "t3-project-delete".to_owned(),
        actor: "local-user".to_owned(),
        archive: None,
    };
    store.delete_project(&gone.id, delete).await.unwrap();
    let (worker_id, worker_version) =
        create_ended_worker(store, "t3-ended", "t3-ended-terminal", "t3-end-worker").await;
    store
        .delete_worker(
            &worker_id,
            DeleteWorker {
                command_id: "t3-worker-delete".to_owned(),
                actor: "local-user".to_owned(),
                expected_worker_version: worker_version,
            },
        )
        .await
        .unwrap();
    (named.worker.id, restored)
}

// T3: a live-shaped fork v34 database (the d897fd1 live Yard) is bridged to
// v39 in one step with every row intact.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn live_shaped_fork_v34_database_bridges_with_every_row_intact() {
    let fresh = fresh_normalized_schema().await;
    let source = TempDir::new().unwrap();
    let store = open_store(&source).await;
    let (named_worker, rearchived) = populate_fork_features(&store, &source).await;
    drop(store);

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("yard.sqlite3");
    drop(open_store(&temp).await);
    let connection = Connection::open(&path).unwrap();
    build_fork_schema(&connection, 34);
    copy_rows_by_column_name(&connection, &source.path().join("yard.sqlite3"));
    assert_eq!(
        crate::classify_schema_lineage(&connection, 34).unwrap(),
        crate::SchemaLineage::Fork(34)
    );
    // The fixture holds every fork-only shape the live database has.
    for (shape, sql) in [
        (
            "named worker",
            "SELECT COUNT(*) FROM workers WHERE display_name IS NOT NULL",
        ),
        (
            "re-archived project",
            "SELECT COUNT(*) FROM (SELECT project_id FROM archived_projects
               GROUP BY project_id HAVING COUNT(*) > 1)",
        ),
        (
            "restored archive",
            "SELECT COUNT(*) FROM archived_projects WHERE restored_at_unix_ms IS NOT NULL",
        ),
        (
            "cancelled assignment",
            "SELECT COUNT(*) FROM assignments WHERE lifecycle = 'cancelled'",
        ),
        (
            "cancellation",
            "SELECT COUNT(*) FROM assignment_cancellations",
        ),
        (
            "minimal receipt",
            "SELECT COUNT(*) FROM completion_receipts WHERE detail_level = 'minimal'",
        ),
        (
            "transcript job",
            "SELECT COUNT(*) FROM transcript_capture_jobs",
        ),
        ("transcript", "SELECT COUNT(*) FROM worker_transcripts"),
        (
            "archived node",
            "SELECT COUNT(*) FROM archived_coordination_nodes",
        ),
        (
            "deleted node",
            "SELECT COUNT(*) FROM deleted_coordination_nodes",
        ),
        ("deleted project", "SELECT COUNT(*) FROM deleted_projects"),
        (
            "project delete command",
            "SELECT COUNT(*) FROM project_delete_commands",
        ),
        ("deleted worker", "SELECT COUNT(*) FROM deleted_workers"),
        (
            "pending cleanup job",
            "SELECT COUNT(*) FROM runtime_cleanup_jobs WHERE status = 'pending'",
        ),
        (
            "cancelled cleanup job",
            "SELECT COUNT(*) FROM runtime_cleanup_jobs WHERE status = 'cancelled'",
        ),
        (
            "released retired binding",
            "SELECT COUNT(*) FROM retired_runtime_bindings WHERE released_by_command_id IS NOT NULL",
        ),
    ] {
        assert!(count(&connection, sql) > 0, "{shape}");
    }
    for command_type in [
        "assignment_disposition",
        "coordination_node_archive",
        "coordination_node_delete",
        "project_restore",
        "worker_rename",
    ] {
        assert!(
            count(
                &connection,
                &format!(
                    "SELECT COUNT(*) FROM command_acknowledgements
                      WHERE command_type = '{command_type}'"
                ),
            ) > 0,
            "{command_type}"
        );
    }
    let created: HashSet<String> = connection
        .prepare("SELECT DISTINCT worker_id FROM worker_allocations WHERE mode = 'create_new'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!created.is_empty());
    let before = named_snapshot(&connection);
    drop(connection);

    let connection = reopen_raw(&path).await;
    assert_rebased_head(&connection);
    assert_eq!(normalized_schema(&connection), fresh);
    assert_rows_survive(&connection, &before);
    let workers: Vec<(String, String, Option<String>)> = connection
        .prepare("SELECT id, ownership_kind, parent_worker_id FROM workers")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (id, ownership_kind, parent_worker_id) in &workers {
        let expected = if created.contains(id) {
            "yard_owned"
        } else {
            "external"
        };
        assert_eq!(ownership_kind, expected, "{id}");
        assert_eq!(parent_worker_id, &None, "{id}");
    }
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM workers WHERE ownership_kind = 'yard_owned'"
        ),
        i64::try_from(created.len()).unwrap()
    );
    let policy: (i64, i64, i64, i64, i64, Option<String>, i64, String, i64) = connection
        .query_row(
            "SELECT singleton_id, automatic_enabled, schedule_minutes, grace_period_ms,
                    batch_size, advisor_profile_id, version, updated_by, updated_at_unix_ms
               FROM worker_cleanup_policy",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        policy,
        (
            1,
            0,
            60,
            86_400_000,
            25,
            None,
            1,
            "yard:migration".to_owned(),
            0
        )
    );
    drop(connection);

    // Fork commands and a mainline command work on the bridged store.
    let store = SqliteProjectStore::open(&path).await.unwrap();
    store
        .rename_worker(
            &named_worker,
            rename_command("t3-rename-after", Some("Builder"), Some("Lead")),
        )
        .await
        .unwrap();
    store
        .restore_project(
            &rearchived.id,
            restore_command("t3-restore-after", "t3-archive-2"),
            crate::ProjectRestoreRuntime::Absent,
        )
        .await
        .unwrap();
    let preview = store
        .start_worker_cleanup_run(
            WorkerCleanupRunTrigger::Preview,
            StartWorkerCleanupRun {
                command_id: "t3-cleanup-preview".to_owned(),
                actor: "local-user".to_owned(),
                preview: true,
            },
        )
        .await
        .unwrap();
    assert!(preview.preview);
}

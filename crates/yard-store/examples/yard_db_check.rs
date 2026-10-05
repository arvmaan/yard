//! Offline database checks for the schema-bridge rehearsal and deploy. Dev
//! tool, not part of the shipped binary. Uses the bundled SQLite of
//! `rusqlite`, so the host needs no `sqlite3` CLI.
//!
//! ```text
//! yard_db_check open   <db>        store open only (migrate + bridge +
//!                                  store-open sweeps), then exit; prints the
//!                                  bridge time and every row-count change
//! yard_db_check check  <db>        user_version, integrity_check, foreign_key_check
//! yard_db_check counts <db>        count(*) for every table, sorted by name
//! yard_db_check backup <src> <dst> online backup in one consistent snapshot;
//!                                  safe while Yard is running
//! ```
//!
//! `open` starts no server, background loop, Herdr or Slack client. Every
//! other command opens the database read-only (`backup` reads `src` only).

use std::{collections::BTreeMap, path::Path, process::ExitCode, time::Instant};

use rusqlite::{Connection, OpenFlags, backup::Backup};

type Counts = BTreeMap<String, i64>;

/// `count(*)` for every table (including `sqlite_sequence`), by name.
fn counts(connection: &Connection) -> rusqlite::Result<Counts> {
    let tables = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tables
        .into_iter()
        .map(|table| {
            let quoted = table.replace('"', "\"\"");
            let count =
                connection.query_row(&format!("SELECT count(*) FROM \"{quoted}\""), [], |row| {
                    row.get(0)
                })?;
            Ok((table, count))
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct CheckReport {
    user_version: i64,
    /// `PRAGMA integrity_check` rows; `["ok"]` when healthy.
    integrity: Vec<String>,
    /// `PRAGMA foreign_key_check` rows as `table rowid parent fkid`.
    foreign_key_violations: Vec<String>,
}

impl CheckReport {
    fn healthy(&self) -> bool {
        self.integrity == ["ok"] && self.foreign_key_violations.is_empty()
    }
}

fn check(connection: &Connection) -> rusqlite::Result<CheckReport> {
    let user_version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let integrity = connection
        .prepare("PRAGMA integrity_check")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let foreign_key_violations = connection
        .prepare("PRAGMA foreign_key_check")?
        .query_map([], |row| {
            Ok(format!(
                "{} {} {} {}",
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?
                    .map_or_else(|| "-".to_owned(), |rowid| rowid.to_string()),
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(CheckReport {
        user_version,
        integrity,
        foreign_key_violations,
    })
}

/// Opens an EXISTING database read-only; never creates one.
fn open_read_only(path: &str) -> Result<Connection, String> {
    if !Path::new(path).is_file() {
        return Err(format!("{path}: no such database file"));
    }
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("{path}: {error}"))
}

fn user_version(path: &str) -> Result<i64, String> {
    open_read_only(path)?
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())
}

/// Row-count changes between two `counts` maps, one line per table.
fn count_changes(before: &Counts, after: &Counts) -> Vec<String> {
    let mut tables = before.keys().chain(after.keys()).collect::<Vec<_>>();
    tables.sort();
    tables.dedup();
    tables
        .into_iter()
        .filter_map(|table| match (before.get(table), after.get(table)) {
            (Some(old), Some(new)) if old == new => None,
            (Some(old), Some(new)) => Some(format!("{table} {old} -> {new} ({:+})", new - old)),
            (None, Some(new)) => Some(format!("{table} (new table) {new}")),
            (Some(old), None) => Some(format!("{table} {old} -> (table dropped)")),
            (None, None) => None,
        })
        .collect()
}

fn command_open(path: &str) -> Result<bool, String> {
    let before_version = user_version(path)?;
    let before = counts(&open_read_only(path)?).map_err(|error| error.to_string())?;
    // The bridge logs `bridged fork schema v.. onto mainline chain as v..`
    // with its elapsed_ms at INFO.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false)
        .try_init();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    let opened = runtime.block_on(yard_store::SqliteProjectStore::open(path));
    let elapsed_ms = started.elapsed().as_millis();
    let store = opened.map_err(|error| format!("store open failed: {error}"))?;
    drop(store);
    let after_version = user_version(path)?;
    let after = counts(&open_read_only(path)?).map_err(|error| error.to_string())?;
    println!("user_version {before_version} -> {after_version}");
    println!("store open (migrate + bridge + sweeps) {elapsed_ms} ms");
    let changes = count_changes(&before, &after);
    println!("row-count changes: {}", changes.len());
    for change in changes {
        println!("  {change}");
    }
    Ok(true)
}

fn command_check(path: &str) -> Result<bool, String> {
    let report = check(&open_read_only(path)?).map_err(|error| error.to_string())?;
    println!("user_version {}", report.user_version);
    println!("integrity_check {}", report.integrity.join("; "));
    println!(
        "foreign_key_check {} violation(s)",
        report.foreign_key_violations.len()
    );
    for violation in report.foreign_key_violations.iter().take(50) {
        println!("  {violation}");
    }
    Ok(report.healthy())
}

fn command_counts(path: &str) -> Result<bool, String> {
    for (table, count) in counts(&open_read_only(path)?).map_err(|error| error.to_string())? {
        println!("{table} {count}");
    }
    Ok(true)
}

fn backup(source: &Connection, destination: &str) -> Result<(), String> {
    if Path::new(destination).exists() {
        return Err(format!(
            "{destination}: already exists; refusing to overwrite"
        ));
    }
    if let Some(parent) = Path::new(destination).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let mut target = Connection::open(destination).map_err(|error| error.to_string())?;
    let result = copy_all_pages(source, &mut target);
    drop(target);
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

/// Copies every page in one `step(-1)`. A chunked copy restarts each time
/// another connection writes the source, so against a live Yard (a
/// reconciliation write every 500 ms) it never finishes on a large database.
/// One step reads a single WAL snapshot and does not block writers. Busy or
/// locked results are retried until a deadline, then the copy fails.
fn copy_all_pages(source: &Connection, target: &mut Connection) -> Result<(), String> {
    use rusqlite::backup::StepResult;
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    let backup = Backup::new(source, target).map_err(|error| format!("backup failed: {error}"))?;
    loop {
        match backup
            .step(-1)
            .map_err(|error| format!("backup failed: {error}"))?
        {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            _ if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => return Err("backup failed: source stayed busy or locked for 60 s".to_owned()),
        }
    }
}

fn command_backup(source: &str, destination: &str) -> Result<bool, String> {
    backup(&open_read_only(source)?, destination)?;
    let report = check(&open_read_only(destination)?).map_err(|error| error.to_string())?;
    println!(
        "backup {source} -> {destination}: user_version {}, integrity_check {}",
        report.user_version,
        report.integrity.join("; ")
    );
    Ok(report.healthy())
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let result = match args.as_slice() {
        ["open", path] => command_open(path),
        ["check", path] => command_check(path),
        ["counts", path] => command_counts(path),
        ["backup", source, destination] => command_backup(source, destination),
        _ => Err("usage: yard_db_check open|check|counts <db> | backup <src> <dst>".to_owned()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("yard_db_check: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, String) {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp
            .path()
            .join("check.sqlite3")
            .to_string_lossy()
            .into_owned();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA user_version = 7;
                 CREATE TABLE parents (id INTEGER PRIMARY KEY);
                 CREATE TABLE children (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    parent_id INTEGER NOT NULL REFERENCES parents(id)
                 );
                 INSERT INTO parents (id) VALUES (1), (2);
                 INSERT INTO children (parent_id) VALUES (1), (2), (2);",
            )
            .unwrap();
        (temp, path)
    }

    #[test]
    fn counts_lists_every_table_sorted_by_name() {
        let (_temp, path) = temp_db();
        let counts = counts(&open_read_only(&path).unwrap()).unwrap();
        assert_eq!(
            counts.into_iter().collect::<Vec<_>>(),
            [
                ("children".to_owned(), 3),
                ("parents".to_owned(), 2),
                ("sqlite_sequence".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn check_reports_version_integrity_and_foreign_key_orphans() {
        let (_temp, path) = temp_db();
        let healthy = check(&open_read_only(&path).unwrap()).unwrap();
        assert_eq!(healthy.user_version, 7);
        assert_eq!(healthy.integrity, ["ok"]);
        assert!(healthy.foreign_key_violations.is_empty());
        assert!(healthy.healthy());

        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "PRAGMA foreign_keys = OFF; INSERT INTO children (parent_id) VALUES (9);",
            )
            .unwrap();
        let orphaned = check(&open_read_only(&path).unwrap()).unwrap();
        assert_eq!(orphaned.foreign_key_violations, ["children 4 parents 0"]);
        assert!(!orphaned.healthy());
    }

    #[test]
    fn read_only_commands_never_create_a_database() {
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("missing.sqlite3");
        assert!(open_read_only(missing.to_str().unwrap()).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn backup_copies_rows_and_refuses_to_overwrite() {
        let (temp, path) = temp_db();
        let destination = temp
            .path()
            .join("copy.sqlite3")
            .to_string_lossy()
            .into_owned();
        assert!(command_backup(&path, &destination).unwrap());
        assert_eq!(
            counts(&open_read_only(&destination).unwrap()).unwrap(),
            counts(&open_read_only(&path).unwrap()).unwrap()
        );
        assert!(backup(&open_read_only(&path).unwrap(), &destination).is_err());
    }

    #[test]
    fn backup_creates_the_destination_directory() {
        let (temp, path) = temp_db();
        let destination = temp.path().join("data").join("yard.sqlite3");
        assert!(command_backup(&path, destination.to_str().unwrap()).unwrap());
        assert!(destination.exists());
    }

    #[test]
    fn backup_finishes_while_another_connection_keeps_writing() {
        let (temp, path) = temp_db();
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE blobs (id INTEGER PRIMARY KEY, body BLOB NOT NULL);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 4000)
                 INSERT INTO blobs (body) SELECT zeroblob(4000) FROM n;",
            )
            .unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writing = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    writer
                        .execute("UPDATE parents SET id = id WHERE id = 1", [])
                        .unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            })
        };
        let destination = temp.path().join("copy.sqlite3");
        let (sender, receiver) = std::sync::mpsc::channel();
        let source = path.clone();
        let target = destination.to_string_lossy().into_owned();
        std::thread::spawn(move || {
            let _ = sender.send(backup(&open_read_only(&source).unwrap(), &target));
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(20));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writing.join().unwrap();
        result
            .expect("backup did not finish under a concurrent writer")
            .unwrap();
        let copy = check(&open_read_only(destination.to_str().unwrap()).unwrap()).unwrap();
        assert_eq!(copy.integrity, ["ok"]);
    }

    #[test]
    fn count_changes_lists_only_changed_tables() {
        let before = Counts::from([("a".to_owned(), 1), ("b".to_owned(), 2)]);
        let after = Counts::from([
            ("a".to_owned(), 1),
            ("b".to_owned(), 5),
            ("c".to_owned(), 0),
        ]);
        assert_eq!(
            count_changes(&before, &after),
            ["b 2 -> 5 (+3)", "c (new table) 0"]
        );
    }
}

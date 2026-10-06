//! Read-only owner paths for the storage scan (PR3).
//!
//! There is no `project_checkout_paths` table yet, so owners come from the
//! paths Yard already persists: the `cwd` of a workspace-backed project
//! creation and a workstream's `workstream_cwd`. Live Herdr observations of
//! a project's bound workspace supplement them through the returned bindings.

use rusqlite::Connection;
use yard_domain::StorageOwnerKind;

use super::{ProjectStoreError, SqliteProjectStore};

/// A path Yard recorded for a project or workstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOwnerPath {
    pub kind: StorageOwnerKind,
    pub owner_id: String,
    pub owner_name: String,
    /// As recorded (not canonicalized).
    pub path: String,
}

/// A project's current Herdr workspace binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageWorkspaceBinding {
    pub project_id: String,
    pub project_name: String,
    pub adapter: String,
    pub session: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageOwnerRecords {
    pub paths: Vec<StorageOwnerPath>,
    pub bindings: Vec<StorageWorkspaceBinding>,
}

pub(super) async fn owner_records(
    store: &SqliteProjectStore,
) -> Result<StorageOwnerRecords, ProjectStoreError> {
    store
        .run(|connection| select_owner_records(connection))
        .await
}

fn select_owner_records(connection: &Connection) -> Result<StorageOwnerRecords, ProjectStoreError> {
    let mut paths = Vec::new();
    let mut statement = connection.prepare(
        "SELECT project.id, project.name, creation.cwd
           FROM workspace_project_creation_commands creation
           JOIN projects project ON project.id = creation.result_project_id
          ORDER BY project.id",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (owner_id, owner_name, path) = row?;
        paths.push(StorageOwnerPath {
            kind: StorageOwnerKind::Project,
            owner_id,
            owner_name,
            path,
        });
    }
    let mut statement = connection.prepare(
        "SELECT id, name, workstream_cwd
           FROM coordination_nodes
          WHERE workstream_cwd IS NOT NULL
          ORDER BY id",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (owner_id, owner_name, path) = row?;
        paths.push(StorageOwnerPath {
            kind: StorageOwnerKind::CoordinationNode,
            owner_id,
            owner_name,
            path,
        });
    }
    let mut statement = connection.prepare(
        "SELECT binding.project_id, project.name, binding.adapter,
                binding.runtime_session, binding.runtime_workspace_id
           FROM project_workspace_bindings binding
           JOIN projects project ON project.id = binding.project_id
          ORDER BY binding.project_id",
    )?;
    let bindings = statement
        .query_map([], |row| {
            Ok(StorageWorkspaceBinding {
                project_id: row.get(0)?,
                project_name: row.get(1)?,
                adapter: row.get(2)?,
                session: row.get(3)?,
                workspace_id: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StorageOwnerRecords { paths, bindings })
}

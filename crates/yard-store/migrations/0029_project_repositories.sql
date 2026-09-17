CREATE TABLE project_repositories (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    root_path TEXT NOT NULL,
    git_common_dir TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (project_id, root_path)
) STRICT;

CREATE INDEX project_repositories_project
    ON project_repositories(project_id, created_at_unix_ms, id);

PRAGMA user_version = 29;

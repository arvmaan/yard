CREATE TABLE deleted_projects (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE,
    actor TEXT NOT NULL,
    orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    deleted_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE deleted_workers (
    worker_id TEXT PRIMARY KEY NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE,
    actor TEXT NOT NULL,
    expected_worker_version INTEGER NOT NULL
        CHECK (expected_worker_version > 0),
    deleted_at_unix_ms INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 28;

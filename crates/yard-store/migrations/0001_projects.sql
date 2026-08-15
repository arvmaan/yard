CREATE TABLE workers (
    id TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('orchestrator')),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    orchestrator_worker_id TEXT NOT NULL UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE project_workspace_bindings (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE CASCADE,
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    UNIQUE (adapter, runtime_session, runtime_workspace_id)
) STRICT;

CREATE TABLE project_placements (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE CASCADE,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE worker_runtime_bindings (
    worker_id TEXT PRIMARY KEY NOT NULL
        REFERENCES workers(id) ON DELETE CASCADE,
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    terminal_id TEXT NOT NULL,
    pane_id TEXT NOT NULL,
    provider_session_source TEXT,
    provider_session_provider TEXT,
    provider_session_kind TEXT,
    provider_session_value TEXT,
    last_observed_at_unix_ms INTEGER NOT NULL,
    UNIQUE (adapter, runtime_session, terminal_id),
    CHECK (
        (
            provider_session_source IS NULL
            AND provider_session_provider IS NULL
            AND provider_session_kind IS NULL
            AND provider_session_value IS NULL
        )
        OR
        (
            provider_session_source IS NOT NULL
            AND provider_session_provider IS NOT NULL
            AND provider_session_kind IS NOT NULL
            AND provider_session_value IS NOT NULL
        )
    )
) STRICT;

PRAGMA user_version = 1;

CREATE TABLE workspace_project_creation_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_name TEXT NOT NULL,
    runtime_adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    workspace_label TEXT NOT NULL,
    cwd TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    orchestrator_objective TEXT NOT NULL,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    result_runtime_workspace_id TEXT,
    result_project_id TEXT UNIQUE
        REFERENCES projects(id) ON DELETE RESTRICT,
    result_worker_id TEXT UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    finished_at_unix_ms INTEGER,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT,
    CHECK (
        (
            result_runtime_workspace_id IS NULL
            AND result_project_id IS NULL
            AND result_worker_id IS NULL
        )
        OR (
            result_runtime_workspace_id IS NOT NULL
            AND result_project_id IS NOT NULL
            AND result_worker_id IS NOT NULL
        )
    )
) STRICT;

CREATE UNIQUE INDEX one_unfinished_workspace_project_creation_per_cwd
ON workspace_project_creation_commands (
    runtime_adapter,
    runtime_session,
    cwd
)
WHERE finished_at_unix_ms IS NULL;

PRAGMA user_version = 12;

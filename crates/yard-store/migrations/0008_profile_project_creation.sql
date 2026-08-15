CREATE TABLE command_acknowledgements_v8 (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL
        CHECK (
            command_type IN (
                'profile_allocation',
                'worker_allocation',
                'profile_project_creation',
                'completion_receipt',
                'assignment_prompt'
            )
        ),
    actor TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'succeeded', 'failed', 'ambiguous')),
    error_message TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (status IN ('failed', 'ambiguous') AND error_message IS NOT NULL)
        OR (status NOT IN ('failed', 'ambiguous') AND error_message IS NULL)
    )
) STRICT;

INSERT INTO command_acknowledgements_v8
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v8
    RENAME TO command_acknowledgements;

CREATE TABLE profile_project_creation_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_name TEXT NOT NULL,
    runtime_adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    orchestrator_objective TEXT NOT NULL,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    result_project_id TEXT UNIQUE
        REFERENCES projects(id) ON DELETE RESTRICT,
    result_worker_id TEXT UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    finished_at_unix_ms INTEGER,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT,
    CHECK (
        (result_project_id IS NULL AND result_worker_id IS NULL)
        OR (result_project_id IS NOT NULL AND result_worker_id IS NOT NULL)
    )
) STRICT;

CREATE UNIQUE INDEX one_unfinished_project_creation_per_workspace
ON profile_project_creation_commands (
    runtime_adapter,
    runtime_session,
    runtime_workspace_id
)
WHERE finished_at_unix_ms IS NULL;

PRAGMA user_version = 8;

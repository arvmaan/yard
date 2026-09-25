ALTER TABLE worker_cleanup_run_items
ADD COLUMN pane_instance_id TEXT;

CREATE TABLE summary_worker_commands (
    command_id TEXT PRIMARY KEY NOT NULL,
    actor TEXT NOT NULL,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    parent_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_parent_worker_version INTEGER NOT NULL CHECK (expected_parent_worker_version > 0),
    expected_project_version INTEGER NOT NULL CHECK (expected_project_version > 0),
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    expected_artifact_id TEXT NOT NULL UNIQUE,
    objective TEXT NOT NULL,
    captured_adapter TEXT NOT NULL,
    captured_runtime_session TEXT NOT NULL,
    captured_workspace_id TEXT NOT NULL,
    captured_terminal_id TEXT NOT NULL,
    captured_tab_id TEXT NOT NULL,
    captured_pane_id TEXT NOT NULL,
    captured_pane_instance_id TEXT,
    captured_provider_session_json TEXT
        CHECK (
            captured_provider_session_json IS NULL
            OR json_valid(captured_provider_session_json)
        ),
    status TEXT NOT NULL CHECK (status IN ('allocating', 'active', 'failed')),
    result_worker_id TEXT UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_assignment_id TEXT UNIQUE
        REFERENCES assignments(id) ON DELETE RESTRICT,
    handoff_command_id TEXT UNIQUE,
    cleanup_run_id TEXT UNIQUE
        REFERENCES worker_cleanup_runs(id) ON DELETE RESTRICT,
    error_message TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (status = 'allocating' AND result_worker_id IS NULL AND result_assignment_id IS NULL)
        OR
        (status = 'active' AND result_worker_id IS NOT NULL AND result_assignment_id IS NOT NULL)
        OR
        (status = 'failed' AND error_message IS NOT NULL)
    ),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE INDEX summary_workers_by_parent
ON summary_worker_commands(project_id, parent_worker_id, created_at_unix_ms);

PRAGMA user_version = 33;

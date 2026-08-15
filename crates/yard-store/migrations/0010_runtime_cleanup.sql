CREATE TABLE runtime_cleanup_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    handoff_command_id TEXT NOT NULL
        REFERENCES worker_handoff_commands(command_id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL
        CHECK (reason IN ('handoff_source', 'replaced_orchestrator')),
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    terminal_id TEXT NOT NULL,
    tab_id TEXT,
    pane_id TEXT NOT NULL,
    owns_tab INTEGER NOT NULL CHECK (owns_tab IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending', 'succeeded')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    next_attempt_at_unix_ms INTEGER NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    completed_at_unix_ms INTEGER,
    UNIQUE (adapter, runtime_session, terminal_id),
    CHECK (
        (status = 'pending' AND completed_at_unix_ms IS NULL)
        OR (status = 'succeeded' AND completed_at_unix_ms IS NOT NULL)
    ),
    CHECK (owns_tab = 0 OR tab_id IS NOT NULL)
) STRICT;

CREATE INDEX pending_runtime_cleanup_jobs
ON runtime_cleanup_jobs(next_attempt_at_unix_ms, created_at_unix_ms)
WHERE status = 'pending';

PRAGMA user_version = 10;

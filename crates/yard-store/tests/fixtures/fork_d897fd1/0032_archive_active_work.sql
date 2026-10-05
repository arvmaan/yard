-- One project archive can end several workers: the orchestrator and every
-- active member it records as cancelled. Each ended worker gets its own
-- retired binding under the archive's command ID, so a retirement is unique
-- per (command, worker) instead of per command. Every other column, CHECK,
-- and the identity UNIQUE are kept verbatim from 0027.
CREATE TABLE retired_runtime_bindings_v32 (
    id TEXT PRIMARY KEY NOT NULL,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL
        CHECK (
            reason IN (
                'handoff_source',
                'worker_session_end',
                'project_archive'
            )
        ),
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    terminal_id TEXT NOT NULL,
    tab_id TEXT,
    pane_id TEXT NOT NULL,
    provider_session_source TEXT,
    provider_session_provider TEXT,
    provider_session_kind TEXT,
    provider_session_value TEXT,
    retired_at_unix_ms INTEGER NOT NULL,
    UNIQUE (command_id, worker_id),
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

INSERT INTO retired_runtime_bindings_v32 (
    id, worker_id, command_id, reason, adapter, runtime_session,
    runtime_workspace_id, terminal_id, tab_id, pane_id,
    provider_session_source, provider_session_provider,
    provider_session_kind, provider_session_value, retired_at_unix_ms
)
SELECT id, worker_id, command_id, reason, adapter, runtime_session,
       runtime_workspace_id, terminal_id, tab_id, pane_id,
       provider_session_source, provider_session_provider,
       provider_session_kind, provider_session_value, retired_at_unix_ms
FROM retired_runtime_bindings;

DROP TABLE retired_runtime_bindings;
ALTER TABLE retired_runtime_bindings_v32
    RENAME TO retired_runtime_bindings;

CREATE INDEX retired_runtime_provider_sessions
ON retired_runtime_bindings (
    adapter,
    runtime_session,
    provider_session_source,
    provider_session_provider,
    provider_session_kind,
    provider_session_value
);

-- The archive inputs that replay compares (D8). Existing archives were all
-- made by clients that could only reject active work.
ALTER TABLE archived_projects
    ADD COLUMN active_work TEXT NOT NULL DEFAULT 'reject'
        CHECK (active_work IN ('reject', 'cancel'));

-- The canonical (sorted) JSON list of {assignment_id,
-- expected_assignment_version}; present exactly when active_work = cancel.
ALTER TABLE archived_projects
    ADD COLUMN expected_active_assignments_json TEXT
        CHECK (
            (active_work = 'cancel') = (expected_active_assignments_json IS NOT NULL)
        );

PRAGMA user_version = 32;

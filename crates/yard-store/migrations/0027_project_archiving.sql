CREATE TABLE command_acknowledgements_v27 (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL
        CHECK (
            command_type IN (
                'profile_allocation',
                'worker_allocation',
                'worker_handoff',
                'profile_project_creation',
                'completion_receipt',
                'assignment_prompt',
                'orchestrator_prompt',
                'worker_session_end',
                'yard_orchestrator_configure',
                'yard_orchestrator_prompt',
                'project_relationship_create',
                'project_relationship_delete',
                'yard_orchestrator_route',
                'coordination_node_create',
                'coordination_node_update',
                'coordination_node_placement',
                'coordination_node_provision',
                'coordination_node_prompt',
                'coordination_node_route',
                'coordination_snapshot_request',
                'automation_create',
                'automation_update',
                'automation_placement',
                'automation_pause',
                'automation_run_now',
                'project_orchestrator_replacement',
                'project_orchestrator_transfer',
                'project_archive'
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

INSERT INTO command_acknowledgements_v27
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v27
    RENAME TO command_acknowledgements;

CREATE TABLE retired_runtime_bindings_v27 (
    id TEXT PRIMARY KEY NOT NULL,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE
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

INSERT INTO retired_runtime_bindings_v27 (
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
ALTER TABLE retired_runtime_bindings_v27
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

CREATE TABLE runtime_cleanup_jobs_v27 (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL
        CHECK (
            reason IN (
                'handoff_source',
                'replaced_orchestrator',
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
    owns_tab INTEGER NOT NULL CHECK (owns_tab IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending', 'succeeded')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    next_attempt_at_unix_ms INTEGER NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    completed_at_unix_ms INTEGER,
    provider_session_source TEXT,
    provider_session_provider TEXT,
    provider_session_kind TEXT,
    provider_session_value TEXT,
    expected_worker_version INTEGER
        CHECK (expected_worker_version IS NULL OR expected_worker_version > 0),
    expected_binding_state TEXT
        CHECK (
            expected_binding_state IS NULL
            OR expected_binding_state IN ('detached', 'replaced')
        ),
    claim_token TEXT,
    claim_expires_at_unix_ms INTEGER,
    CHECK (
        (status = 'pending' AND completed_at_unix_ms IS NULL)
        OR (status = 'succeeded' AND completed_at_unix_ms IS NOT NULL)
    ),
    CHECK (owns_tab = 0 OR tab_id IS NOT NULL),
    CHECK (
        (
            provider_session_source IS NULL
            AND provider_session_provider IS NULL
            AND provider_session_kind IS NULL
            AND provider_session_value IS NULL
        )
        OR (
            provider_session_source IS NOT NULL
            AND provider_session_provider IS NOT NULL
            AND provider_session_kind IS NOT NULL
            AND provider_session_value IS NOT NULL
        )
    ),
    CHECK (
        (
            claim_token IS NULL
            AND claim_expires_at_unix_ms IS NULL
        )
        OR (
            status = 'pending'
            AND claim_token IS NOT NULL
            AND claim_expires_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

INSERT INTO runtime_cleanup_jobs_v27 (
    id, command_id, worker_id, reason, adapter, runtime_session,
    runtime_workspace_id, terminal_id, tab_id, pane_id, owns_tab,
    status, attempts, last_error, next_attempt_at_unix_ms,
    created_at_unix_ms, updated_at_unix_ms, completed_at_unix_ms,
    provider_session_source, provider_session_provider,
    provider_session_kind, provider_session_value,
    expected_worker_version, expected_binding_state,
    claim_token, claim_expires_at_unix_ms
)
SELECT id, command_id, worker_id, reason, adapter, runtime_session,
       runtime_workspace_id, terminal_id, tab_id, pane_id, owns_tab,
       status, attempts, last_error, next_attempt_at_unix_ms,
       created_at_unix_ms, updated_at_unix_ms, completed_at_unix_ms,
       provider_session_source, provider_session_provider,
       provider_session_kind, provider_session_value,
       expected_worker_version, expected_binding_state,
       claim_token, claim_expires_at_unix_ms
FROM runtime_cleanup_jobs;

DROP TABLE runtime_cleanup_jobs;
ALTER TABLE runtime_cleanup_jobs_v27
    RENAME TO runtime_cleanup_jobs;

CREATE INDEX pending_runtime_cleanup_jobs
ON runtime_cleanup_jobs(next_attempt_at_unix_ms, created_at_unix_ms)
WHERE status = 'pending';

CREATE UNIQUE INDEX claimed_runtime_cleanup_tokens
ON runtime_cleanup_jobs(claim_token)
WHERE claim_token IS NOT NULL;

CREATE TABLE archived_projects (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    expected_project_version INTEGER NOT NULL
        CHECK (expected_project_version > 0),
    expected_orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_orchestrator_worker_version INTEGER NOT NULL
        CHECK (expected_orchestrator_worker_version > 0),
    expected_orchestrator_runtime_version INTEGER
        CHECK (
            expected_orchestrator_runtime_version IS NULL
            OR expected_orchestrator_runtime_version > 0
        ),
    result_project_version INTEGER NOT NULL
        CHECK (result_project_version > 0),
    result_orchestrator_worker_version INTEGER NOT NULL
        CHECK (result_orchestrator_worker_version > 0),
    runtime_adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    archived_at_unix_ms INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 27;

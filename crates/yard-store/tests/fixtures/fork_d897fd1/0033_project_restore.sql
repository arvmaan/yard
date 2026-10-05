-- Restore (Undo for archive).
--
-- A project can now be archived, restored, and archived again, so
-- `archived_projects` is keyed by the archive command and at most one row
-- per project is current (not restored). Restore cancels the orchestrator's
-- archive cleanup job, may re-bind its retired runtime (releasing the
-- retirement), and records its inputs for replay in
-- `project_restore_commands`.

CREATE TABLE command_acknowledgements_v33 (
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
                'project_archive',
                'project_delete',
                'worker_delete',
                'coordination_node_archive',
                'coordination_node_delete',
                'assignment_disposition',
                'project_restore'
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

INSERT INTO command_acknowledgements_v33
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v33
    RENAME TO command_acknowledgements;

-- Restore cancels the orchestrator's archive cleanup job; a cancelled job
-- is terminal like a succeeded one and never holds a claim.
CREATE TABLE runtime_cleanup_jobs_v33 (
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
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'succeeded', 'cancelled')),
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
        OR (status = 'cancelled' AND completed_at_unix_ms IS NOT NULL)
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

INSERT INTO runtime_cleanup_jobs_v33 (
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
ALTER TABLE runtime_cleanup_jobs_v33
    RENAME TO runtime_cleanup_jobs;

CREATE INDEX pending_runtime_cleanup_jobs
ON runtime_cleanup_jobs(next_attempt_at_unix_ms, created_at_unix_ms)
WHERE status = 'pending';

CREATE UNIQUE INDEX claimed_runtime_cleanup_tokens
ON runtime_cleanup_jobs(claim_token)
WHERE claim_token IS NOT NULL;

-- Restore releases the retirement of a runtime it re-binds, so the same
-- identity can be retired again by a later archive.
CREATE TABLE retired_runtime_bindings_v33 (
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
    released_at_unix_ms INTEGER,
    released_by_command_id TEXT
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    UNIQUE (command_id, worker_id),
    CHECK ((released_at_unix_ms IS NULL) = (released_by_command_id IS NULL)),
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

INSERT INTO retired_runtime_bindings_v33 (
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
ALTER TABLE retired_runtime_bindings_v33
    RENAME TO retired_runtime_bindings;

-- A retired identity reserves its runtime until Restore re-binds it; only
-- unreleased rows keep the one-retirement-per-identity rule.
CREATE UNIQUE INDEX unreleased_retired_runtime_identities
ON retired_runtime_bindings (adapter, runtime_session, terminal_id)
WHERE released_at_unix_ms IS NULL;

CREATE INDEX retired_runtime_provider_sessions
ON retired_runtime_bindings (
    adapter,
    runtime_session,
    provider_session_source,
    provider_session_provider,
    provider_session_kind,
    provider_session_value
);

-- Keyed by the archive command: every column, CHECK, and default of the
-- 0027 table plus the 0032 archive inputs, and the restore that ended it.
CREATE TABLE archived_projects_v33 (
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    command_id TEXT PRIMARY KEY NOT NULL
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
    archived_at_unix_ms INTEGER NOT NULL,
    active_work TEXT NOT NULL DEFAULT 'reject'
        CHECK (active_work IN ('reject', 'cancel')),
    expected_active_assignments_json TEXT,
    restore_command_id TEXT UNIQUE
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    restored_at_unix_ms INTEGER,
    CHECK (
        (active_work = 'cancel') = (expected_active_assignments_json IS NOT NULL)
    ),
    CHECK ((restore_command_id IS NULL) = (restored_at_unix_ms IS NULL))
) STRICT;

INSERT INTO archived_projects_v33 (
    project_id, command_id, expected_project_version,
    expected_orchestrator_worker_id, expected_orchestrator_worker_version,
    expected_orchestrator_runtime_version, result_project_version,
    result_orchestrator_worker_version, runtime_adapter, runtime_session,
    runtime_workspace_id, archived_at_unix_ms, active_work,
    expected_active_assignments_json, restore_command_id,
    restored_at_unix_ms
)
SELECT project_id, command_id, expected_project_version,
       expected_orchestrator_worker_id, expected_orchestrator_worker_version,
       expected_orchestrator_runtime_version, result_project_version,
       result_orchestrator_worker_version, runtime_adapter, runtime_session,
       runtime_workspace_id, archived_at_unix_ms, active_work,
       expected_active_assignments_json, NULL, NULL
FROM archived_projects;

DROP TABLE archived_projects;
ALTER TABLE archived_projects_v33 RENAME TO archived_projects;

-- The one current (not restored) archive of a project.
CREATE UNIQUE INDEX current_archived_projects
ON archived_projects (project_id)
WHERE restored_at_unix_ms IS NULL;


-- Restore's inputs (D8: project, actor via the ack, expected archive) and
-- what it did, so an identical retry replays.
CREATE TABLE project_restore_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    expected_archive_command_id TEXT NOT NULL UNIQUE
        REFERENCES archived_projects(command_id) ON DELETE RESTRICT,
    orchestrator_runtime TEXT NOT NULL
        CHECK (orchestrator_runtime IN ('rebound', 'unbound')),
    allocation_id TEXT NOT NULL UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    result_project_version INTEGER NOT NULL
        CHECK (result_project_version > 0),
    result_orchestrator_worker_version INTEGER NOT NULL
        CHECK (result_orchestrator_worker_version > 0),
    restored_at_unix_ms INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 33;

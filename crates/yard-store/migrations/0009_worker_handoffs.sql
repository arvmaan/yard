CREATE TABLE command_acknowledgements_v9 (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL
        CHECK (
            command_type IN (
                'profile_allocation',
                'worker_allocation',
                'worker_handoff',
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

INSERT INTO command_acknowledgements_v9
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v9
    RENAME TO command_acknowledgements;

CREATE TABLE worker_allocations_v9 (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    mode TEXT NOT NULL
        CHECK (mode IN ('adopt_existing', 'create_new', 'handoff')),
    started_by_command_id TEXT
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    started_at_unix_ms INTEGER NOT NULL,
    ended_at_unix_ms INTEGER
) STRICT;

INSERT INTO worker_allocations_v9
SELECT id, project_id, worker_id, mode, started_by_command_id,
       started_at_unix_ms, ended_at_unix_ms
FROM worker_allocations;

DROP TABLE worker_allocations;
ALTER TABLE worker_allocations_v9 RENAME TO worker_allocations;

CREATE UNIQUE INDEX one_active_allocation_per_worker
ON worker_allocations(worker_id)
WHERE ended_at_unix_ms IS NULL;

CREATE TABLE assignments_v9 (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    allocation_id TEXT NOT NULL UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL,
    objective TEXT NOT NULL,
    role TEXT NOT NULL,
    isolation_policy TEXT NOT NULL
        CHECK (isolation_policy IN ('project_workspace')),
    lifecycle TEXT NOT NULL
        CHECK (
            lifecycle IN (
                'allocating',
                'active',
                'handing_off',
                'handed_off',
                'completed',
                'failed'
            )
        ),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (id, project_id),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO assignments_v9
SELECT id, project_id, allocation_id, worker_id, profile_id, profile_version,
       objective, role, isolation_policy, lifecycle, version,
       created_at_unix_ms, updated_at_unix_ms
FROM assignments;

DROP TABLE assignments;
ALTER TABLE assignments_v9 RENAME TO assignments;

CREATE UNIQUE INDEX one_open_assignment_per_worker
ON assignments(worker_id)
WHERE lifecycle IN ('allocating', 'active', 'handing_off');

CREATE TABLE assignment_attempts_v9 (
    id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL
        REFERENCES assignments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    lifecycle TEXT NOT NULL
        CHECK (
            lifecycle IN (
                'starting',
                'active',
                'handing_off',
                'handed_off',
                'completed',
                'failed'
            )
        ),
    error_message TEXT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (id, assignment_id),
    UNIQUE (assignment_id, ordinal),
    CHECK (
        (lifecycle = 'failed' AND error_message IS NOT NULL)
        OR (lifecycle <> 'failed' AND error_message IS NULL)
    )
) STRICT;

INSERT INTO assignment_attempts_v9
SELECT id, assignment_id, ordinal, lifecycle, error_message, version,
       created_at_unix_ms, updated_at_unix_ms
FROM assignment_attempts;

DROP TABLE assignment_attempts;
ALTER TABLE assignment_attempts_v9 RENAME TO assignment_attempts;

CREATE TABLE worker_handoff_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    source_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    source_assignment_id TEXT NOT NULL,
    source_attempt_id TEXT NOT NULL,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_worker_version INTEGER NOT NULL CHECK (expected_worker_version > 0),
    expected_source_project_version INTEGER NOT NULL
        CHECK (expected_source_project_version > 0),
    expected_source_assignment_version INTEGER NOT NULL
        CHECK (expected_source_assignment_version > 0),
    expected_source_attempt_version INTEGER NOT NULL
        CHECK (expected_source_attempt_version > 0),
    target_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    expected_target_project_version INTEGER NOT NULL
        CHECK (expected_target_project_version > 0),
    target_role TEXT NOT NULL CHECK (target_role IN ('member', 'orchestrator')),
    objective TEXT NOT NULL,
    role TEXT NOT NULL,
    isolation_policy TEXT NOT NULL
        CHECK (isolation_policy IN ('project_workspace')),
    source_runtime_adapter TEXT NOT NULL,
    source_runtime_session TEXT NOT NULL,
    source_runtime_workspace_id TEXT NOT NULL,
    source_terminal_id TEXT NOT NULL,
    source_tab_id TEXT,
    source_pane_id TEXT NOT NULL,
    source_provider_session_source TEXT,
    source_provider_session_provider TEXT,
    source_provider_session_kind TEXT,
    source_provider_session_value TEXT,
    target_runtime_adapter TEXT,
    target_runtime_session TEXT,
    target_runtime_workspace_id TEXT,
    target_terminal_id TEXT,
    target_tab_id TEXT,
    target_pane_id TEXT,
    target_provider_session_source TEXT,
    target_provider_session_provider TEXT,
    target_provider_session_kind TEXT,
    target_provider_session_value TEXT,
    target_runtime_claimed_at_unix_ms INTEGER,
    replaced_orchestrator_worker_id TEXT
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_allocation_id TEXT UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    result_assignment_id TEXT UNIQUE
        REFERENCES assignments(id) ON DELETE RESTRICT,
    finished_at_unix_ms INTEGER,
    FOREIGN KEY (source_assignment_id, source_project_id)
        REFERENCES assignments(id, project_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_attempt_id, source_assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT,
    CHECK (source_project_id <> target_project_id),
    CHECK (
        (target_role = 'member' AND replaced_orchestrator_worker_id IS NULL)
        OR (
            target_role = 'orchestrator'
            AND replaced_orchestrator_worker_id IS NOT NULL
        )
    ),
    CHECK (
        (
            source_provider_session_source IS NULL
            AND source_provider_session_provider IS NULL
            AND source_provider_session_kind IS NULL
            AND source_provider_session_value IS NULL
        )
        OR
        (
            source_provider_session_source IS NOT NULL
            AND source_provider_session_provider IS NOT NULL
            AND source_provider_session_kind IS NOT NULL
            AND source_provider_session_value IS NOT NULL
        )
    ),
    CHECK (
        (
            target_runtime_adapter IS NULL
            AND target_runtime_session IS NULL
            AND target_runtime_workspace_id IS NULL
            AND target_terminal_id IS NULL
            AND target_tab_id IS NULL
            AND target_pane_id IS NULL
            AND target_provider_session_source IS NULL
            AND target_provider_session_provider IS NULL
            AND target_provider_session_kind IS NULL
            AND target_provider_session_value IS NULL
            AND target_runtime_claimed_at_unix_ms IS NULL
        )
        OR
        (
            target_runtime_adapter IS NOT NULL
            AND target_runtime_session IS NOT NULL
            AND target_runtime_workspace_id IS NOT NULL
            AND target_terminal_id IS NOT NULL
            AND target_pane_id IS NOT NULL
            AND target_runtime_claimed_at_unix_ms IS NOT NULL
        )
    ),
    CHECK (
        (
            target_provider_session_source IS NULL
            AND target_provider_session_provider IS NULL
            AND target_provider_session_kind IS NULL
            AND target_provider_session_value IS NULL
        )
        OR
        (
            target_provider_session_source IS NOT NULL
            AND target_provider_session_provider IS NOT NULL
            AND target_provider_session_kind IS NOT NULL
            AND target_provider_session_value IS NOT NULL
        )
    ),
    CHECK (
        (result_allocation_id IS NULL AND result_assignment_id IS NULL)
        OR (result_allocation_id IS NOT NULL AND result_assignment_id IS NOT NULL)
    )
) STRICT;

CREATE UNIQUE INDEX one_unfinished_orchestrator_handoff_per_project
ON worker_handoff_commands(target_project_id)
WHERE target_role = 'orchestrator' AND finished_at_unix_ms IS NULL;

CREATE UNIQUE INDEX one_unfinished_handoff_per_worker
ON worker_handoff_commands(worker_id)
WHERE finished_at_unix_ms IS NULL;

CREATE UNIQUE INDEX one_claim_per_runtime_terminal
ON worker_handoff_commands (
    target_runtime_adapter,
    target_runtime_session,
    target_terminal_id
)
WHERE target_terminal_id IS NOT NULL AND finished_at_unix_ms IS NULL;

CREATE TABLE retired_runtime_bindings (
    id TEXT PRIMARY KEY NOT NULL,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    handoff_command_id TEXT NOT NULL UNIQUE
        REFERENCES worker_handoff_commands(command_id) ON DELETE RESTRICT,
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

CREATE INDEX retired_runtime_provider_sessions
ON retired_runtime_bindings (
    adapter,
    runtime_session,
    provider_session_source,
    provider_session_provider,
    provider_session_kind,
    provider_session_value
);

PRAGMA user_version = 9;

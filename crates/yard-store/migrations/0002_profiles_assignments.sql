CREATE TABLE worker_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    current_version INTEGER NOT NULL CHECK (current_version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE worker_profile_revisions (
    profile_id TEXT NOT NULL
        REFERENCES worker_profiles(id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version > 0),
    name TEXT NOT NULL,
    runtime_adapter TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT,
    default_role TEXT NOT NULL,
    instructions_ref TEXT,
    sandbox_policy TEXT NOT NULL,
    worktree_policy TEXT NOT NULL,
    permission_policy TEXT NOT NULL,
    completion_contract TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (profile_id, version)
) STRICT;

CREATE TABLE worker_profile_tools (
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    value TEXT NOT NULL,
    PRIMARY KEY (profile_id, profile_version, position),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE worker_profile_skills (
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    value TEXT NOT NULL,
    PRIMARY KEY (profile_id, profile_version, position),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE worker_profile_mcp_servers (
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    value TEXT NOT NULL,
    PRIMARY KEY (profile_id, profile_version, position),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE workers_v2 (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT,
    profile_version INTEGER,
    desired_state TEXT NOT NULL CHECK (desired_state IN ('running')),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT,
    CHECK (
        (profile_id IS NULL AND profile_version IS NULL)
        OR (profile_id IS NOT NULL AND profile_version IS NOT NULL)
    )
) STRICT;

INSERT INTO workers_v2 (
    id, profile_id, profile_version, desired_state, version,
    created_at_unix_ms, updated_at_unix_ms
)
SELECT id, NULL, NULL, 'running', 1, created_at_unix_ms, updated_at_unix_ms
FROM workers;

CREATE TABLE projects_v2 (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    orchestrator_worker_id TEXT NOT NULL UNIQUE
        REFERENCES workers_v2(id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

INSERT INTO projects_v2
SELECT id, name, orchestrator_worker_id, version,
       created_at_unix_ms, updated_at_unix_ms
FROM projects;

CREATE TABLE project_workspace_bindings_v2 (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects_v2(id) ON DELETE CASCADE,
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    UNIQUE (adapter, runtime_session, runtime_workspace_id)
) STRICT;

INSERT INTO project_workspace_bindings_v2
SELECT project_id, adapter, runtime_session, runtime_workspace_id
FROM project_workspace_bindings;

CREATE TABLE project_placements_v2 (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects_v2(id) ON DELETE CASCADE,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

INSERT INTO project_placements_v2
SELECT project_id, canvas_x, canvas_y, canvas_width, canvas_height,
       version, updated_at_unix_ms
FROM project_placements;

CREATE TABLE worker_runtime_bindings_v2 (
    worker_id TEXT PRIMARY KEY NOT NULL
        REFERENCES workers_v2(id) ON DELETE CASCADE,
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
    owns_tab INTEGER NOT NULL CHECK (owns_tab IN (0, 1)),
    observation_state TEXT NOT NULL
        CHECK (observation_state IN ('observed', 'missing', 'ambiguous')),
    version INTEGER NOT NULL CHECK (version > 0),
    last_observed_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
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

INSERT INTO worker_runtime_bindings_v2 (
    worker_id, adapter, runtime_session, runtime_workspace_id,
    terminal_id, tab_id, pane_id, provider_session_source,
    provider_session_provider, provider_session_kind,
    provider_session_value, owns_tab, observation_state, version,
    last_observed_at_unix_ms, updated_at_unix_ms
)
SELECT wrb.worker_id, wrb.adapter, wrb.runtime_session,
       wrb.runtime_workspace_id, wrb.terminal_id, NULL, wrb.pane_id,
       wrb.provider_session_source, wrb.provider_session_provider,
       wrb.provider_session_kind, wrb.provider_session_value, 0,
       'observed', 1, wrb.last_observed_at_unix_ms,
       w.updated_at_unix_ms
FROM worker_runtime_bindings wrb
JOIN workers w ON w.id = wrb.worker_id;

DROP TABLE project_placements;
DROP TABLE project_workspace_bindings;
DROP TABLE worker_runtime_bindings;
DROP TABLE projects;
DROP TABLE workers;

ALTER TABLE workers_v2 RENAME TO workers;
ALTER TABLE projects_v2 RENAME TO projects;
ALTER TABLE project_workspace_bindings_v2
    RENAME TO project_workspace_bindings;
ALTER TABLE project_placements_v2 RENAME TO project_placements;
ALTER TABLE worker_runtime_bindings_v2 RENAME TO worker_runtime_bindings;

CREATE TABLE command_acknowledgements (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL CHECK (command_type IN ('profile_allocation')),
    actor TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'succeeded', 'failed')),
    error_message TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (status = 'failed' AND error_message IS NOT NULL)
        OR (status <> 'failed' AND error_message IS NULL)
    )
) STRICT;

CREATE TABLE profile_allocation_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL,
    expected_project_version INTEGER NOT NULL CHECK (expected_project_version > 0),
    objective TEXT NOT NULL,
    role TEXT NOT NULL,
    isolation_policy TEXT NOT NULL CHECK (isolation_policy IN ('project_workspace')),
    result_worker_id TEXT,
    result_allocation_id TEXT,
    result_assignment_id TEXT,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE worker_allocations (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    mode TEXT NOT NULL CHECK (mode IN ('adopt_existing', 'create_new')),
    started_by_command_id TEXT
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    started_at_unix_ms INTEGER NOT NULL,
    ended_at_unix_ms INTEGER
) STRICT;

CREATE UNIQUE INDEX one_active_allocation_per_worker
ON worker_allocations(worker_id)
WHERE ended_at_unix_ms IS NULL;

INSERT INTO worker_allocations (
    id, project_id, worker_id, mode, started_by_command_id,
    started_at_unix_ms, ended_at_unix_ms
)
SELECT 'migration:orchestrator:' || p.id, p.id, p.orchestrator_worker_id,
       'adopt_existing', NULL, p.created_at_unix_ms, NULL
FROM projects p;

CREATE TABLE assignments (
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
    isolation_policy TEXT NOT NULL CHECK (isolation_policy IN ('project_workspace')),
    lifecycle TEXT NOT NULL
        CHECK (lifecycle IN ('allocating', 'active', 'failed')),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX one_open_assignment_per_worker
ON assignments(worker_id)
WHERE lifecycle IN ('allocating', 'active');

CREATE TABLE assignment_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL
        REFERENCES assignments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('starting', 'active', 'failed')),
    error_message TEXT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (assignment_id, ordinal),
    CHECK (
        (lifecycle = 'failed' AND error_message IS NOT NULL)
        OR (lifecycle <> 'failed' AND error_message IS NULL)
    )
) STRICT;

CREATE TABLE lifecycle_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    aggregate_version INTEGER NOT NULL CHECK (aggregate_version > 0),
    event_type TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 2;

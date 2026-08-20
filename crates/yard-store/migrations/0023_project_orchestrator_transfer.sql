CREATE TABLE command_acknowledgements_v23 (
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
                'project_orchestrator_transfer'
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

INSERT INTO command_acknowledgements_v23
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v23
    RENAME TO command_acknowledgements;

CREATE TABLE project_orchestrator_transfer_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_worker_version INTEGER NOT NULL
        CHECK (expected_worker_version > 0),
    expected_project_version INTEGER NOT NULL
        CHECK (expected_project_version > 0),
    expected_orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_orchestrator_worker_version INTEGER NOT NULL
        CHECK (expected_orchestrator_worker_version > 0),
    replaced_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_project_version INTEGER NOT NULL
        CHECK (result_project_version > 0),
    result_worker_version INTEGER NOT NULL
        CHECK (result_worker_version > 0),
    result_replaced_worker_version INTEGER NOT NULL
        CHECK (result_replaced_worker_version > 0),
    result_allocation_id TEXT NOT NULL UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    result_project_json TEXT NOT NULL,
    finished_at_unix_ms INTEGER NOT NULL,
    CHECK (worker_id <> expected_orchestrator_worker_id),
    CHECK (replaced_worker_id = expected_orchestrator_worker_id)
) STRICT;

CREATE TABLE project_orchestrator_transfer_runtime_bindings (
    command_id TEXT NOT NULL
        REFERENCES project_orchestrator_transfer_commands(command_id)
        ON DELETE RESTRICT,
    binding_role TEXT NOT NULL
        CHECK (binding_role IN ('candidate', 'displaced')),
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
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
    process_state TEXT NOT NULL
        CHECK (process_state IN ('running', 'exited', 'unknown')),
    observed_status TEXT NOT NULL
        CHECK (observed_status IN ('idle', 'working', 'blocked', 'done', 'unknown')),
    state_change_sequence INTEGER NOT NULL CHECK (state_change_sequence >= 0),
    runtime_revision INTEGER NOT NULL CHECK (runtime_revision >= 0),
    runtime_version INTEGER NOT NULL CHECK (runtime_version > 0),
    last_observed_at_unix_ms INTEGER NOT NULL
        CHECK (last_observed_at_unix_ms >= 0),
    PRIMARY KEY (command_id, binding_role),
    UNIQUE (command_id, worker_id),
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
    )
) STRICT;

PRAGMA user_version = 23;

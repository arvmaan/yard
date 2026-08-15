CREATE TABLE command_acknowledgements_v16 (
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
                'yard_orchestrator_route'
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

INSERT INTO command_acknowledgements_v16
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v16
    RENAME TO command_acknowledgements;

CREATE TABLE project_relationships (
    id TEXT PRIMARY KEY NOT NULL,
    source_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    target_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('depends_on')),
    version INTEGER NOT NULL CHECK (version > 0),
    created_by TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (source_project_id, target_project_id, kind),
    CHECK (source_project_id <> target_project_id)
) STRICT;

CREATE TABLE project_relationship_create_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    relationship_id TEXT NOT NULL UNIQUE,
    source_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    target_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('depends_on')),
    result_version INTEGER NOT NULL CHECK (result_version > 0),
    result_created_by TEXT NOT NULL,
    result_created_at_unix_ms INTEGER NOT NULL,
    result_updated_at_unix_ms INTEGER NOT NULL,
    CHECK (source_project_id <> target_project_id)
) STRICT;

CREATE TABLE project_relationship_delete_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    relationship_id TEXT NOT NULL,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    deleted_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE yard_orchestrator_route_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_orchestrator_version INTEGER NOT NULL
        CHECK (expected_orchestrator_version > 0),
    target_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    target_orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_project_version INTEGER NOT NULL
        CHECK (expected_project_version > 0),
    prompt_text TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_pane_id TEXT NOT NULL,
    result_runtime_status TEXT,
    submitted_at_unix_ms INTEGER,
    CHECK (
        (result_runtime_status IS NULL AND submitted_at_unix_ms IS NULL)
        OR (
            result_runtime_status IS NOT NULL
            AND submitted_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

CREATE INDEX yard_orchestrator_routes_by_created
ON yard_orchestrator_route_commands(target_project_id, command_id);

PRAGMA user_version = 16;

CREATE TABLE command_acknowledgements_v30 (
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
                'coordination_node_delete'
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

INSERT INTO command_acknowledgements_v30
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v30
    RENAME TO command_acknowledgements;

-- One row per archived node, keyed by the command that archived it (an
-- archive, or a delete that archived an active node). It stores the command
-- inputs for exact replay and the versions the archive produced. The node row
-- itself is never removed or rewritten.
CREATE TABLE archived_coordination_nodes (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL UNIQUE
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    actor TEXT NOT NULL,
    expected_node_version INTEGER NOT NULL CHECK (expected_node_version > 0),
    expected_worker_version INTEGER CHECK (expected_worker_version > 0),
    result_node_version INTEGER NOT NULL
        CHECK (result_node_version > expected_node_version),
    worker_id TEXT
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_worker_version INTEGER CHECK (result_worker_version > 0),
    archived_at_unix_ms INTEGER NOT NULL,
    CHECK ((worker_id IS NULL) = (result_worker_version IS NULL)),
    CHECK (expected_worker_version IS NULL OR worker_id IS NOT NULL)
) STRICT;

-- The node-scoped automations an archive paused, so a later restore can
-- resume exactly these.
CREATE TABLE archived_coordination_node_automations (
    command_id TEXT NOT NULL
        REFERENCES archived_coordination_nodes(command_id) ON DELETE RESTRICT,
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE RESTRICT,
    result_automation_version INTEGER NOT NULL
        CHECK (result_automation_version > 0),
    PRIMARY KEY (command_id, automation_id)
) STRICT;

-- A deleted node must have been archived first, possibly by the same command.
CREATE TABLE deleted_coordination_nodes (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL UNIQUE
        REFERENCES archived_coordination_nodes(node_id) ON DELETE RESTRICT,
    actor TEXT NOT NULL,
    archive_json TEXT,
    archive_applied INTEGER NOT NULL CHECK (archive_applied IN (0, 1)),
    node_version INTEGER NOT NULL CHECK (node_version > 0),
    deleted_at_unix_ms INTEGER NOT NULL,
    CHECK (archive_applied = 0 OR archive_json IS NOT NULL)
) STRICT;

PRAGMA user_version = 30;

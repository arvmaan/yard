-- Repairs a version 28 database created by the first 0028 (commit 3fdcf5e).
-- Commit f2cf35e rewrote 0028 in place, so a database already at version 28
-- never ran it: its acknowledgement types stop at 'project_archive', and
-- deleted_projects/deleted_workers keep an actor column with no command FK.
-- That code never wrote acknowledgements for its deletes, so each legacy
-- tombstone gets a succeeded one carrying its stored actor and time.

CREATE TABLE command_acknowledgements_v28 (
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
                'worker_delete'
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

INSERT INTO command_acknowledgements_v28
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v28
    RENAME TO command_acknowledgements;

INSERT INTO command_acknowledgements (
    id, command_type, actor, status, error_message,
    created_at_unix_ms, updated_at_unix_ms
)
SELECT command_id, 'project_delete', actor, 'succeeded', NULL,
       deleted_at_unix_ms, deleted_at_unix_ms
  FROM deleted_projects deleted
 WHERE NOT EXISTS (
        SELECT 1 FROM command_acknowledgements command
         WHERE command.id = deleted.command_id
       );

-- A project delete also tombstoned its orchestrator under the same command,
-- which the insert above already acknowledged.
INSERT INTO command_acknowledgements (
    id, command_type, actor, status, error_message,
    created_at_unix_ms, updated_at_unix_ms
)
SELECT command_id, 'worker_delete', actor, 'succeeded', NULL,
       deleted_at_unix_ms, deleted_at_unix_ms
  FROM deleted_workers deleted
 WHERE NOT EXISTS (
        SELECT 1 FROM command_acknowledgements command
         WHERE command.id = deleted.command_id
       );

ALTER TABLE deleted_projects RENAME TO deleted_projects_3fdcf5e;
ALTER TABLE deleted_workers RENAME TO deleted_workers_3fdcf5e;

CREATE TABLE deleted_projects (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    deleted_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE deleted_workers (
    worker_id TEXT PRIMARY KEY NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    expected_worker_version INTEGER NOT NULL
        CHECK (expected_worker_version > 0),
    deleted_at_unix_ms INTEGER NOT NULL
) STRICT;

INSERT INTO deleted_projects (
    project_id, command_id, orchestrator_worker_id, deleted_at_unix_ms
)
SELECT project_id, command_id, orchestrator_worker_id, deleted_at_unix_ms
  FROM deleted_projects_3fdcf5e;

INSERT INTO deleted_workers (
    worker_id, command_id, expected_worker_version, deleted_at_unix_ms
)
SELECT worker_id, command_id, expected_worker_version, deleted_at_unix_ms
  FROM deleted_workers_3fdcf5e;

DROP TABLE deleted_projects_3fdcf5e;
DROP TABLE deleted_workers_3fdcf5e;

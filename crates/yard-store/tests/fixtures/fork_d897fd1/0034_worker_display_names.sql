-- Worker display names.
--
-- A worker may carry a user-chosen display name so several workers of the
-- same profile ("Generalist", "Generalist-2", ...) can be told apart. The
-- domain validates the name (trimmed, at most 64 characters, no control or
-- bidi override characters); the byte-length CHECK is only a backstop.
-- Renaming is cosmetic: it never changes `workers.version` or any runtime
-- binding version, so in-flight commands that carry an expected worker
-- version keep working. Existing workers get NULL (the default label).

ALTER TABLE workers
ADD COLUMN display_name TEXT
    CHECK (
        display_name IS NULL
        OR (
            length(display_name) > 0
            AND length(CAST(display_name AS BLOB)) <= 256
        )
    );

CREATE TABLE command_acknowledgements_v34 (
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
                'project_restore',
                'worker_rename'
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

INSERT INTO command_acknowledgements_v34
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v34
    RENAME TO command_acknowledgements;

-- A rename's inputs and result, so an identical retry replays and a reused
-- command id with a different payload is an idempotency conflict.
CREATE TABLE worker_rename_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_display_name TEXT,
    display_name TEXT,
    previous_display_name TEXT,
    renamed_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX worker_rename_commands_by_worker
ON worker_rename_commands (worker_id, renamed_at_unix_ms);

PRAGMA user_version = 34;

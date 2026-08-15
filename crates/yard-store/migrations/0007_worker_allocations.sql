CREATE TABLE command_acknowledgements_v7 (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL
        CHECK (
            command_type IN (
                'profile_allocation',
                'worker_allocation',
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

INSERT INTO command_acknowledgements_v7
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v7
    RENAME TO command_acknowledgements;

CREATE TABLE worker_allocation_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_worker_version INTEGER NOT NULL
        CHECK (expected_worker_version > 0),
    requested_profile_id TEXT,
    requested_profile_version INTEGER,
    expected_project_version INTEGER NOT NULL
        CHECK (expected_project_version > 0),
    objective TEXT NOT NULL,
    role TEXT NOT NULL,
    isolation_policy TEXT NOT NULL
        CHECK (isolation_policy IN ('project_workspace')),
    replace_runtime INTEGER NOT NULL CHECK (replace_runtime IN (0, 1)),
    result_allocation_id TEXT UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    result_assignment_id TEXT UNIQUE
        REFERENCES assignments(id) ON DELETE RESTRICT,
    CHECK (
        (
            requested_profile_id IS NULL
            AND requested_profile_version IS NULL
        )
        OR
        (
            requested_profile_id IS NOT NULL
            AND requested_profile_version IS NOT NULL
            AND requested_profile_version > 0
        )
    )
) STRICT;

UPDATE worker_allocations
   SET ended_at_unix_ms = (
       SELECT a.updated_at_unix_ms
         FROM assignments a
        WHERE a.allocation_id = worker_allocations.id
          AND a.lifecycle = 'failed'
   )
 WHERE ended_at_unix_ms IS NULL
   AND EXISTS (
       SELECT 1
         FROM assignments a
        WHERE a.allocation_id = worker_allocations.id
          AND a.lifecycle = 'failed'
   );

PRAGMA user_version = 7;

CREATE TABLE command_acknowledgements_v4 (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL
        CHECK (
            command_type IN (
                'profile_allocation',
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

INSERT INTO command_acknowledgements_v4
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v4
    RENAME TO command_acknowledgements;

CREATE TABLE assignment_prompt_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    expected_assignment_version INTEGER NOT NULL
        CHECK (expected_assignment_version > 0),
    expected_attempt_version INTEGER NOT NULL
        CHECK (expected_attempt_version > 0),
    prompt_text TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_pane_id TEXT NOT NULL,
    result_runtime_status TEXT,
    submitted_at_unix_ms INTEGER,
    FOREIGN KEY (assignment_id, project_id)
        REFERENCES assignments(id, project_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT,
    CHECK (
        (result_runtime_status IS NULL AND submitted_at_unix_ms IS NULL)
        OR (
            result_runtime_status IS NOT NULL
            AND submitted_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

PRAGMA user_version = 4;

CREATE TABLE command_acknowledgements_v3 (
    id TEXT PRIMARY KEY NOT NULL,
    command_type TEXT NOT NULL
        CHECK (command_type IN ('profile_allocation', 'completion_receipt')),
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

INSERT INTO command_acknowledgements_v3
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v3
    RENAME TO command_acknowledgements;

CREATE TABLE assignments_v3 (
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
        CHECK (lifecycle IN ('allocating', 'active', 'completed', 'failed')),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (id, project_id),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO assignments_v3
SELECT id, project_id, allocation_id, worker_id, profile_id, profile_version,
       objective, role, isolation_policy, lifecycle, version,
       created_at_unix_ms, updated_at_unix_ms
FROM assignments;

DROP TABLE assignments;
ALTER TABLE assignments_v3 RENAME TO assignments;

CREATE UNIQUE INDEX one_open_assignment_per_worker
ON assignments(worker_id)
WHERE lifecycle IN ('allocating', 'active');

CREATE TABLE assignment_attempts_v3 (
    id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL
        REFERENCES assignments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    lifecycle TEXT NOT NULL
        CHECK (lifecycle IN ('starting', 'active', 'completed', 'failed')),
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

INSERT INTO assignment_attempts_v3
SELECT id, assignment_id, ordinal, lifecycle, error_message, version,
       created_at_unix_ms, updated_at_unix_ms
FROM assignment_attempts;

DROP TABLE assignment_attempts;
ALTER TABLE assignment_attempts_v3 RENAME TO assignment_attempts;

CREATE TABLE completion_receipt_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    expected_assignment_version INTEGER NOT NULL
        CHECK (expected_assignment_version > 0),
    expected_attempt_version INTEGER NOT NULL
        CHECK (expected_attempt_version > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('completed')),
    summary TEXT NOT NULL,
    FOREIGN KEY (assignment_id, project_id)
        REFERENCES assignments(id, project_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE completion_receipts (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL UNIQUE
        REFERENCES completion_receipt_commands(command_id) ON DELETE RESTRICT,
    assignment_id TEXT NOT NULL UNIQUE
        REFERENCES assignments(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL UNIQUE
        REFERENCES assignment_attempts(id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL CHECK (outcome IN ('completed')),
    summary TEXT NOT NULL,
    actor TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE completion_receipt_artifacts (
    receipt_id TEXT NOT NULL
        REFERENCES completion_receipts(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK (position >= 0),
    value TEXT NOT NULL,
    PRIMARY KEY (receipt_id, position)
) STRICT;

CREATE TABLE completion_receipt_evidence (
    receipt_id TEXT NOT NULL
        REFERENCES completion_receipts(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK (position >= 0),
    value TEXT NOT NULL,
    PRIMARY KEY (receipt_id, position)
) STRICT;

CREATE TABLE completion_receipt_blockers (
    receipt_id TEXT NOT NULL
        REFERENCES completion_receipts(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK (position >= 0),
    value TEXT NOT NULL,
    PRIMARY KEY (receipt_id, position)
) STRICT;

PRAGMA user_version = 3;

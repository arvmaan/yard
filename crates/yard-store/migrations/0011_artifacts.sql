CREATE UNIQUE INDEX completion_receipt_assignment_attempt_identity
ON completion_receipts(id, assignment_id, attempt_id);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL
        CHECK (kind IN ('markdown', 'html')),
    media_type TEXT NOT NULL
        CHECK (media_type IN ('text/markdown', 'text/html')),
    display_name TEXT NOT NULL,
    byte_size INTEGER NOT NULL
        CHECK (byte_size > 0 AND byte_size <= 1048576),
    sha256 TEXT NOT NULL
        CHECK (length(sha256) = 64),
    source TEXT NOT NULL
        CHECK (source IN ('upload')),
    created_by TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    UNIQUE (id, assignment_id, attempt_id),
    FOREIGN KEY (assignment_id, project_id)
        REFERENCES assignments(id, project_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE completion_receipt_artifact_links (
    receipt_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    artifact_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    PRIMARY KEY (receipt_id, position),
    UNIQUE (receipt_id, artifact_id),
    FOREIGN KEY (receipt_id, assignment_id, attempt_id)
        REFERENCES completion_receipts(id, assignment_id, attempt_id)
        ON DELETE RESTRICT,
    FOREIGN KEY (artifact_id, assignment_id, attempt_id)
        REFERENCES artifacts(id, assignment_id, attempt_id)
        ON DELETE RESTRICT
) STRICT;

PRAGMA user_version = 11;

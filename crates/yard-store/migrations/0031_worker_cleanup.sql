ALTER TABLE workers
ADD COLUMN ownership_kind TEXT NOT NULL DEFAULT 'external'
    CHECK (ownership_kind IN ('external', 'yard_owned', 'system_ephemeral'));

ALTER TABLE workers
ADD COLUMN parent_worker_id TEXT REFERENCES workers(id) ON DELETE RESTRICT;

UPDATE workers
   SET ownership_kind = 'yard_owned'
 WHERE id IN (
    SELECT worker_id FROM worker_allocations WHERE mode = 'create_new'
 );

CREATE TABLE worker_cleanup_policy (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    automatic_enabled INTEGER NOT NULL CHECK (automatic_enabled IN (0, 1)),
    schedule_minutes INTEGER NOT NULL CHECK (schedule_minutes BETWEEN 5 AND 10080),
    grace_period_ms INTEGER NOT NULL CHECK (grace_period_ms BETWEEN 60000 AND 7776000000),
    batch_size INTEGER NOT NULL CHECK (batch_size BETWEEN 1 AND 100),
    advisor_profile_id TEXT REFERENCES worker_profiles(id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version > 0),
    updated_by TEXT NOT NULL CHECK (length(updated_by) > 0),
    updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0)
) STRICT;

INSERT INTO worker_cleanup_policy (
    singleton_id, automatic_enabled, schedule_minutes, grace_period_ms,
    batch_size, advisor_profile_id, version, updated_by, updated_at_unix_ms
) VALUES (1, 0, 60, 86400000, 25, NULL, 1, 'yard:migration', 0);

CREATE TABLE worker_cleanup_runs (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL UNIQUE,
    trigger TEXT NOT NULL CHECK (trigger IN ('preview', 'manual', 'scheduled')),
    preview INTEGER NOT NULL CHECK (preview IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'completed', 'cancelled', 'failed')),
    advisor_state TEXT NOT NULL CHECK (advisor_state IN ('not_requested', 'unsupported', 'pending', 'completed', 'failed')),
    requested_by TEXT NOT NULL,
    policy_version INTEGER NOT NULL CHECK (policy_version > 0),
    grace_period_ms INTEGER NOT NULL,
    batch_size INTEGER NOT NULL,
    advisor_profile_id TEXT,
    cancellation_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancellation_requested IN (0, 1)),
    cancellation_command_id TEXT UNIQUE,
    cancelled_by TEXT,
    last_error TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    completed_at_unix_ms INTEGER
) STRICT;

CREATE TABLE worker_cleanup_run_items (
    run_id TEXT NOT NULL REFERENCES worker_cleanup_runs(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
    assignment_id TEXT NOT NULL REFERENCES assignments(id) ON DELETE RESTRICT,
    completion_receipt_id TEXT NOT NULL REFERENCES completion_receipts(id) ON DELETE RESTRICT,
    expected_worker_version INTEGER NOT NULL CHECK (expected_worker_version > 0),
    expected_runtime_version INTEGER NOT NULL CHECK (expected_runtime_version > 0),
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    terminal_id TEXT NOT NULL,
    tab_id TEXT,
    pane_id TEXT NOT NULL,
    provider_session_json TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'keep', 'review', 'reconciled', 'retired', 'failed', 'cancelled')),
    reason TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at_unix_ms INTEGER NOT NULL,
    claim_token TEXT,
    claim_expires_at_unix_ms INTEGER,
    advisor_worker_id TEXT REFERENCES workers(id) ON DELETE RESTRICT,
    advisor_assignment_id TEXT REFERENCES assignments(id) ON DELETE RESTRICT,
    advisor_completion_receipt_id TEXT REFERENCES completion_receipts(id) ON DELETE RESTRICT,
    advisor_recommendation TEXT CHECK (advisor_recommendation IN ('keep', 'retire', 'review')),
    advisor_artifact_id TEXT REFERENCES artifacts(id) ON DELETE RESTRICT,
    advisor_rationale TEXT,
    is_cleanup_advisor INTEGER NOT NULL DEFAULT 0
        CHECK (is_cleanup_advisor IN (0, 1)),
    updated_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (run_id, worker_id),
    UNIQUE (run_id, assignment_id),
    UNIQUE (advisor_worker_id),
    UNIQUE (advisor_assignment_id),
    UNIQUE (advisor_completion_receipt_id)
) STRICT;

CREATE INDEX pending_worker_cleanup_items
ON worker_cleanup_run_items(next_attempt_at_unix_ms, run_id, worker_id)
WHERE status = 'pending';

CREATE UNIQUE INDEX one_pending_cleanup_per_worker
ON worker_cleanup_run_items(worker_id)
WHERE status = 'pending';

CREATE TABLE worker_cleanup_pins (
    worker_id TEXT PRIMARY KEY NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
    pinned_by TEXT NOT NULL,
    reason TEXT NOT NULL,
    pinned_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE cleanup_advisor_artifacts (
    artifact_id TEXT PRIMARY KEY NOT NULL REFERENCES artifacts(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL REFERENCES worker_cleanup_runs(id) ON DELETE RESTRICT,
    target_worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
    completion_receipt_id TEXT NOT NULL REFERENCES completion_receipts(id) ON DELETE RESTRICT,
    recommendation TEXT NOT NULL CHECK (recommendation IN ('keep', 'retire', 'review')),
    evidence_ids_json TEXT NOT NULL CHECK (json_valid(evidence_ids_json)),
    rationale TEXT NOT NULL,
    recorded_at_unix_ms INTEGER NOT NULL,
    UNIQUE (run_id, target_worker_id)
) STRICT;

PRAGMA user_version = 31;

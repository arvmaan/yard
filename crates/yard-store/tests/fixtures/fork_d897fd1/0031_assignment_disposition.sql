CREATE TABLE command_acknowledgements_v31 (
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
                'assignment_disposition'
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

INSERT INTO command_acknowledgements_v31
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v31
    RENAME TO command_acknowledgements;

-- Assignments and attempts gain the non-success terminal state `cancelled`.
-- `completed` stays success-only. The inline composite UNIQUEs are FK parents
-- for artifacts, prompts, handoffs, and receipts, so they are kept verbatim.
CREATE TABLE assignments_v31 (
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
        CHECK (
            lifecycle IN (
                'allocating',
                'active',
                'handing_off',
                'handed_off',
                'completed',
                'failed',
                'cancelled'
            )
        ),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (id, project_id),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO assignments_v31 (
    id, project_id, allocation_id, worker_id, profile_id, profile_version,
    objective, role, isolation_policy, lifecycle, version,
    created_at_unix_ms, updated_at_unix_ms
)
SELECT id, project_id, allocation_id, worker_id, profile_id, profile_version,
       objective, role, isolation_policy, lifecycle, version,
       created_at_unix_ms, updated_at_unix_ms
FROM assignments;

DROP TABLE assignments;
ALTER TABLE assignments_v31 RENAME TO assignments;

CREATE UNIQUE INDEX one_open_assignment_per_worker
ON assignments(worker_id)
WHERE lifecycle IN ('allocating', 'active', 'handing_off');

CREATE TABLE assignment_attempts_v31 (
    id TEXT PRIMARY KEY NOT NULL,
    assignment_id TEXT NOT NULL
        REFERENCES assignments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    lifecycle TEXT NOT NULL
        CHECK (
            lifecycle IN (
                'starting',
                'active',
                'handing_off',
                'handed_off',
                'completed',
                'failed',
                'cancelled'
            )
        ),
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

INSERT INTO assignment_attempts_v31 (
    id, assignment_id, ordinal, lifecycle, error_message, version,
    created_at_unix_ms, updated_at_unix_ms
)
SELECT id, assignment_id, ordinal, lifecycle, error_message, version,
       created_at_unix_ms, updated_at_unix_ms
FROM assignment_attempts;

DROP TABLE assignment_attempts;
ALTER TABLE assignment_attempts_v31 RENAME TO assignment_attempts;

-- Receipts gain an explicit detail level. A minimal receipt comes only from
-- the disposition command, which has no completion_receipt_commands row, so
-- the command FK now points at the global acknowledgement ledger. Existing
-- receipts are all detailed.
CREATE TABLE completion_receipts_v31 (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL UNIQUE
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    assignment_id TEXT NOT NULL UNIQUE
        REFERENCES assignments(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL UNIQUE
        REFERENCES assignment_attempts(id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL CHECK (outcome IN ('completed')),
    summary TEXT NOT NULL,
    actor TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    detail_level TEXT NOT NULL
        CHECK (detail_level IN ('detailed', 'minimal')),
    objective_snapshot TEXT,
    request_origin TEXT
        CHECK (request_origin IN ('browser', 'none')),
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT,
    CHECK (
        detail_level = 'detailed'
        OR (objective_snapshot IS NOT NULL AND request_origin IS NOT NULL)
    )
) STRICT;

INSERT INTO completion_receipts_v31 (
    id, command_id, assignment_id, attempt_id, outcome, summary, actor,
    created_at_unix_ms, detail_level, objective_snapshot, request_origin
)
SELECT id, command_id, assignment_id, attempt_id, outcome, summary, actor,
       created_at_unix_ms, 'detailed', NULL, NULL
FROM completion_receipts;

DROP TABLE completion_receipts;
ALTER TABLE completion_receipts_v31 RENAME TO completion_receipts;

-- Parent key of completion_receipt_artifact_links' composite FK (0011).
CREATE UNIQUE INDEX completion_receipt_assignment_attempt_identity
ON completion_receipts(id, assignment_id, attempt_id);

-- An assignment that ended without completion. It is written instead of a
-- receipt. `command_id` is not unique so one archive can later cancel many.
CREATE TABLE assignment_cancellations (
    assignment_id TEXT PRIMARY KEY NOT NULL
        REFERENCES assignments(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL UNIQUE
        REFERENCES assignment_attempts(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL
        CHECK (reason IN ('ended_without_completion', 'project_archived')),
    actor TEXT NOT NULL,
    request_origin TEXT NOT NULL
        CHECK (request_origin IN ('browser', 'none')),
    objective_snapshot TEXT NOT NULL,
    cancelled_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT
) STRICT;

-- Every input of one disposition command, for exact replay.
CREATE TABLE assignment_disposition_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_assignment_version INTEGER NOT NULL
        CHECK (expected_assignment_version > 0),
    expected_attempt_version INTEGER NOT NULL
        CHECK (expected_attempt_version > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('completed', 'cancelled')),
    end_session INTEGER NOT NULL CHECK (end_session IN (0, 1)),
    expected_worker_version INTEGER
        CHECK (expected_worker_version > 0),
    expected_runtime_version INTEGER
        CHECK (expected_runtime_version > 0),
    request_origin TEXT NOT NULL
        CHECK (request_origin IN ('browser', 'none')),
    -- The ambiguous handoff closed by a handing_off cancellation, if any.
    handoff_command_id TEXT
        REFERENCES worker_handoff_commands(command_id) ON DELETE RESTRICT,
    finished_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (assignment_id, project_id)
        REFERENCES assignments(id, project_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_id, assignment_id)
        REFERENCES assignment_attempts(id, assignment_id) ON DELETE RESTRICT,
    CHECK (
        (end_session = 1 AND expected_worker_version IS NOT NULL)
        OR (
            end_session = 0
            AND expected_worker_version IS NULL
            AND expected_runtime_version IS NULL
        )
    ),
    CHECK (handoff_command_id IS NULL OR outcome = 'cancelled')
) STRICT;

-- Durable, retryable reads of a worker's terminal before its output is lost.
-- The captured runtime identity is compared against fresh inventory before
-- and after every read. A job never expires while Herdr is unreachable.
CREATE TABLE transcript_capture_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    assignment_id TEXT
        REFERENCES assignments(id) ON DELETE RESTRICT,
    allocation_id TEXT
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
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
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'succeeded', 'expired')),
    expired_reason TEXT
        CHECK (
            expired_reason IN (
                'runtime_closed',
                'runtime_reused',
                'worker_reallocated'
            )
        ),
    attempts INTEGER NOT NULL CHECK (attempts >= 0),
    last_error TEXT,
    next_attempt_at_unix_ms INTEGER NOT NULL,
    claim_token TEXT,
    claim_expires_at_unix_ms INTEGER,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    completed_at_unix_ms INTEGER,
    UNIQUE (command_id, worker_id),
    CHECK ((status = 'expired') = (expired_reason IS NOT NULL)),
    CHECK (
        (status = 'pending' AND completed_at_unix_ms IS NULL)
        OR (status <> 'pending' AND completed_at_unix_ms IS NOT NULL)
    ),
    CHECK ((claim_token IS NULL) = (claim_expires_at_unix_ms IS NULL)),
    CHECK (claim_token IS NULL OR status = 'pending'),
    CHECK (assignment_id IS NOT NULL OR allocation_id IS NULL),
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

CREATE INDEX pending_transcript_capture_jobs
ON transcript_capture_jobs(next_attempt_at_unix_ms)
WHERE status = 'pending';

CREATE UNIQUE INDEX claimed_transcript_capture_tokens
ON transcript_capture_jobs(claim_token)
WHERE claim_token IS NOT NULL;

CREATE INDEX transcript_capture_jobs_by_assignment
ON transcript_capture_jobs(assignment_id)
WHERE assignment_id IS NOT NULL;

-- Read-only terminal text kept after an assignment ends, keyed by the worker
-- and the runtime identity it was read from. At most 10,000 lines and
-- 1 MiB; `truncated` records any cut. Purge (a later change) clears `text`
-- and keeps the metadata.
CREATE TABLE worker_transcripts (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL UNIQUE
        REFERENCES transcript_capture_jobs(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    assignment_id TEXT
        REFERENCES assignments(id) ON DELETE RESTRICT,
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    terminal_id TEXT NOT NULL,
    pane_id TEXT NOT NULL,
    provider_session_source TEXT,
    provider_session_provider TEXT,
    provider_session_kind TEXT,
    provider_session_value TEXT,
    source TEXT NOT NULL,
    format TEXT NOT NULL,
    text TEXT,
    line_count INTEGER NOT NULL
        CHECK (line_count >= 0 AND line_count <= 10000),
    byte_count INTEGER NOT NULL
        CHECK (byte_count >= 0 AND byte_count <= 1048576),
    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    captured_at_unix_ms INTEGER NOT NULL,
    purged_at_unix_ms INTEGER,
    purged_by TEXT,
    CHECK ((purged_at_unix_ms IS NULL) = (purged_by IS NULL)),
    CHECK ((text IS NULL) = (purged_at_unix_ms IS NOT NULL)),
    CHECK (text IS NULL OR length(CAST(text AS BLOB)) = byte_count),
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

CREATE INDEX worker_transcripts_by_runtime
ON worker_transcripts(worker_id, adapter, runtime_session, terminal_id);

CREATE INDEX worker_transcripts_by_assignment
ON worker_transcripts(assignment_id)
WHERE assignment_id IS NOT NULL;

PRAGMA user_version = 31;

CREATE TABLE command_acknowledgements_v20 (
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
                'project_orchestrator_replacement'
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

INSERT INTO command_acknowledgements_v20
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v20
    RENAME TO command_acknowledgements;

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN provider_session_source TEXT;

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN provider_session_provider TEXT;

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN provider_session_kind TEXT;

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN provider_session_value TEXT;

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN expected_worker_version INTEGER
    CHECK (expected_worker_version IS NULL OR expected_worker_version > 0);

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN expected_binding_state TEXT
    CHECK (
        expected_binding_state IS NULL
        OR expected_binding_state IN ('detached', 'replaced')
    );

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN claim_token TEXT;

ALTER TABLE runtime_cleanup_jobs
ADD COLUMN claim_expires_at_unix_ms INTEGER;

UPDATE runtime_cleanup_jobs
   SET provider_session_source = (
           SELECT retired.provider_session_source
             FROM retired_runtime_bindings retired
            WHERE retired.command_id = runtime_cleanup_jobs.command_id
              AND retired.worker_id = runtime_cleanup_jobs.worker_id
       ),
       provider_session_provider = (
           SELECT retired.provider_session_provider
             FROM retired_runtime_bindings retired
            WHERE retired.command_id = runtime_cleanup_jobs.command_id
              AND retired.worker_id = runtime_cleanup_jobs.worker_id
       ),
       provider_session_kind = (
           SELECT retired.provider_session_kind
             FROM retired_runtime_bindings retired
            WHERE retired.command_id = runtime_cleanup_jobs.command_id
              AND retired.worker_id = runtime_cleanup_jobs.worker_id
       ),
       provider_session_value = (
           SELECT retired.provider_session_value
             FROM retired_runtime_bindings retired
            WHERE retired.command_id = runtime_cleanup_jobs.command_id
              AND retired.worker_id = runtime_cleanup_jobs.worker_id
       ),
       expected_worker_version = (
           SELECT worker.version
             FROM workers worker
            WHERE worker.id = runtime_cleanup_jobs.worker_id
       ),
       expected_binding_state = CASE runtime_cleanup_jobs.reason
           WHEN 'handoff_source' THEN 'replaced'
           ELSE 'detached'
       END;

-- V18 orchestrator handoff left the displaced worker bound after ownership
-- moved. Detach only an exact pending cleanup identity that is no longer
-- owned anywhere; any inconsistent row remains bound and fails closed.
UPDATE workers
   SET version = version + 1,
       updated_at_unix_ms = MAX(
           updated_at_unix_ms,
           (
               SELECT MAX(cleanup.updated_at_unix_ms)
                 FROM runtime_cleanup_jobs cleanup
                WHERE cleanup.worker_id = workers.id
                  AND cleanup.status = 'pending'
                  AND cleanup.reason = 'replaced_orchestrator'
           )
       )
 WHERE EXISTS (
           SELECT 1
             FROM runtime_cleanup_jobs cleanup
             JOIN worker_runtime_bindings binding
               ON binding.worker_id = cleanup.worker_id
              AND binding.adapter = cleanup.adapter
              AND binding.runtime_session = cleanup.runtime_session
              AND binding.terminal_id = cleanup.terminal_id
              AND (
                  (
                      cleanup.provider_session_source IS NULL
                      AND binding.provider_session_source IS NULL
                  )
                  OR (
                      cleanup.provider_session_source =
                          binding.provider_session_source
                      AND cleanup.provider_session_provider =
                          binding.provider_session_provider
                      AND cleanup.provider_session_kind =
                          binding.provider_session_kind
                      AND cleanup.provider_session_value =
                          binding.provider_session_value
                  )
              )
            WHERE cleanup.worker_id = workers.id
              AND cleanup.status = 'pending'
              AND cleanup.reason = 'replaced_orchestrator'
       )
   AND NOT EXISTS (
           SELECT 1 FROM worker_allocations allocation
            WHERE allocation.worker_id = workers.id
              AND allocation.ended_at_unix_ms IS NULL
       )
   AND NOT EXISTS (
           SELECT 1 FROM assignments assignment
            WHERE assignment.worker_id = workers.id
              AND assignment.lifecycle IN ('allocating', 'active', 'handing_off')
       )
   AND NOT EXISTS (
           SELECT 1 FROM projects project
            WHERE project.orchestrator_worker_id = workers.id
       )
   AND NOT EXISTS (
           SELECT 1 FROM yard_orchestrator orchestrator
            WHERE orchestrator.worker_id = workers.id
       )
   AND NOT EXISTS (
           SELECT 1 FROM coordination_nodes node
            WHERE node.worker_id = workers.id
       );

DELETE FROM worker_runtime_bindings
 WHERE EXISTS (
           SELECT 1
             FROM runtime_cleanup_jobs cleanup
            WHERE cleanup.worker_id = worker_runtime_bindings.worker_id
              AND cleanup.status = 'pending'
              AND cleanup.reason = 'replaced_orchestrator'
              AND cleanup.adapter = worker_runtime_bindings.adapter
              AND cleanup.runtime_session =
                  worker_runtime_bindings.runtime_session
              AND cleanup.terminal_id = worker_runtime_bindings.terminal_id
              AND (
                  (
                      cleanup.provider_session_source IS NULL
                      AND worker_runtime_bindings.provider_session_source IS NULL
                  )
                  OR (
                      cleanup.provider_session_source =
                          worker_runtime_bindings.provider_session_source
                      AND cleanup.provider_session_provider =
                          worker_runtime_bindings.provider_session_provider
                      AND cleanup.provider_session_kind =
                          worker_runtime_bindings.provider_session_kind
                      AND cleanup.provider_session_value =
                          worker_runtime_bindings.provider_session_value
                  )
              )
       )
   AND NOT EXISTS (
           SELECT 1 FROM worker_allocations allocation
            WHERE allocation.worker_id = worker_runtime_bindings.worker_id
              AND allocation.ended_at_unix_ms IS NULL
       )
   AND NOT EXISTS (
           SELECT 1 FROM assignments assignment
            WHERE assignment.worker_id = worker_runtime_bindings.worker_id
              AND assignment.lifecycle IN ('allocating', 'active', 'handing_off')
       )
   AND NOT EXISTS (
           SELECT 1 FROM projects project
            WHERE project.orchestrator_worker_id =
                  worker_runtime_bindings.worker_id
       )
   AND NOT EXISTS (
           SELECT 1 FROM yard_orchestrator orchestrator
            WHERE orchestrator.worker_id = worker_runtime_bindings.worker_id
       )
   AND NOT EXISTS (
           SELECT 1 FROM coordination_nodes node
            WHERE node.worker_id = worker_runtime_bindings.worker_id
       );

UPDATE runtime_cleanup_jobs
   SET expected_worker_version = (
           SELECT worker.version
             FROM workers worker
            WHERE worker.id = runtime_cleanup_jobs.worker_id
       )
 WHERE status = 'pending';

CREATE TABLE runtime_cleanup_jobs_v20 (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL
        CHECK (
            reason IN (
                'handoff_source',
                'replaced_orchestrator',
                'worker_session_end'
            )
        ),
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_workspace_id TEXT NOT NULL,
    terminal_id TEXT NOT NULL,
    tab_id TEXT,
    pane_id TEXT NOT NULL,
    owns_tab INTEGER NOT NULL CHECK (owns_tab IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending', 'succeeded')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    next_attempt_at_unix_ms INTEGER NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    completed_at_unix_ms INTEGER,
    provider_session_source TEXT,
    provider_session_provider TEXT,
    provider_session_kind TEXT,
    provider_session_value TEXT,
    expected_worker_version INTEGER
        CHECK (expected_worker_version IS NULL OR expected_worker_version > 0),
    expected_binding_state TEXT
        CHECK (
            expected_binding_state IS NULL
            OR expected_binding_state IN ('detached', 'replaced')
        ),
    claim_token TEXT,
    claim_expires_at_unix_ms INTEGER,
    CHECK (
        (status = 'pending' AND completed_at_unix_ms IS NULL)
        OR (status = 'succeeded' AND completed_at_unix_ms IS NOT NULL)
    ),
    CHECK (owns_tab = 0 OR tab_id IS NOT NULL),
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
    ),
    CHECK (
        (
            claim_token IS NULL
            AND claim_expires_at_unix_ms IS NULL
        )
        OR (
            status = 'pending'
            AND claim_token IS NOT NULL
            AND claim_expires_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

INSERT INTO runtime_cleanup_jobs_v20 (
    id, command_id, worker_id, reason, adapter, runtime_session,
    runtime_workspace_id, terminal_id, tab_id, pane_id, owns_tab,
    status, attempts, last_error, next_attempt_at_unix_ms,
    created_at_unix_ms, updated_at_unix_ms, completed_at_unix_ms,
    provider_session_source, provider_session_provider,
    provider_session_kind, provider_session_value,
    expected_worker_version, expected_binding_state,
    claim_token, claim_expires_at_unix_ms
)
SELECT id, command_id, worker_id, reason, adapter, runtime_session,
       runtime_workspace_id, terminal_id, tab_id, pane_id, owns_tab,
       status, attempts, last_error, next_attempt_at_unix_ms,
       created_at_unix_ms, updated_at_unix_ms, completed_at_unix_ms,
       provider_session_source, provider_session_provider,
       provider_session_kind, provider_session_value,
       expected_worker_version, expected_binding_state,
       claim_token, claim_expires_at_unix_ms
FROM runtime_cleanup_jobs;

DROP TABLE runtime_cleanup_jobs;
ALTER TABLE runtime_cleanup_jobs_v20 RENAME TO runtime_cleanup_jobs;

CREATE INDEX pending_runtime_cleanup_jobs
ON runtime_cleanup_jobs(next_attempt_at_unix_ms, created_at_unix_ms)
WHERE status = 'pending';

CREATE UNIQUE INDEX claimed_runtime_cleanup_tokens
ON runtime_cleanup_jobs(claim_token)
WHERE claim_token IS NOT NULL;

CREATE TABLE project_orchestrator_replacement_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    expected_project_version INTEGER NOT NULL
        CHECK (expected_project_version > 0),
    expected_orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_orchestrator_worker_version INTEGER NOT NULL
        CHECK (expected_orchestrator_worker_version > 0),
    expected_orchestrator_runtime_version INTEGER NOT NULL
        CHECK (expected_orchestrator_runtime_version > 0),
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    objective TEXT NOT NULL,
    role TEXT NOT NULL,
    old_session_disposition TEXT NOT NULL
        CHECK (
            old_session_disposition IN (
                'retain_for_inspection',
                'retire_after_cutover',
                'end_after_cutover'
            )
        ),
    handoff_artifact_ref TEXT,
    result_worker_id TEXT UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_allocation_id TEXT UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    result_assignment_id TEXT UNIQUE
        REFERENCES assignments(id) ON DELETE RESTRICT,
    finished_at_unix_ms INTEGER,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT,
    CHECK (
        (
            result_worker_id IS NULL
            AND result_allocation_id IS NULL
            AND result_assignment_id IS NULL
        )
        OR (
            result_worker_id IS NOT NULL
            AND result_allocation_id IS NOT NULL
            AND result_assignment_id IS NOT NULL
            AND finished_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

CREATE UNIQUE INDEX one_unfinished_project_orchestrator_replacement
ON project_orchestrator_replacement_commands(project_id)
WHERE finished_at_unix_ms IS NULL;

CREATE TABLE orchestrator_replacement_runtime_bindings (
    id TEXT PRIMARY KEY NOT NULL,
    command_id TEXT NOT NULL
        REFERENCES project_orchestrator_replacement_commands(command_id)
        ON DELETE RESTRICT,
    binding_role TEXT NOT NULL
        CHECK (
            binding_role IN (
                'displaced',
                'replacement_prepared',
                'replacement_started'
            )
        ),
    worker_id TEXT
        REFERENCES workers(id) ON DELETE RESTRICT,
    allocation_id TEXT
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    assignment_id TEXT
        REFERENCES assignments(id) ON DELETE RESTRICT,
    assignment_version INTEGER
        CHECK (assignment_version IS NULL OR assignment_version > 0),
    attempt_id TEXT
        REFERENCES assignment_attempts(id) ON DELETE RESTRICT,
    attempt_version INTEGER
        CHECK (attempt_version IS NULL OR attempt_version > 0),
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
    owns_tab INTEGER NOT NULL CHECK (owns_tab IN (0, 1)),
    observation_state TEXT NOT NULL
        CHECK (observation_state IN ('observed', 'missing', 'ambiguous')),
    process_state TEXT NOT NULL
        CHECK (process_state IN ('running', 'exited', 'unknown')),
    observed_status TEXT NOT NULL
        CHECK (observed_status IN ('idle', 'working', 'blocked', 'done', 'unknown')),
    state_change_sequence INTEGER NOT NULL CHECK (state_change_sequence >= 0),
    runtime_revision INTEGER NOT NULL CHECK (runtime_revision >= 0),
    runtime_version INTEGER NOT NULL CHECK (runtime_version > 0),
    last_observed_at_unix_ms INTEGER NOT NULL
        CHECK (last_observed_at_unix_ms >= 0),
    captured_at_unix_ms INTEGER NOT NULL,
    UNIQUE (command_id, binding_role),
    CHECK (
        (
            binding_role = 'displaced'
            AND worker_id IS NOT NULL
            AND allocation_id IS NOT NULL
            AND (
                (
                    assignment_id IS NULL
                    AND assignment_version IS NULL
                    AND attempt_id IS NULL
                    AND attempt_version IS NULL
                )
                OR (
                    assignment_id IS NOT NULL
                    AND assignment_version IS NOT NULL
                    AND attempt_id IS NOT NULL
                    AND attempt_version IS NOT NULL
                )
            )
        )
        OR (
            binding_role <> 'displaced'
            AND worker_id IS NULL
            AND allocation_id IS NULL
            AND assignment_id IS NULL
            AND assignment_version IS NULL
            AND attempt_id IS NULL
            AND attempt_version IS NULL
        )
    ),
    CHECK (
        (
            provider_session_source IS NULL
            AND provider_session_provider IS NULL
            AND provider_session_kind IS NULL
            AND provider_session_value IS NULL
        )
        OR
        (
            provider_session_source IS NOT NULL
            AND provider_session_provider IS NOT NULL
            AND provider_session_kind IS NOT NULL
            AND provider_session_value IS NOT NULL
        )
    ),
    CHECK (owns_tab = 0 OR tab_id IS NOT NULL)
) STRICT;

CREATE UNIQUE INDEX one_project_replacement_claim_per_runtime
ON orchestrator_replacement_runtime_bindings (
    adapter,
    runtime_session,
    terminal_id,
    binding_role
)
WHERE binding_role IN ('replacement_prepared', 'replacement_started');

PRAGMA user_version = 20;

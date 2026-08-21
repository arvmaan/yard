ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_last_absence_observed_at_unix_ms INTEGER
    CHECK (
        prepare_last_absence_observed_at_unix_ms IS NULL
        OR prepare_last_absence_observed_at_unix_ms >= 0
    );

ALTER TABLE worker_allocation_commands
ADD COLUMN objective_delivery_confirmed INTEGER NOT NULL DEFAULT 0
    CHECK (objective_delivery_confirmed IN (0, 1));

UPDATE worker_allocation_commands AS allocation
   SET objective_delivery_confirmed = 1
 WHERE allocation.replace_runtime = 1
   AND allocation.result_allocation_id IS NOT NULL
   AND allocation.result_assignment_id IS NOT NULL
   AND EXISTS (
       SELECT 1
         FROM command_acknowledgements AS command
        WHERE command.id = allocation.command_id
          AND command.status = 'pending'
   )
   AND EXISTS (
       SELECT 1
         FROM lifecycle_events AS event
        WHERE event.aggregate_type = 'worker'
          AND event.aggregate_id = allocation.worker_id
          AND event.aggregate_version =
              allocation.expected_worker_version + 2
          AND event.event_type = 'runtime_binding_replaced'
          AND event.source = 'herdr'
   );

UPDATE project_orchestrator_replacement_commands
   SET prepare_recovery_outcome = NULL,
       prepare_recovery_detail =
           'v25 requires two strictly newer absence observations',
       prepare_reconciled_at_unix_ms = NULL,
       prepare_absence_observations = 0,
       prepare_next_recovery_at_unix_ms = 0
 WHERE prepare_absence_observations > 0
    OR prepare_recovery_outcome = 'absent_converged';

CREATE TABLE provisioning_runtime_claims (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
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
        CHECK (observation_state IN ('observed', 'ambiguous')),
    process_state TEXT NOT NULL
        CHECK (process_state IN ('running', 'exited', 'unknown')),
    observed_status TEXT NOT NULL,
    state_change_sequence INTEGER NOT NULL CHECK (state_change_sequence >= 0),
    runtime_revision INTEGER NOT NULL CHECK (runtime_revision >= 0),
    runtime_version INTEGER NOT NULL CHECK (runtime_version > 0),
    last_observed_at_unix_ms INTEGER NOT NULL
        CHECK (last_observed_at_unix_ms >= 0),
    captured_at_unix_ms INTEGER NOT NULL CHECK (captured_at_unix_ms >= 0),
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

CREATE TABLE quarantined_provisioning_runtime_bindings (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
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
        CHECK (observation_state IN ('observed', 'ambiguous')),
    process_state TEXT NOT NULL
        CHECK (process_state IN ('running', 'exited', 'unknown')),
    observed_status TEXT NOT NULL,
    state_change_sequence INTEGER NOT NULL CHECK (state_change_sequence >= 0),
    runtime_revision INTEGER NOT NULL CHECK (runtime_revision >= 0),
    runtime_version INTEGER NOT NULL CHECK (runtime_version > 0),
    last_observed_at_unix_ms INTEGER NOT NULL
        CHECK (last_observed_at_unix_ms >= 0),
    captured_at_unix_ms INTEGER NOT NULL CHECK (captured_at_unix_ms >= 0),
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

CREATE TABLE dedicated_runtime_provision_intents (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL
        CHECK (kind IN ('yard_orchestrator', 'coordination_node')),
    target_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    expected_target_version INTEGER NOT NULL
        CHECK (expected_target_version > 0),
    runtime_start_confirmed INTEGER NOT NULL DEFAULT 0
        CHECK (runtime_start_confirmed IN (0, 1)),
    result_worker_id TEXT
        REFERENCES workers(id) ON DELETE RESTRICT,
    created_at_unix_ms INTEGER NOT NULL CHECK (created_at_unix_ms >= 0),
    updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0)
) STRICT;

CREATE INDEX provisioning_runtime_claim_identity
ON provisioning_runtime_claims (
    adapter,
    runtime_session,
    terminal_id
);

CREATE INDEX quarantined_provisioning_runtime_identity
ON quarantined_provisioning_runtime_bindings (
    adapter,
    runtime_session,
    terminal_id
);

PRAGMA user_version = 25;

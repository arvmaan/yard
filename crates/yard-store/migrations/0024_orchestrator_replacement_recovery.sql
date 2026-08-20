ALTER TABLE orchestrator_replacement_runtime_bindings
ADD COLUMN objective_delivery_confirmed INTEGER NOT NULL DEFAULT 0
    CHECK (objective_delivery_confirmed IN (0, 1));

ALTER TABLE orchestrator_replacement_runtime_bindings
ADD COLUMN recovery_outcome TEXT
    CHECK (
        recovery_outcome IS NULL
        OR recovery_outcome IN (
            'adopted_current',
            'absent_converged',
            'conflicting_reused',
            'present_not_safely_retirable'
        )
    );

ALTER TABLE orchestrator_replacement_runtime_bindings
ADD COLUMN recovery_detail TEXT;

ALTER TABLE orchestrator_replacement_runtime_bindings
ADD COLUMN reconciled_at_unix_ms INTEGER
    CHECK (
        reconciled_at_unix_ms IS NULL
        OR reconciled_at_unix_ms >= 0
    );

ALTER TABLE orchestrator_replacement_runtime_bindings
ADD COLUMN recovery_attempts INTEGER NOT NULL DEFAULT 0
    CHECK (recovery_attempts >= 0);

ALTER TABLE orchestrator_replacement_runtime_bindings
ADD COLUMN next_recovery_at_unix_ms INTEGER
    CHECK (
        next_recovery_at_unix_ms IS NULL
        OR next_recovery_at_unix_ms >= 0
    );

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_tab_label TEXT;

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_intent_at_unix_ms INTEGER
    CHECK (
        prepare_intent_at_unix_ms IS NULL
        OR prepare_intent_at_unix_ms >= 0
    );

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_recovery_outcome TEXT
    CHECK (
        prepare_recovery_outcome IS NULL
        OR prepare_recovery_outcome IN (
            'adopted_current',
            'absent_converged',
            'conflicting_reused',
            'present_not_safely_retirable'
        )
    );

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_recovery_detail TEXT;

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_reconciled_at_unix_ms INTEGER
    CHECK (
        prepare_reconciled_at_unix_ms IS NULL
        OR prepare_reconciled_at_unix_ms >= 0
    );

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_recovery_attempts INTEGER NOT NULL DEFAULT 0
    CHECK (prepare_recovery_attempts >= 0);

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_absence_observations INTEGER NOT NULL DEFAULT 0
    CHECK (prepare_absence_observations >= 0);

ALTER TABLE project_orchestrator_replacement_commands
ADD COLUMN prepare_next_recovery_at_unix_ms INTEGER
    CHECK (
        prepare_next_recovery_at_unix_ms IS NULL
        OR prepare_next_recovery_at_unix_ms >= 0
    );

UPDATE project_orchestrator_replacement_commands
   SET prepare_recovery_outcome = 'present_not_safely_retirable',
       prepare_recovery_detail =
           'Legacy replacement has no durable command-unique prepare label; any uncaptured prepared runtime requires operator inspection',
       prepare_reconciled_at_unix_ms = (
           SELECT updated_at_unix_ms
             FROM command_acknowledgements
            WHERE id = project_orchestrator_replacement_commands.command_id
       )
 WHERE prepare_tab_label IS NULL
   AND NOT EXISTS (
       SELECT 1
         FROM orchestrator_replacement_runtime_bindings capture
        WHERE capture.command_id =
              project_orchestrator_replacement_commands.command_id
          AND capture.binding_role IN (
              'replacement_prepared',
              'replacement_started'
          )
   )
   AND command_id IN (
       SELECT id
         FROM command_acknowledgements
        WHERE command_type = 'project_orchestrator_replacement'
          AND status IN ('pending', 'ambiguous')
   );

DROP INDEX one_project_replacement_claim_per_runtime;

CREATE INDEX project_replacement_runtime_identity
ON orchestrator_replacement_runtime_bindings (
    adapter,
    runtime_session,
    terminal_id,
    binding_role
);

PRAGMA user_version = 24;

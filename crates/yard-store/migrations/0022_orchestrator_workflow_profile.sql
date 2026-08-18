CREATE TABLE orchestrator_workflow_profile_revisions (
    version INTEGER PRIMARY KEY NOT NULL CHECK (version > 0),
    instructions_markdown TEXT NOT NULL CHECK (length(instructions_markdown) > 0),
    monitor_interval_ms INTEGER NOT NULL CHECK (monitor_interval_ms > 0),
    source TEXT NOT NULL CHECK (source IN ('factory', 'user', 'reset')),
    updated_by TEXT NOT NULL CHECK (length(updated_by) > 0),
    created_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE orchestrator_workflow_profile_current (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    current_version INTEGER NOT NULL
        REFERENCES orchestrator_workflow_profile_revisions(version)
        ON DELETE RESTRICT,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

ALTER TABLE yard_orchestrator
ADD COLUMN workflow_profile_version INTEGER
    REFERENCES orchestrator_workflow_profile_revisions(version)
    ON DELETE RESTRICT;

ALTER TABLE yard_orchestrator_configure_commands
ADD COLUMN workflow_profile_version INTEGER
    REFERENCES orchestrator_workflow_profile_revisions(version)
    ON DELETE RESTRICT;

PRAGMA user_version = 22;

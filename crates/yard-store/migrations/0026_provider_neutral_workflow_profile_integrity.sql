CREATE TABLE orchestrator_workflow_profile_revisions_v26 (
    profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
    version INTEGER NOT NULL CHECK (version > 0),
    name TEXT NOT NULL CHECK (length(name) > 0),
    description TEXT NOT NULL CHECK (length(description) > 0),
    instructions_markdown TEXT NOT NULL CHECK (length(instructions_markdown) > 0),
    monitor_interval_ms INTEGER NOT NULL CHECK (monitor_interval_ms > 0),
    commands_json TEXT NOT NULL
        CHECK (json_valid(commands_json) AND json_type(commands_json) = 'array'),
    adapter_context_files_json TEXT NOT NULL
        CHECK (
            json_valid(adapter_context_files_json)
            AND json_type(adapter_context_files_json) = 'array'
        ),
    source TEXT NOT NULL CHECK (source IN ('factory', 'user', 'reset')),
    updated_by TEXT NOT NULL CHECK (length(updated_by) > 0),
    created_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (profile_id, version)
) STRICT;

INSERT INTO orchestrator_workflow_profile_revisions_v26 (
    profile_id, version, name, description, instructions_markdown,
    monitor_interval_ms, commands_json, adapter_context_files_json,
    source, updated_by, created_at_unix_ms
)
SELECT profile_id, version, name, description, instructions_markdown,
       monitor_interval_ms, commands_json, adapter_context_files_json,
       source, updated_by, created_at_unix_ms
FROM orchestrator_workflow_profile_revisions;

CREATE TABLE orchestrator_workflow_profile_current_v26 (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    profile_id TEXT NOT NULL DEFAULT 'yard:standard-orchestrator'
        CHECK (profile_id = 'yard:standard-orchestrator'),
    current_version INTEGER NOT NULL CHECK (current_version > 0),
    updated_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, current_version)
        REFERENCES orchestrator_workflow_profile_revisions_v26(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO orchestrator_workflow_profile_current_v26 (
    singleton_id, profile_id, current_version, updated_at_unix_ms
)
SELECT singleton_id, 'yard:standard-orchestrator',
       current_version, updated_at_unix_ms
FROM orchestrator_workflow_profile_current;

CREATE TABLE orchestrator_workflow_profiles_v26 (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    current_version INTEGER NOT NULL CHECK (current_version > 0),
    updated_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (id, current_version)
        REFERENCES orchestrator_workflow_profile_revisions_v26(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO orchestrator_workflow_profiles_v26 (
    id, current_version, updated_at_unix_ms
)
SELECT id, current_version, updated_at_unix_ms
FROM orchestrator_workflow_profiles;

CREATE TABLE project_workflow_profile_pins_v26 (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    pinned_by TEXT NOT NULL CHECK (length(pinned_by) > 0),
    pinned_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES orchestrator_workflow_profile_revisions_v26(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO project_workflow_profile_pins_v26 (
    project_id, profile_id, profile_version, pinned_by, pinned_at_unix_ms
)
SELECT project_id, profile_id, profile_version, pinned_by, pinned_at_unix_ms
FROM project_workflow_profile_pins;

CREATE TABLE yard_orchestrator_v26 (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    worker_id TEXT UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    workflow_profile_id TEXT NOT NULL DEFAULT 'yard:standard-orchestrator'
        CHECK (workflow_profile_id = 'yard:standard-orchestrator'),
    workflow_profile_version INTEGER NOT NULL CHECK (workflow_profile_version > 0),
    FOREIGN KEY (workflow_profile_id, workflow_profile_version)
        REFERENCES orchestrator_workflow_profile_revisions_v26(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO yard_orchestrator_v26 (
    singleton_id, worker_id, version, created_at_unix_ms, updated_at_unix_ms,
    workflow_profile_id, workflow_profile_version
)
SELECT singleton_id, worker_id, version, created_at_unix_ms, updated_at_unix_ms,
       'yard:standard-orchestrator', workflow_profile_version
FROM yard_orchestrator;

CREATE TABLE yard_orchestrator_configure_commands_v26 (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_worker_version INTEGER NOT NULL CHECK (expected_worker_version > 0),
    expected_orchestrator_version INTEGER NOT NULL
        CHECK (expected_orchestrator_version > 0),
    replaced_worker_id TEXT
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_orchestrator_version INTEGER NOT NULL
        CHECK (result_orchestrator_version > 0),
    finished_at_unix_ms INTEGER NOT NULL,
    workflow_profile_id TEXT NOT NULL DEFAULT 'yard:standard-orchestrator'
        CHECK (workflow_profile_id = 'yard:standard-orchestrator'),
    workflow_profile_version INTEGER NOT NULL CHECK (workflow_profile_version > 0),
    FOREIGN KEY (workflow_profile_id, workflow_profile_version)
        REFERENCES orchestrator_workflow_profile_revisions_v26(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO yard_orchestrator_configure_commands_v26 (
    command_id, worker_id, expected_worker_version,
    expected_orchestrator_version, replaced_worker_id,
    result_orchestrator_version, finished_at_unix_ms,
    workflow_profile_id, workflow_profile_version
)
SELECT command_id, worker_id, expected_worker_version,
       expected_orchestrator_version, replaced_worker_id,
       result_orchestrator_version, finished_at_unix_ms,
       'yard:standard-orchestrator', workflow_profile_version
FROM yard_orchestrator_configure_commands;

DROP TABLE project_workflow_profile_pins;
DROP TABLE orchestrator_workflow_profiles;
DROP TABLE orchestrator_workflow_profile_current;
DROP TABLE yard_orchestrator_configure_commands;
DROP TABLE yard_orchestrator;
DROP TABLE orchestrator_workflow_profile_revisions;

ALTER TABLE orchestrator_workflow_profile_revisions_v26
    RENAME TO orchestrator_workflow_profile_revisions;
ALTER TABLE orchestrator_workflow_profile_current_v26
    RENAME TO orchestrator_workflow_profile_current;
ALTER TABLE orchestrator_workflow_profiles_v26
    RENAME TO orchestrator_workflow_profiles;
ALTER TABLE project_workflow_profile_pins_v26
    RENAME TO project_workflow_profile_pins;
ALTER TABLE yard_orchestrator_v26
    RENAME TO yard_orchestrator;
ALTER TABLE yard_orchestrator_configure_commands_v26
    RENAME TO yard_orchestrator_configure_commands;

CREATE TABLE project_orchestrator_replacement_workflow_pins (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES project_orchestrator_replacement_commands(command_id)
        ON DELETE RESTRICT,
    profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES orchestrator_workflow_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO project_orchestrator_replacement_workflow_pins (
    command_id, profile_id, profile_version
)
SELECT replacement.command_id, pin.profile_id, pin.profile_version
  FROM project_orchestrator_replacement_commands replacement
  JOIN project_workflow_profile_pins pin ON pin.project_id = replacement.project_id;

CREATE TRIGGER orchestrator_workflow_profile_revision_immutable_update
BEFORE UPDATE ON orchestrator_workflow_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'orchestrator workflow profile revisions are immutable');
END;

CREATE TRIGGER orchestrator_workflow_profile_revision_immutable_delete
BEFORE DELETE ON orchestrator_workflow_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'orchestrator workflow profile revisions are immutable');
END;

CREATE TRIGGER profile_project_creation_workflow_pin_insert
BEFORE INSERT ON profile_project_creation_commands
WHEN NOT EXISTS (
    SELECT 1
      FROM orchestrator_workflow_profile_revisions
     WHERE profile_id = NEW.workflow_profile_id
       AND version = NEW.workflow_profile_version
)
BEGIN
    SELECT RAISE(ABORT, 'profile project creation workflow revision does not exist');
END;

CREATE TRIGGER profile_project_creation_workflow_pin_update
BEFORE UPDATE OF workflow_profile_id, workflow_profile_version
ON profile_project_creation_commands
WHEN NOT EXISTS (
    SELECT 1
      FROM orchestrator_workflow_profile_revisions
     WHERE profile_id = NEW.workflow_profile_id
       AND version = NEW.workflow_profile_version
)
BEGIN
    SELECT RAISE(ABORT, 'profile project creation workflow revision does not exist');
END;

CREATE TRIGGER workspace_project_creation_workflow_pin_insert
BEFORE INSERT ON workspace_project_creation_commands
WHEN NOT EXISTS (
    SELECT 1
      FROM orchestrator_workflow_profile_revisions
     WHERE profile_id = NEW.workflow_profile_id
       AND version = NEW.workflow_profile_version
)
BEGIN
    SELECT RAISE(ABORT, 'workspace project creation workflow revision does not exist');
END;

CREATE TRIGGER workspace_project_creation_workflow_pin_update
BEFORE UPDATE OF workflow_profile_id, workflow_profile_version
ON workspace_project_creation_commands
WHEN NOT EXISTS (
    SELECT 1
      FROM orchestrator_workflow_profile_revisions
     WHERE profile_id = NEW.workflow_profile_id
       AND version = NEW.workflow_profile_version
)
BEGIN
    SELECT RAISE(ABORT, 'workspace project creation workflow revision does not exist');
END;

PRAGMA user_version = 26;

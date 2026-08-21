ALTER TABLE orchestrator_workflow_profile_revisions
ADD COLUMN profile_id TEXT NOT NULL DEFAULT 'yard:standard-orchestrator';

ALTER TABLE orchestrator_workflow_profile_revisions
ADD COLUMN name TEXT NOT NULL DEFAULT 'Legacy Yard Orchestrator Workflow';

ALTER TABLE orchestrator_workflow_profile_revisions
ADD COLUMN description TEXT NOT NULL DEFAULT
    'Workflow revision migrated from the singleton orchestrator profile.';

ALTER TABLE orchestrator_workflow_profile_revisions
ADD COLUMN commands_json TEXT NOT NULL DEFAULT '[]';

ALTER TABLE orchestrator_workflow_profile_revisions
ADD COLUMN adapter_context_files_json TEXT NOT NULL DEFAULT '[]';

CREATE UNIQUE INDEX orchestrator_workflow_profile_revision_identity
ON orchestrator_workflow_profile_revisions (profile_id, version);

CREATE TABLE orchestrator_workflow_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    current_version INTEGER NOT NULL CHECK (current_version > 0),
    updated_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (id, current_version)
        REFERENCES orchestrator_workflow_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO orchestrator_workflow_profiles (
    id, current_version, updated_at_unix_ms
)
SELECT 'yard:standard-orchestrator', current_version, updated_at_unix_ms
FROM orchestrator_workflow_profile_current
WHERE singleton_id = 1;

CREATE TABLE project_workflow_profile_pins (
    project_id TEXT PRIMARY KEY NOT NULL
        REFERENCES projects(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    pinned_by TEXT NOT NULL CHECK (length(pinned_by) > 0),
    pinned_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES orchestrator_workflow_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

INSERT INTO project_workflow_profile_pins (
    project_id, profile_id, profile_version, pinned_by, pinned_at_unix_ms
)
SELECT p.id, 'yard:standard-orchestrator', current.current_version,
       'yard:migration', p.created_at_unix_ms
FROM projects p
CROSS JOIN orchestrator_workflow_profile_current current
WHERE current.singleton_id = 1;

UPDATE project_orchestrator_transfer_commands AS transfer
SET result_project_json = json_set(
    transfer.result_project_json,
    '$.workflow_profile',
    json((
        SELECT json_object(
            'profile_id', pin.profile_id,
            'profile_version', CAST(pin.profile_version AS TEXT),
            'pinned_by', pin.pinned_by,
            'pinned_at_unix_ms', pin.pinned_at_unix_ms
        )
        FROM project_workflow_profile_pins pin
        WHERE pin.project_id = transfer.project_id
    ))
)
WHERE json_type(transfer.result_project_json, '$.workflow_profile') IS NULL;

ALTER TABLE profile_project_creation_commands
ADD COLUMN workflow_profile_id TEXT NOT NULL DEFAULT 'yard:standard-orchestrator';

ALTER TABLE profile_project_creation_commands
ADD COLUMN workflow_profile_version INTEGER NOT NULL DEFAULT 1
    CHECK (workflow_profile_version > 0);

ALTER TABLE workspace_project_creation_commands
ADD COLUMN workflow_profile_id TEXT NOT NULL DEFAULT 'yard:standard-orchestrator';

ALTER TABLE workspace_project_creation_commands
ADD COLUMN workflow_profile_version INTEGER NOT NULL DEFAULT 1
    CHECK (workflow_profile_version > 0);

UPDATE profile_project_creation_commands
SET workflow_profile_version = (
    SELECT current_version
    FROM orchestrator_workflow_profile_current
    WHERE singleton_id = 1
)
WHERE finished_at_unix_ms IS NULL;

UPDATE workspace_project_creation_commands
SET workflow_profile_version = (
    SELECT current_version
    FROM orchestrator_workflow_profile_current
    WHERE singleton_id = 1
)
WHERE finished_at_unix_ms IS NULL;

PRAGMA user_version = 26;

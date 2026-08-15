CREATE TABLE command_acknowledgements_v18 (
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
                'automation_run_now'
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

INSERT INTO command_acknowledgements_v18
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v18
    RENAME TO command_acknowledgements;

CREATE TABLE automations (
    id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(id) = 36
            AND lower(id) = id
            AND id NOT GLOB '*[^0-9a-f-]*'
            AND substr(id, 9, 1) = '-'
            AND substr(id, 14, 1) = '-'
            AND substr(id, 19, 1) = '-'
            AND substr(id, 24, 1) = '-'
        ),
    name TEXT NOT NULL CHECK (length(name) > 0),
    scope_kind TEXT NOT NULL
        CHECK (
            scope_kind IN (
                'yard_orchestrator',
                'project_orchestrator',
                'workstream_coordination_node'
            )
        ),
    scope_project_id TEXT
        REFERENCES projects(id) ON DELETE RESTRICT,
    scope_node_id TEXT
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    schedule_hour INTEGER NOT NULL
        CHECK (schedule_hour BETWEEN 0 AND 23),
    schedule_minute INTEGER NOT NULL
        CHECK (schedule_minute BETWEEN 0 AND 59),
    schedule_timezone TEXT NOT NULL CHECK (length(schedule_timezone) > 0),
    prompt_template TEXT NOT NULL CHECK (length(prompt_template) > 0),
    state TEXT NOT NULL CHECK (state IN ('active', 'paused')),
    next_run_at_unix_ms INTEGER,
    version INTEGER NOT NULL CHECK (version > 0),
    created_by TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (
            scope_kind = 'yard_orchestrator'
            AND scope_project_id IS NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'project_orchestrator'
            AND scope_project_id IS NOT NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'workstream_coordination_node'
            AND scope_project_id IS NULL
            AND scope_node_id IS NOT NULL
        )
    ),
    CHECK (
        (state = 'active' AND next_run_at_unix_ms IS NOT NULL)
        OR (state = 'paused' AND next_run_at_unix_ms IS NULL)
    )
) STRICT;

CREATE TABLE automation_placements (
    automation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES automations(id) ON DELETE CASCADE,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE automation_selected_projects (
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    PRIMARY KEY (automation_id, project_id)
) STRICT;

CREATE INDEX automations_by_next_run
ON automations(state, next_run_at_unix_ms, id);

CREATE INDEX automation_selected_projects_by_project
ON automation_selected_projects(project_id, automation_id);

CREATE TABLE automation_create_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    automation_id TEXT NOT NULL UNIQUE
        REFERENCES automations(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    scope_kind TEXT NOT NULL
        CHECK (
            scope_kind IN (
                'yard_orchestrator',
                'project_orchestrator',
                'workstream_coordination_node'
            )
        ),
    scope_project_id TEXT
        REFERENCES projects(id) ON DELETE RESTRICT,
    scope_node_id TEXT
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    schedule_hour INTEGER NOT NULL
        CHECK (schedule_hour BETWEEN 0 AND 23),
    schedule_minute INTEGER NOT NULL
        CHECK (schedule_minute BETWEEN 0 AND 59),
    schedule_timezone TEXT NOT NULL,
    prompt_template TEXT NOT NULL,
    next_run_at_unix_ms INTEGER NOT NULL,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    result_version INTEGER NOT NULL CHECK (result_version > 0),
    CHECK (
        (
            scope_kind = 'yard_orchestrator'
            AND scope_project_id IS NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'project_orchestrator'
            AND scope_project_id IS NOT NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'workstream_coordination_node'
            AND scope_project_id IS NULL
            AND scope_node_id IS NOT NULL
        )
    )
) STRICT;

CREATE TABLE automation_create_command_projects (
    command_id TEXT NOT NULL
        REFERENCES automation_create_commands(command_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    PRIMARY KEY (command_id, project_id)
) STRICT;

CREATE TABLE automation_update_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE RESTRICT,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    name TEXT NOT NULL,
    scope_kind TEXT NOT NULL
        CHECK (
            scope_kind IN (
                'yard_orchestrator',
                'project_orchestrator',
                'workstream_coordination_node'
            )
        ),
    scope_project_id TEXT
        REFERENCES projects(id) ON DELETE RESTRICT,
    scope_node_id TEXT
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    schedule_hour INTEGER NOT NULL
        CHECK (schedule_hour BETWEEN 0 AND 23),
    schedule_minute INTEGER NOT NULL
        CHECK (schedule_minute BETWEEN 0 AND 59),
    schedule_timezone TEXT NOT NULL,
    prompt_template TEXT NOT NULL,
    requested_next_run_at_unix_ms INTEGER,
    result_version INTEGER NOT NULL CHECK (result_version > 0),
    CHECK (
        (
            scope_kind = 'yard_orchestrator'
            AND scope_project_id IS NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'project_orchestrator'
            AND scope_project_id IS NOT NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'workstream_coordination_node'
            AND scope_project_id IS NULL
            AND scope_node_id IS NOT NULL
        )
    )
) STRICT;

CREATE TABLE automation_update_command_projects (
    command_id TEXT NOT NULL
        REFERENCES automation_update_commands(command_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    PRIMARY KEY (command_id, project_id)
) STRICT;

CREATE TABLE automation_placement_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE RESTRICT,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    result_version INTEGER NOT NULL CHECK (result_version > 0)
) STRICT;

CREATE TABLE automation_pause_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE RESTRICT,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    paused INTEGER NOT NULL CHECK (paused IN (0, 1)),
    requested_next_run_at_unix_ms INTEGER,
    result_version INTEGER NOT NULL CHECK (result_version > 0),
    CHECK (
        (paused = 1 AND requested_next_run_at_unix_ms IS NULL)
        OR (paused = 0 AND requested_next_run_at_unix_ms IS NOT NULL)
    )
) STRICT;

CREATE TABLE automation_runs (
    id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(id) = 36
            AND lower(id) = id
            AND id NOT GLOB '*[^0-9a-f-]*'
            AND substr(id, 9, 1) = '-'
            AND substr(id, 14, 1) = '-'
            AND substr(id, 19, 1) = '-'
            AND substr(id, 24, 1) = '-'
        ),
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE RESTRICT,
    scope_kind TEXT NOT NULL
        CHECK (
            scope_kind IN (
                'yard_orchestrator',
                'project_orchestrator',
                'workstream_coordination_node'
            )
        ),
    scope_project_id TEXT
        REFERENCES projects(id) ON DELETE RESTRICT,
    scope_node_id TEXT
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    automation_version INTEGER NOT NULL CHECK (automation_version > 0),
    version INTEGER NOT NULL CHECK (version > 0),
    trigger TEXT NOT NULL CHECK (trigger IN ('manual', 'scheduled')),
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'submitted', 'failed', 'ambiguous')),
    prompt_template TEXT NOT NULL,
    dispatch_command_id TEXT NOT NULL UNIQUE,
    requested_by TEXT NOT NULL,
    scheduled_for_unix_ms INTEGER,
    next_due_after_claim_unix_ms INTEGER,
    runtime_status TEXT,
    error_message TEXT,
    submitted_at_unix_ms INTEGER,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (
            scope_kind = 'yard_orchestrator'
            AND scope_project_id IS NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'project_orchestrator'
            AND scope_project_id IS NOT NULL
            AND scope_node_id IS NULL
        )
        OR (
            scope_kind = 'workstream_coordination_node'
            AND scope_project_id IS NULL
            AND scope_node_id IS NOT NULL
        )
    ),
    CHECK (
        (
            trigger = 'manual'
            AND scheduled_for_unix_ms IS NULL
            AND next_due_after_claim_unix_ms IS NULL
        )
        OR (
            trigger = 'scheduled'
            AND scheduled_for_unix_ms IS NOT NULL
            AND next_due_after_claim_unix_ms IS NOT NULL
        )
    ),
    CHECK (
        (
            status = 'pending'
            AND runtime_status IS NULL
            AND error_message IS NULL
            AND submitted_at_unix_ms IS NULL
        )
        OR (
            status = 'submitted'
            AND runtime_status IS NOT NULL
            AND error_message IS NULL
            AND submitted_at_unix_ms IS NOT NULL
        )
        OR (
            status IN ('failed', 'ambiguous')
            AND runtime_status IS NULL
            AND error_message IS NOT NULL
            AND submitted_at_unix_ms IS NULL
        )
    )
) STRICT;

CREATE TABLE automation_run_projects (
    run_id TEXT NOT NULL
        REFERENCES automation_runs(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    PRIMARY KEY (run_id, project_id)
) STRICT;

CREATE UNIQUE INDEX one_pending_run_per_automation
ON automation_runs(automation_id)
WHERE status = 'pending';

CREATE UNIQUE INDEX one_scheduled_run_per_occurrence
ON automation_runs(automation_id, scheduled_for_unix_ms)
WHERE trigger = 'scheduled';

CREATE INDEX automation_runs_by_automation
ON automation_runs(automation_id, created_at_unix_ms DESC, id DESC);

CREATE INDEX automation_runs_by_status
ON automation_runs(status, created_at_unix_ms, id);

CREATE TABLE automation_run_now_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    automation_id TEXT NOT NULL
        REFERENCES automations(id) ON DELETE RESTRICT,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    run_id TEXT NOT NULL UNIQUE
        REFERENCES automation_runs(id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER automations_workstream_scope_insert
BEFORE INSERT ON automations
WHEN NEW.scope_kind = 'workstream_coordination_node'
     AND NOT EXISTS (
         SELECT 1
           FROM coordination_nodes
          WHERE id = NEW.scope_node_id AND kind = 'workstream'
     )
BEGIN
    SELECT RAISE(ABORT, 'automation scope requires a workstream coordination node');
END;

CREATE TRIGGER automations_workstream_scope_update
BEFORE UPDATE OF scope_kind, scope_node_id ON automations
WHEN NEW.scope_kind = 'workstream_coordination_node'
     AND NOT EXISTS (
         SELECT 1
           FROM coordination_nodes
          WHERE id = NEW.scope_node_id AND kind = 'workstream'
     )
BEGIN
    SELECT RAISE(ABORT, 'automation scope requires a workstream coordination node');
END;

CREATE TRIGGER automation_create_commands_workstream_scope
BEFORE INSERT ON automation_create_commands
WHEN NEW.scope_kind = 'workstream_coordination_node'
     AND NOT EXISTS (
         SELECT 1
           FROM coordination_nodes
          WHERE id = NEW.scope_node_id AND kind = 'workstream'
     )
BEGIN
    SELECT RAISE(ABORT, 'automation scope requires a workstream coordination node');
END;

CREATE TRIGGER automation_update_commands_workstream_scope
BEFORE INSERT ON automation_update_commands
WHEN NEW.scope_kind = 'workstream_coordination_node'
     AND NOT EXISTS (
         SELECT 1
           FROM coordination_nodes
          WHERE id = NEW.scope_node_id AND kind = 'workstream'
     )
BEGIN
    SELECT RAISE(ABORT, 'automation scope requires a workstream coordination node');
END;

CREATE TRIGGER automation_runs_workstream_scope
BEFORE INSERT ON automation_runs
WHEN NEW.scope_kind = 'workstream_coordination_node'
     AND NOT EXISTS (
         SELECT 1
           FROM coordination_nodes
          WHERE id = NEW.scope_node_id AND kind = 'workstream'
     )
BEGIN
    SELECT RAISE(ABORT, 'automation scope requires a workstream coordination node');
END;

CREATE TRIGGER automation_selected_project_scope
BEFORE INSERT ON automation_selected_projects
WHEN (
    SELECT scope_kind FROM automations WHERE id = NEW.automation_id
) = 'project_orchestrator'
AND NEW.project_id <> (
    SELECT scope_project_id FROM automations WHERE id = NEW.automation_id
)
BEGIN
    SELECT RAISE(ABORT, 'project automation selection must match its scope');
END;

CREATE TRIGGER automation_selected_workstream_project
BEFORE INSERT ON automation_selected_projects
WHEN (
    SELECT scope_kind FROM automations WHERE id = NEW.automation_id
) = 'workstream_coordination_node'
AND NOT EXISTS (
    SELECT 1
      FROM automations a
      JOIN coordination_node_projects cnp
        ON cnp.node_id = a.scope_node_id
     WHERE a.id = NEW.automation_id
       AND cnp.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'workstream automation project is not attached');
END;

CREATE TRIGGER automation_create_command_project_scope
BEFORE INSERT ON automation_create_command_projects
WHEN (
    SELECT scope_kind
      FROM automation_create_commands
     WHERE command_id = NEW.command_id
) = 'project_orchestrator'
AND NEW.project_id <> (
    SELECT scope_project_id
      FROM automation_create_commands
     WHERE command_id = NEW.command_id
)
BEGIN
    SELECT RAISE(ABORT, 'project automation selection must match its scope');
END;

CREATE TRIGGER automation_create_command_workstream_project
BEFORE INSERT ON automation_create_command_projects
WHEN (
    SELECT scope_kind
      FROM automation_create_commands
     WHERE command_id = NEW.command_id
) = 'workstream_coordination_node'
AND NOT EXISTS (
    SELECT 1
      FROM automation_create_commands acc
      JOIN coordination_node_projects cnp
        ON cnp.node_id = acc.scope_node_id
     WHERE acc.command_id = NEW.command_id
       AND cnp.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'workstream automation project is not attached');
END;

CREATE TRIGGER automation_update_command_project_scope
BEFORE INSERT ON automation_update_command_projects
WHEN (
    SELECT scope_kind
      FROM automation_update_commands
     WHERE command_id = NEW.command_id
) = 'project_orchestrator'
AND NEW.project_id <> (
    SELECT scope_project_id
      FROM automation_update_commands
     WHERE command_id = NEW.command_id
)
BEGIN
    SELECT RAISE(ABORT, 'project automation selection must match its scope');
END;

CREATE TRIGGER automation_update_command_workstream_project
BEFORE INSERT ON automation_update_command_projects
WHEN (
    SELECT scope_kind
      FROM automation_update_commands
     WHERE command_id = NEW.command_id
) = 'workstream_coordination_node'
AND NOT EXISTS (
    SELECT 1
      FROM automation_update_commands auc
      JOIN coordination_node_projects cnp
        ON cnp.node_id = auc.scope_node_id
     WHERE auc.command_id = NEW.command_id
       AND cnp.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'workstream automation project is not attached');
END;

PRAGMA user_version = 18;

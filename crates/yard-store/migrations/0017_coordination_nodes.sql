CREATE TABLE command_acknowledgements_v17 (
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
                'coordination_snapshot_request'
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

INSERT INTO command_acknowledgements_v17
SELECT id, command_type, actor, status, error_message,
       created_at_unix_ms, updated_at_unix_ms
FROM command_acknowledgements;

DROP TABLE command_acknowledgements;
ALTER TABLE command_acknowledgements_v17
    RENAME TO command_acknowledgements;

CREATE TABLE coordination_nodes (
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
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('workstream', 'knowledge_store')),
    worker_id TEXT UNIQUE
        REFERENCES workers(id) ON DELETE RESTRICT,
    workstream_cwd TEXT,
    knowledge_path TEXT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_by TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (
            kind = 'workstream'
            AND workstream_cwd IS NOT NULL
            AND knowledge_path IS NULL
        )
        OR (
            kind = 'knowledge_store'
            AND workstream_cwd IS NULL
            AND knowledge_path IS NOT NULL
            AND worker_id IS NULL
        )
    )
) STRICT;

CREATE TABLE coordination_node_placements (
    node_id TEXT PRIMARY KEY NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE CASCADE,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE coordination_node_projects (
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    PRIMARY KEY (node_id, project_id)
) STRICT;

CREATE INDEX coordination_nodes_by_created
ON coordination_nodes(created_at_unix_ms, id);

CREATE INDEX coordination_nodes_by_project
ON coordination_node_projects(project_id, node_id);

CREATE TABLE coordination_node_create_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL UNIQUE
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('workstream', 'knowledge_store')),
    name TEXT NOT NULL,
    attached_project_ids TEXT NOT NULL,
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL
) STRICT;

CREATE TABLE coordination_node_update_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    name TEXT NOT NULL,
    attached_project_ids TEXT NOT NULL,
    result_version INTEGER NOT NULL CHECK (result_version > 0)
) STRICT;

CREATE TABLE coordination_node_placement_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    expected_version INTEGER NOT NULL CHECK (expected_version > 0),
    canvas_x REAL NOT NULL,
    canvas_y REAL NOT NULL,
    canvas_width REAL NOT NULL,
    canvas_height REAL NOT NULL,
    result_version INTEGER NOT NULL CHECK (result_version > 0)
) STRICT;

CREATE TABLE coordination_node_provision_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    expected_node_version INTEGER NOT NULL CHECK (expected_node_version > 0),
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    result_node_version INTEGER NOT NULL CHECK (result_node_version > 0),
    finished_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE coordination_node_prompt_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_node_version INTEGER NOT NULL CHECK (expected_node_version > 0),
    prompt_text TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_pane_id TEXT NOT NULL,
    result_runtime_status TEXT,
    submitted_at_unix_ms INTEGER,
    CHECK (
        (result_runtime_status IS NULL AND submitted_at_unix_ms IS NULL)
        OR (
            result_runtime_status IS NOT NULL
            AND submitted_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

CREATE TABLE coordination_node_route_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_node_version INTEGER NOT NULL CHECK (expected_node_version > 0),
    target_project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    target_orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    expected_project_version INTEGER NOT NULL CHECK (expected_project_version > 0),
    prompt_text TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    runtime_pane_id TEXT NOT NULL,
    result_runtime_status TEXT,
    submitted_at_unix_ms INTEGER,
    CHECK (
        (result_runtime_status IS NULL AND submitted_at_unix_ms IS NULL)
        OR (
            result_runtime_status IS NOT NULL
            AND submitted_at_unix_ms IS NOT NULL
        )
    )
) STRICT;

CREATE INDEX coordination_node_routes_by_created
ON coordination_node_route_commands(node_id, command_id);

CREATE TABLE coordination_snapshots (
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
    node_id TEXT NOT NULL
        REFERENCES coordination_nodes(id) ON DELETE RESTRICT,
    command_id TEXT NOT NULL UNIQUE
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    expected_node_version INTEGER NOT NULL CHECK (expected_node_version > 0),
    folder_path TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    UNIQUE (node_id, id)
) STRICT;

CREATE INDEX coordination_snapshots_by_created
ON coordination_snapshots(node_id, created_at_unix_ms DESC, id DESC);

CREATE TABLE coordination_snapshot_projects (
    snapshot_id TEXT NOT NULL
        REFERENCES coordination_snapshots(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    project_version INTEGER NOT NULL CHECK (project_version > 0),
    orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    folder_path TEXT NOT NULL,
    collection_status TEXT NOT NULL
        CHECK (collection_status IN ('pending', 'collected')),
    delivery_status TEXT NOT NULL
        CHECK (delivery_status IN ('pending', 'submitted', 'failed', 'ambiguous')),
    delivery_error TEXT,
    result_runtime_status TEXT,
    submitted_at_unix_ms INTEGER,
    collected_at_unix_ms INTEGER,
    PRIMARY KEY (snapshot_id, project_id),
    CHECK (
        (collection_status = 'pending' AND collected_at_unix_ms IS NULL)
        OR (collection_status = 'collected' AND collected_at_unix_ms IS NOT NULL)
    ),
    CHECK (
        (
            delivery_status = 'pending'
            AND delivery_error IS NULL
            AND result_runtime_status IS NULL
            AND submitted_at_unix_ms IS NULL
        )
        OR (
            delivery_status = 'submitted'
            AND delivery_error IS NULL
            AND result_runtime_status IS NOT NULL
            AND submitted_at_unix_ms IS NOT NULL
        )
        OR (
            delivery_status IN ('failed', 'ambiguous')
            AND delivery_error IS NOT NULL
            AND result_runtime_status IS NULL
            AND submitted_at_unix_ms IS NULL
        )
    )
) STRICT;

PRAGMA user_version = 17;

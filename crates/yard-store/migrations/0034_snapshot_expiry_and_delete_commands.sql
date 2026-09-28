CREATE TABLE coordination_snapshot_projects_v29 (
    snapshot_id TEXT NOT NULL
        REFERENCES coordination_snapshots(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    project_version INTEGER NOT NULL CHECK (project_version > 0),
    orchestrator_worker_id TEXT NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    folder_path TEXT NOT NULL,
    collection_status TEXT NOT NULL
        CHECK (collection_status IN ('pending', 'collected', 'abandoned')),
    delivery_status TEXT NOT NULL
        CHECK (delivery_status IN ('pending', 'submitted', 'failed', 'ambiguous')),
    delivery_error TEXT,
    result_runtime_status TEXT,
    submitted_at_unix_ms INTEGER,
    collected_at_unix_ms INTEGER,
    abandoned_at_unix_ms INTEGER,
    abandoned_reason TEXT CHECK (abandoned_reason IN ('expired')),
    PRIMARY KEY (snapshot_id, project_id),
    CHECK (
        (
            collection_status = 'pending'
            AND collected_at_unix_ms IS NULL
            AND abandoned_at_unix_ms IS NULL
            AND abandoned_reason IS NULL
        )
        OR (
            collection_status = 'collected'
            AND collected_at_unix_ms IS NOT NULL
            AND abandoned_at_unix_ms IS NULL
            AND abandoned_reason IS NULL
        )
        OR (
            collection_status = 'abandoned'
            AND collected_at_unix_ms IS NULL
            AND abandoned_at_unix_ms IS NOT NULL
            AND abandoned_reason IS NOT NULL
            AND delivery_status <> 'pending'
        )
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

INSERT INTO coordination_snapshot_projects_v29 (
    snapshot_id, project_id, project_version, orchestrator_worker_id,
    folder_path, collection_status, delivery_status, delivery_error,
    result_runtime_status, submitted_at_unix_ms, collected_at_unix_ms
)
SELECT snapshot_id, project_id, project_version, orchestrator_worker_id,
       folder_path, collection_status, delivery_status, delivery_error,
       result_runtime_status, submitted_at_unix_ms, collected_at_unix_ms
  FROM coordination_snapshot_projects;

DROP TABLE coordination_snapshot_projects;
ALTER TABLE coordination_snapshot_projects_v29 RENAME TO coordination_snapshot_projects;

CREATE TABLE project_delete_commands (
    command_id TEXT PRIMARY KEY NOT NULL
        REFERENCES command_acknowledgements(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL UNIQUE
        REFERENCES projects(id) ON DELETE RESTRICT,
    archive_json TEXT,
    archive_applied INTEGER NOT NULL CHECK (archive_applied IN (0, 1)),
    CHECK (archive_applied = 0 OR archive_json IS NOT NULL)
) STRICT;

INSERT INTO project_delete_commands (
    command_id, project_id, archive_json, archive_applied
)
SELECT command_id, project_id, NULL, 0
  FROM deleted_projects;

PRAGMA user_version = 34;

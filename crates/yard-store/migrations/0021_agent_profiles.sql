CREATE TABLE IF NOT EXISTS agent_profile_revisions (
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    format_api_version TEXT NOT NULL,
    canonical_manifest_json TEXT NOT NULL
        CHECK (json_valid(canonical_manifest_json)),
    original_manifest_json TEXT NOT NULL
        CHECK (json_valid(original_manifest_json)),
    validation_report_json TEXT NOT NULL
        CHECK (json_valid(validation_report_json)),
    source TEXT NOT NULL
        CHECK (source IN ('worker_profile_migration', 'worker_profile_api', 'agent_profile_api')),
    created_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (profile_id, profile_version),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES worker_profile_revisions(profile_id, version)
        ON DELETE RESTRICT
) STRICT;

CREATE INDEX IF NOT EXISTS agent_profile_revisions_format
ON agent_profile_revisions(format_api_version);

PRAGMA user_version = 21;

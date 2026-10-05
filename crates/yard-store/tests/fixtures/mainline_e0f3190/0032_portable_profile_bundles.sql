CREATE TABLE agent_profile_files (
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    path TEXT NOT NULL,
    media_type TEXT NOT NULL,
    content TEXT NOT NULL,
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    PRIMARY KEY (profile_id, profile_version, path),
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES agent_profile_revisions(profile_id, profile_version)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE profile_launch_audits (
    command_id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL,
    profile_version INTEGER NOT NULL CHECK (profile_version > 0),
    plan_json TEXT NOT NULL CHECK (json_valid(plan_json)),
    created_at_unix_ms INTEGER NOT NULL,
    FOREIGN KEY (profile_id, profile_version)
        REFERENCES agent_profile_revisions(profile_id, profile_version)
        ON DELETE RESTRICT
) STRICT;

PRAGMA user_version = 32;

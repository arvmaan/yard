CREATE TABLE token_spend_settings (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    superintendent_auto_requests_project_summaries INTEGER NOT NULL
        CHECK (superintendent_auto_requests_project_summaries IN (0, 1)),
    project_orchestrators_auto_request_worker_summaries INTEGER NOT NULL
        CHECK (project_orchestrators_auto_request_worker_summaries IN (0, 1)),
    scheduled_automatic_summaries INTEGER NOT NULL
        CHECK (scheduled_automatic_summaries IN (0, 1)),
    version INTEGER NOT NULL CHECK (version > 0),
    updated_by TEXT NOT NULL CHECK (length(updated_by) > 0),
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

INSERT INTO token_spend_settings (
    id,
    superintendent_auto_requests_project_summaries,
    project_orchestrators_auto_request_worker_summaries,
    scheduled_automatic_summaries,
    version,
    updated_by,
    updated_at_unix_ms
) VALUES (1, 0, 0, 0, 1, 'yard:migration', 0);

CREATE TABLE automatic_summary_request_watermarks (
    request_kind TEXT NOT NULL
        CHECK (request_kind IN ('superintendent_project', 'project_worker')),
    target_id TEXT NOT NULL CHECK (length(target_id) BETWEEN 1 AND 120),
    claimed_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (request_kind, target_id)
) STRICT;

PRAGMA user_version = 19;

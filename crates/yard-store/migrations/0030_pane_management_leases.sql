CREATE TABLE yard_installation (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    installation_uuid TEXT NOT NULL UNIQUE,
    created_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE pane_management_batches (
    command_id TEXT PRIMARY KEY NOT NULL,
    actor TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'succeeded')),
    result_json TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (
        (status = 'pending' AND result_json IS NULL)
        OR (status = 'succeeded' AND result_json IS NOT NULL)
    )
) STRICT;

CREATE TABLE pane_management_leases (
    worker_id TEXT PRIMARY KEY NOT NULL
        REFERENCES workers(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL
        REFERENCES projects(id) ON DELETE RESTRICT,
    allocation_id TEXT NOT NULL UNIQUE
        REFERENCES worker_allocations(id) ON DELETE RESTRICT,
    installation_uuid TEXT NOT NULL,
    herdr_session TEXT NOT NULL,
    pane_id TEXT NOT NULL,
    pane_instance_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    lease_token TEXT NOT NULL,
    expires_at_unix_ms INTEGER NOT NULL,
    acquisition_request_id TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('active', 'recovery_required')),
    renewal_failures INTEGER NOT NULL DEFAULT 0 CHECK (renewal_failures >= 0),
    renew_after_unix_ms INTEGER NOT NULL,
    last_error_code TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    UNIQUE (herdr_session, pane_id, pane_instance_id),
    FOREIGN KEY (installation_uuid)
        REFERENCES yard_installation(installation_uuid) ON DELETE RESTRICT
) STRICT;

CREATE INDEX pane_management_leases_renewal
    ON pane_management_leases(status, renew_after_unix_ms, expires_at_unix_ms);

PRAGMA user_version = 30;

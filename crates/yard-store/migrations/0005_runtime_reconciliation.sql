ALTER TABLE worker_runtime_bindings
    ADD COLUMN observed_status TEXT NOT NULL DEFAULT 'unknown'
        CHECK (observed_status IN ('idle', 'working', 'blocked', 'done', 'unknown'));

ALTER TABLE worker_runtime_bindings
    ADD COLUMN process_state TEXT NOT NULL DEFAULT 'unknown'
        CHECK (process_state IN ('running', 'exited', 'unknown'));

ALTER TABLE worker_runtime_bindings
    ADD COLUMN state_change_sequence INTEGER NOT NULL DEFAULT 0
        CHECK (state_change_sequence >= 0);

ALTER TABLE worker_runtime_bindings
    ADD COLUMN runtime_revision INTEGER NOT NULL DEFAULT 0
        CHECK (runtime_revision >= 0);

PRAGMA user_version = 5;

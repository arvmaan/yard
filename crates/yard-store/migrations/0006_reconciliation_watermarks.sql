CREATE TABLE runtime_reconciliation_watermarks (
    adapter TEXT NOT NULL,
    runtime_session TEXT NOT NULL,
    observed_at_unix_ms INTEGER NOT NULL CHECK (observed_at_unix_ms >= 0),
    PRIMARY KEY (adapter, runtime_session)
) STRICT;

PRAGMA user_version = 6;

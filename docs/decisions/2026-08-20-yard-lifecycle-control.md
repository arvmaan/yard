# ADR: Yard Lifecycle Control

- Status: Proposed
- Date: 2026-08-20

## Context

Yard needs one local executable that can run in a terminal or as a managed
background process. A listening HTTP port, process name, health response, or
persisted PID does not prove lifecycle ownership and must not authorize
shutdown. Concurrent launches, stale files, database aliases, and interrupted
shutdown also must not create two owners for one SQLite database.

## Decision

The `yard-server` Cargo package produces one executable named `yard`. Public
commands are `start`, `status`, `stop`, and explicit foreground `run`; bare
`yard` prints help. Browser launch follows a verified successful start, uses a
single argv value without a shell, and is a bounded non-fatal operation.

Lifecycle identity is scoped to the normalized physical database path.
Relative paths and existing symbolic-link aliases converge on one identity.
Hard-linked database files are rejected because they cannot be mapped back to
one stable pathname before and after initial database creation.
Each identity has an owner-only runtime directory containing persistent launch
and instance lock files, a managed log, atomic instance metadata, and a Unix
control socket.

The short-lived launch lock serializes launch, stop, and stale-state repair.
The process-lifetime instance lock prevents split ownership and is never
unlinked. The SQLite sidecar lock remains an independent store boundary and
uses the same normalized database path.

Control requests require a matching protocol version, launch ID, random
per-launch secret, and effective peer UID. Persisted PIDs are display metadata
only. `yard stop` requests authenticated shutdown from a managed owner and
never signals a PID read from disk. It refuses foreground ownership.

Live-owner cleanup removes metadata only after atomically quarantining and
verifying the matching launch. Stale repair similarly quarantines the exact
directory entry before validating and unlinking it. Missing or unreachable
control is reclaimable only while holding the launch lock and observing the
instance lock free. Symlinks, wrong types, foreign ownership, insecure modes,
active unidentified sockets, and held locks fail closed.

Managed and foreground execution share one server runner. It binds TCP before
opening SQLite or starting background work. SIGINT, SIGTERM, authenticated
stop, control-listener failure, and HTTP exit converge on bounded shutdown,
including terminal WebSocket cancellation and database-lock retention through
blocking work.

## Consequences

- Linux and macOS receive managed lifecycle and browser integration; Windows
  lifecycle support remains out of scope.
- Same-UID processes are inside the local trust boundary, but accidental or
  cross-user control cannot authenticate.
- Stop timeout reports failure without escalating to a signal.
- A service manager, restart-on-crash policy, log service, auto-update, and
  bundled Herdr runtime remain separate work.

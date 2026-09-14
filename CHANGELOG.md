# Changelog

All notable Yard changes are recorded here. Yard uses semantic versioning once
a release is tagged.

## [Unreleased]

### Added

- Installed `yard` CLI with managed `start`, `status`, and `stop` commands,
  explicit foreground `run`, and a user-local source installer.
- Added durable project archiving with guarded runtime cleanup, active-map
  removal, released workspace ownership, and retained project history.
- Added retained-pane recovery for an exited superintendent without replacing
  its durable Yard identity.
- Added a worker History filter while hiding ended workers from operational
  views by default.

### Changed

- Expanded Herdr compatibility from protocol 19 through protocol 22, including
  Herdr 0.8.2 protocol 20.
- Reconciled interactive workers without provider-session metadata by using
  stable session, workspace, tab, pane, and terminal topology.
- Increased restored terminal and chat history to 1,000 lines and chunked large
  terminal input without splitting Unicode characters.
- Separated recognized agent progress and tool activity into collapsed
  **Agent work** sections while keeping final answers and unrecognized output
  visible.
- Added latest-question markers to chat and terminal context views, and moved
  prior terminal history into an overlay that does not resize the live
  terminal.

### Hardened

- Same-UID, secret-authenticated Unix-socket lifecycle control, lifetime and
  launch locks, stale-state recovery, and bounded SIGINT/SIGTERM shutdown.
- Removed raw pane-input fallback during agent startup so failed agent prompts
  cannot be typed into an exposed shell.
- Added archive dependency checks for active work, project automations, and
  unresolved automation-run snapshots.
- Preserved database identity across hard-linked paths when enforcing the
  single-process ownership lock.

## [0.1.0] - 2026-08-15

### Added

- Spatial project and worker control plane backed by Herdr.
- Durable worker profiles, assignments, handoffs, orchestrators, artifacts,
  completion receipts, relationships, coordination nodes, and automations.
- Dedicated Yard superintendent, project pulse, structured status reports,
  and durable cross-project routing.
- Full-screen chat and terminal workspaces, group orders, Ghostty launch,
  child-agent trees, themes, and map auto-layout.
- Workstream orchestrators and revisioned knowledge-store snapshots.

### Hardened

- Shared automation dispatch locking between HTTP and the scheduler.
- Durable artifact directory entries before metadata commit.
- Target-scoped chat completion handling.
- Consistent modal focus trapping, inert backgrounds, Escape handling, and
  focus restoration.
- Strict TypeScript checking for application and Playwright sources.
- Isolated smoke-test ports and a declared WebSocket harness dependency.

### Known Limitations

- Yard is source-only, single-user local software without authentication.
- Herdr 0.8.0 is the only runtime adapter.
- Interrupted project creation, allocation, or handoff can retain a safety
  reservation without a self-service cancel or resolve workflow.

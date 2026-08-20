# Changelog

All notable Yard changes are recorded here. Yard uses semantic versioning once
a release is tagged.

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
- Installed `yard` CLI with managed `start`, `status`, and `stop` commands,
  explicit foreground `run`, and a user-local source installer.

### Hardened

- Shared automation dispatch locking between HTTP and the scheduler.
- Durable artifact directory entries before metadata commit.
- Target-scoped chat completion handling.
- Consistent modal focus trapping, inert backgrounds, Escape handling, and
  focus restoration.
- Strict TypeScript checking for application and Playwright sources.
- Isolated smoke-test ports and a declared WebSocket harness dependency.
- Same-UID, secret-authenticated Unix-socket lifecycle control, lifetime and
  launch locks, stale-state recovery, and bounded SIGINT/SIGTERM shutdown.

### Known Limitations

- Yard is source-only, single-user local software without authentication.
- Herdr 0.8.0 is the only runtime adapter.
- Interrupted project creation, allocation, or handoff can retain a safety
  reservation without a self-service cancel or resolve workflow.

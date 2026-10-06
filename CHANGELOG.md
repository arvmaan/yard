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
- Added one-click **Complete** for workers: after a five-second Undo window it
  records a minimal receipt (actor, time, and the objective copied on the
  server, with no artifacts or evidence) and, with **Complete and end session**
  on, ends Yard's session in the same request. The evidence-backed form stays
  available as **Complete with details…**, and an empty untyped receipt is still
  rejected.
- Added **End without completion**, which records a `cancelled` assignment and
  its reason instead of a receipt, and one compact sheet for ending or deleting
  a worker that still has active work. The disposition API can also cancel
  the work of a worker stranded by an ambiguous handoff; the web UI does not
  offer that exit yet. Retries replay by command ID.
- Added read-only retained transcripts: when an assignment ends, Yard captures
  the worker's recent terminal output under an identity guard and keeps it
  after the runtime closes, retrying in the background while Herdr is
  unreachable.
- Added workstream archive and delete with one preview-driven confirmation.
  Archive ends the dedicated worker without closing its Herdr tab and pauses
  the workstream's automations; delete archives an active workstream in the
  same request. Neither changes attached projects, and both replay by command
  ID.
- Project archive and delete can now end active workers: the one
  confirmation lists them from a new disposition preview, and each listed
  assignment is recorded as cancelled (`project_archived`) with its session
  ended, its allocation closed, and its transcript captured in the background.
  A changed set returns 409 `project_archive_preview_stale` with a fresh
  preview; retries replay by command ID.
  Running summary workers are listed separately (`summary_worker_assignments`)
  and end with the archive; their summary command is marked failed
  (`project_archived`). While a summary worker is still being allocated the
  archive returns 409 `project_summary_worker_allocating`.
- Project archive can be undone: a 10 s **Undo** toast after a restorable
  archive and an **Archived** shelf with **Restore** call the new
  `POST /api/v1/projects/{id}/restore` (listing: `GET /api/v1/archived`).
  Restore re-binds the orchestrator's Herdr tab when it is still the same pane,
  otherwise restores it unbound, cancels its archive cleanup job, and never
  reopens cancelled assignments. Refusals are 409
  `project_restore_unavailable` with a `reason` (for example
  `herdr_unreachable`); retries replay by command ID. A restored project can be
  archived again.
- Added a read-only storage preview: with `YARD_STORAGE_ROOTS` set,
  `POST /api/v1/storage/scans` sizes Rust, Node, Vite, Gradle, and marked
  workspace build output and registered Git worktrees, and classifies each as
  safe, review, or blocked using Git and workspace source state and in-use
  checks. Workspace roots are recognised only by the marker file names in
  `YARD_STORAGE_WORKSPACE_MARKERS` (unset: workspace classes disabled).
  Nothing is deleted yet.

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
- Project delete now archives an active project in the same request after one
  confirmation, and a retry replays even if another tab archived it first.
  Upgrading repairs databases created by the first version-28 schema, where
  every delete failed with a storage error.
- Archive and delete are no longer refused for knowledge snapshot collection,
  ambiguous handoffs, or ambiguous automation runs; they report pending
  snapshots as background status, and a snapshot still uncollected 24 hours
  after its prompt is marked expired until its files arrive.

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

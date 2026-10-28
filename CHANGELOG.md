# Changelog

All notable Yard changes are recorded here. Yard uses semantic versioning once
a release is tagged.

## [Unreleased]

### Added

- Added image paste into web terminals: a pasted or dropped PNG, JPEG, GIF or
  WebP image is uploaded to `POST <terminal route>/images`, checked against
  its magic bytes and `YARD_IMAGE_PASTE_MAX_BYTES` (10 MiB), stored privately
  under `YARD_IMAGE_PASTE_PATH` (`pastes/` beside the database, pruned after 7
  days and capped at 200 per worker), and its absolute path is typed into the
  attached pane as a bracketed paste and a space for Claude Code or Codex.
  Each path and any accompanying text go out as separate pastes; pruning only
  removes Yard-named files in a marked root, and a paste path inside a git
  checkout or overlapping Yard's other directories is refused at startup.
  Typing waits behind an upload so the path lands first, but control input
  (Ctrl-C, Esc and other single control keys) is sent at once after cancelling
  the pending upload and discarding the typing queued behind it, so an
  interrupt never lands ahead of earlier input; an upload can
  be cancelled from the terminal, the browser abandons it after 60 s, and the
  server bounds the wait for an upload slot (15 s) and the body read (60 s).
- Added Slack DM notifications for one owner (`YARD_SLACK_NOTIFICATIONS=on`):
  blocked agents, work ready for review, and failed or ambiguous commands,
  titles and states only, with the bot token read from Secrets Manager.
- Added answering from Slack (`YARD_SLACK_INBOUND=on` with an app-level token
  in `YARD_SLACK_APP_SECRET_ID`), over an outbound Socket Mode connection with
  no inbound endpoint and only the owner's own DMs accepted: blocked
  notifications carry the agent's current prompt (redacted) with single-use,
  5-minute buttons that re-check the terminal and prompt before typing, and
  never offer "always allow"; `status`, `status <project>`, `blocked`,
  `review` and `help` answer from durable state; any other message goes to
  the Superintendent, or to a project orchestrator for `Project: question`,
  and its status report is posted back in the thread. Every inbound action is
  audited in `slack-audit.jsonl`. The Slack app manifests are now JSON
  (`docs/slack/manifest.json`, `docs/slack/manifest-outbound-only.json`).
- Added Slack conversations: a top-level owner question starts a
  conversation rooted at its thread, and follow-ups in that thread go to the
  same agent with the earlier turns and are answered in the same thread
  (messages queue per thread while the agent is busy, at most 5). Plain
  questions go to a dedicated, lazily started Slack Superintendent; the main
  Superintendent answers if it cannot. Conversations persist in
  `slack-conversations.json` (0600, redacted, 7-day TTL). Owner messages in a
  thread are now routed or queued instead of refused as "not an open
  conversation".
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

- Ctrl+Enter and Shift+Enter in web terminals now reach the agent as distinct
  keys through Herdr `pane.send_keys`, with a carriage-return fallback only
  when Herdr cannot have pressed the key.
- Orchestrator replacement is no longer blocked by a bare interactive shell
  that Herdr restored in the captured tab (inspection failure still refuses),
  and the operator can choose the working directory when the displaced pane
  and worktree are both gone. The field is prefilled only with the workspace's
  worktree checkout, never another pane's directory, and a chosen directory
  that a live pane already uses is refused (`orchestrator_cwd_in_use`) unless
  the operator confirms it explicitly.
- Jump to latest now returns full-screen (alternate-screen, mouse-reporting)
  agents to their latest output, and terminal scroll batches are pipelined
  across the round trip so long scrolls no longer lag.
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

- Every state-changing API request (`POST`, `PUT`, `PATCH` or `DELETE` under
  `/api/`) now passes the loopback Host/Origin guard in one router layer,
  before its route runs or its body is parsed: a cross-site page (foreign
  `Origin`) gets 403 `request_origin_forbidden`, and a DNS-rebinding page
  (foreign `Host`) gets 403 `request_host_forbidden`. This newly covers
  end-session, completion receipts, node archive and delete, worker delete,
  prompts, provisioning and the other commands that had no check of their
  own. The loopback web UI and clients that send no `Origin` on a loopback
  host (the CLI, agents) are unaffected; reads are unchanged.
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

# ADR: Durable Token-Spend Coordination Settings

- Status: Proposed
- Date: 2026-08-18

## Context

Yard has three automatic behaviors that can consume model tokens:

1. the superintendent requesting summaries from project orchestrators;
2. project orchestrators requesting summaries from assigned workers; and
3. scheduled automation dispatches.

These behaviors must be independently opt-in. Manual prompts, manual routes,
and explicit automation `Run now` commands must remain available regardless of
the automatic settings.

Before selecting a persistence model, the relevant paths were traced:

- Runtime status polling has two paths in `web/src/App.tsx`: visible-session
  inventory refreshes call `fetchInventory` in `web/src/api.ts`, then the
  inventory HTTP handler and reconciliation service update Yard's observed
  runtime projection; project workflow-report refreshes call the read-only
  orchestrator status-output helper and reach the output-read method in
  `crates/yard-server/src/intervention_service.rs`. Neither path creates prompt
  or route commands.
- Manual chat prompts and routes start in
  `web/src/AgentChatWorkspace.tsx`, use the explicit POST helpers in
  `web/src/api.ts`, and reach the manual methods in
  `crates/yard-server/src/intervention_service.rs`.
- The attached automation scheduler is
  `AutomationService::run`/`run_due_once` in
  `crates/yard-server/src/automation_service.rs`. It polls every 30 seconds,
  resumes pending runs first, and then claims due scheduled runs.
  `AutomationService::run_now` is a separate explicit path and the store
  permits it for paused automations.
- Durable state is owned by the SQLite `YardStore` implementation in
  `crates/yard-store/src/lib.rs`; schema version 18 had no accepted general
  settings model. Existing browser preferences are local presentation choices
  and are not an enforcement boundary.
- Automation HTTP routes live in `crates/yard-server/src/http/mod.rs`, while
  the corresponding UI state and controls live in `web/src/App.tsx`,
  `web/src/api.ts`, and `web/src/AutomationInspector.tsx`.

## Decision

Add a singleton `token_spend_settings` row to Yard's SQLite store with three
independent booleans:

- `superintendent_auto_requests_project_summaries`
- `project_orchestrators_auto_request_worker_summaries`
- `scheduled_automatic_summaries`

The migration inserts the singleton with every value false. Updates use an
optimistic version and record the actor and update time.

Add `automatic_summary_request_watermarks`, keyed by behavior and durable
target ID. The attached scheduler uses these records to claim no more than one
automatic summary request per target in the configured interval. A claim
checks the corresponding setting in the same SQLite transaction.

Automatic summary dispatch uses server-internal typed methods. Every durable
prompt/route `begin` operation requires an explicit manual or automatic command
source and checks that source's setting in the same immediate SQLite
transaction that creates the command. Public manual prompt and route handlers
continue to pass the manual source.

The settings API and attached scheduler share one `AutomationService` operation
lock. A disable update waits for an earlier automatic runtime dispatch to
finish, so once the update returns no earlier scheduler tick can still hand a
new prompt to the runtime. A prompt already handed to the runtime cannot be
retracted.

Scheduled automation enforcement is based on
`AutomationRunTrigger::Scheduled`. The setting is checked before listing or
claiming due work, atomically when a scheduled run is claimed, during pending
run recovery, and immediately before dispatch. Manual runs are not subject to
this setting. Each scheduler tick also reconciles pending automation runs from
their durable dispatch acknowledgements, covering a process that delivered a
command but failed before updating the run row.

Watermarks are isolated by automatic behavior and target. They survive reopen
and setting toggles. A watermark later than the current wall clock is reclaimed
once at the current time so clock rollback or restored data cannot suppress the
target indefinitely.

The API is:

- `GET /api/v1/token-spend-settings`
- `PUT /api/v1/token-spend-settings`

The UI groups these switches under **Automatic token use** and keeps manual
actions visibly separate from automatic scheduling.

## Consequences

- New and migrated stores cannot spend tokens automatically until a user
  explicitly enables a behavior.
- Enabling one layer does not implicitly enable either other layer.
- Settings are enforceable for scheduler and direct HTTP paths without relying
  on hidden controls or browser storage.
- Policy remains above `RuntimeIntervention`; the schema contains no Herdr- or
  provider-specific identity.
- A claimed request is rate-limited even if runtime delivery fails. Operators
  retain manual prompts and routes for immediate recovery.

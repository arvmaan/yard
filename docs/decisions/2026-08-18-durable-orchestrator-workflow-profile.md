# ADR: Durable Orchestrator Workflow Profile

- Status: Proposed
- Date: 2026-08-18

## Context

The central Yard orchestrator previously received a short coordination
objective plus the worker profile's optional instruction reference. The
standard Herdr orchestration procedure lived outside Yard in a local skill
file, so a newly provisioned orchestrator did not receive complete,
relocatable instructions for worker fanout, monitoring, intervention,
artifact collection, reconciliation, verification, and worker cleanup.

Browser-local settings cannot enforce server prompt behavior. `WorkerProfile`
also has a different responsibility: it selects provider launch and worker
capability policy. Reusing it would couple the central workflow to a provider
profile and make workflow edits change unrelated workers.

The workflow includes a ten-minute monitoring cadence, but Yard's automatic
token-spending settings are independent and default off. A descriptive cadence
must not enable the scheduler or bypass its backend command-source checks.

## Decision

Add a singleton `OrchestratorWorkflowProfile` with an append-only revision
table and a singleton current pointer. Revision 1 is seeded from factory
Markdown embedded in the Yard binary, with `monitor_interval_ms = 600000`.
Yard does not read a developer-home skill path at runtime.

PUT appends a user revision after optimistic version validation. Reset appends
a new revision containing the embedded factory values; it never moves the
pointer backward or mutates revision 1. The API is:

- `GET /api/v1/orchestrator-workflow-profile`
- `PUT /api/v1/orchestrator-workflow-profile`
- `POST /api/v1/orchestrator-workflow-profile/reset`

The central Yard orchestrator pins one workflow revision when it is first
configured or replaced. Fresh provisioning composes the active revision before
the central objective and status contract, then persists the same revision pin
with ownership. Recovery and configure-time adoption/replacement compose the
pin into their lifecycle prompts. Direct Yard-orchestrator prompts resolve the
immutable pin server-side; caller text remains unchanged in the durable prompt
command row. The status protocol remains the final prompt section.

An edit or reset changes the active revision for the next central
adoption/replacement. It does not silently change the behavior of an already
running central session.

`monitor_interval_ms` is inert configuration in this change. It creates no
timer, wakeup, automatic command, scheduler registration, or token-spend
setting mutation. Existing automatic sources remain independently default-off
and backend-enforced.

## Factory Contract

The factory Markdown requires the central orchestrator to:

- decompose every request into bounded independent Yard/Herdr worker lanes;
- capture once the workspace ID that owns the central orchestrator pane without
  inferring it from repository identity, focus, or a workspace naming pattern;
- create every worker as a separate tab in that captured workspace, using an
  isolated git worktree for write lanes and a plain tab for read-only lanes;
- provide each worker complete zero-context briefs and explicit exclusions;
- inspect the fleet every ten minutes while actively running;
- send concrete push-forward prompts for stalls and unresolved decisions;
- collect named artifacts and committed write-lane SHAs;
- require separate explicit grants for automatic token spend and
  permission-bypass settings;
- review all lane output, choose an integration order, and integrate accepted
  commits;
- run final tests, formatting, linting, and review; and
- close completed worker lifecycles without incidental branch, worktree, or
  transcript deletion.

## Scope

This slice covers the central Yard orchestrator completely:

- bootstrap and active-revision pinning in
  `crates/yard-server/src/yard_orchestrator_service.rs`;
- configure-time adoption/replacement and recovery lifecycle prompts in the
  same service; and
- direct central prompts in
  `crates/yard-server/src/intervention_service.rs`.

The profile is not yet composed into project or workstream orchestrators.
Exact remaining paths are:

- project creation in `crates/yard-server/src/project_service.rs`;
- project-orchestrator replacement in
  `crates/yard-server/src/orchestrator_replacement_service.rs`;
- handoff into an orchestrator role in
  `crates/yard-server/src/allocation_service.rs`;
- direct project prompts and Yard-to-project routes in
  `crates/yard-server/src/intervention_service.rs`; and
- workstream routes and snapshots in
  `crates/yard-server/src/coordination_node_service.rs`.

Those paths need a separate product decision about whether they inherit this
central workflow, receive a project-specific workflow profile, or remain
unchanged.

## Consequences

- Standard central behavior is durable, editable, resettable, and independent
  of localStorage and developer-home files.
- Immutable pins keep a running central orchestrator deterministic across
  profile edits.
- Factory resets preserve audit history.
- The ten-minute cadence is visible to the agent but does not claim unsupported
  wakeups.
- Extending the model to project/workstream orchestrators remains explicit
  follow-up work rather than an accidental behavior change.

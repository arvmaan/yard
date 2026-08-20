# Portable Profiles and Orchestrator Replacement Research

- Date: 2026-08-18
- Scope: WorkerProfile portability, runtime adapters, public agent
  configuration conventions, embedded Herdr orchestration, and explicit
  orchestrator replacement
- Source access date: 2026-08-18 unless an update date is stated

## Executive Decision

Adopt a versioned Yard `AgentProfile` bundle with a small portable semantic
core, versioned namespaced adapter extensions, explicit capability
negotiation, relative hashed artifacts, and strict secret exclusion. Preserve
the existing immutable Yard profile revision separately from the format
version.

Embed a relocatable `herdr-orchestration` Agent Skill and `herdr-cli`
companion in Yard-created project orchestrators. Materialize it relative to
the owned project workspace and discover runtime tools dynamically. Capture
the workspace that owns the orchestrator pane once, create isolated write
checkouts with Git, and attach every lane as a tab in that captured workspace
before starting or allocating its agent. Default to a dry run and require
non-persisted token-spend approval before model use. Require a separate
explicit grant only when permission-bypass settings are requested.

Do not add replacement UI in this lane. The project handoff path has an
atomic durable cutover, but displaced-orchestrator cleanup loses the captured
provider identity and can be marked successful without closing the old
runtime. Superintendent reassignment lacks service-level fresh identity
validation and cleanup entirely. Neither is the complete safe backend
contract required for an explicit destructive replacement control.

## Current WorkerProfile

### Domain, Store, API, and UI

| Area | Current behavior | Evidence |
|---|---|---|
| Domain | Flat fields combine portable intent with runtime/provider details. | `crates/yard-domain/src/profile.rs:9` |
| Revision | `WorkerProfile.version` is an immutable Yard revision and optimistic concurrency value. | `crates/yard-domain/src/profile.rs:60`; `crates/yard-store/migrations/0002_profiles_assignments.sql:9` |
| Validation | Trims and bounds strings/lists, but does not validate cross-field adapter support. | `crates/yard-domain/src/profile.rs:25` |
| Persistence | Scalar and ordered-list revisions are immutable; current version advances transactionally. | `crates/yard-store/src/lib.rs:2163`; `crates/yard-store/src/lib.rs:7478` |
| Pinning | Workers and assignments retain exact `(profile_id, profile_version)` references. | `crates/yard-store/migrations/0002_profiles_assignments.sql:60`; `crates/yard-store/migrations/0002_profiles_assignments.sql:253` |
| REST | CRUD exposes the current flat revision through `/api/v1/worker-profiles`. | `crates/yard-server/src/http/mod.rs:331`; `web/src/api.ts:529` |
| UI | Editor exposes provider, model, instruction reference, free-form capability lists, and a few fixed policies. | `web/src/ProfileEditor.tsx:109` |
| Defaults | Templates are Herdr/Codex/project-workspace/manual-receipt shaped. | `web/src/workerProfileTemplates.ts:9` |

The profile lists suggest capability injection that does not exist.
Provisioning rejects any non-empty tool, skill, or MCP list at
`crates/yard-server/src/allocation_service.rs:934`. Supported policy values
are limited to Herdr, `project_workspace`, `runtime_default` sandbox,
`runtime_default|yolo` permissions, and `manual_receipt` completion at
`crates/yard-server/src/allocation_service.rs:906`.

Model and full-access argument lowering is provider-specific and implemented
only for Codex and Claude
(`crates/yard-server/src/allocation_service.rs:946`). The larger accepted
Herdr agent-kind list includes Kiro and Hermes but does not imply equivalent
model or permission support (`crates/yard-server/src/allocation_service.rs:1065`).
`instructions_ref` is not loaded; it becomes a prompt sentence
(`crates/yard-server/src/allocation_service.rs:995`).

The launch flow is:

1. Profile CRUD persists an immutable revision.
2. Allocation/project/orchestrator commands pin that exact revision.
3. `validate_supported_profile` rejects unsupported combinations.
4. `provider_args` lowers the recognized provider subset.
5. The service builds assignment text and calls `RuntimeControl`.
6. `HerdrInventorySource` forwards opaque kind/args to the Herdr adapter
   (`crates/yard-server/src/inventory_service.rs:157`).
7. Herdr issues `agent.start` and later prompts the prepared pane
   (`crates/yard-herdr/src/control.rs:302`).

### Portability Implications

- Keep profile identity and immutable revision pinning.
- Add a separate format discriminator and version.
- Move provider argument generation behind adapter compilers.
- Make required/optional capabilities and degradation explicit.
- Treat instructions, skills, plugins, tools, MCP, hooks, subagents, and
  administrative policy as different component classes.
- Preserve unknown native artifacts and extensions for round trips.
- Validate launch compatibility before saving or selecting a profile, while
  still permitting storage of an opaque profile for a different installed
  adapter.

## Public Provider Research

No provider offers a complete interchange format. The common floor is
instruction files, the Agent Skills directory convention, and MCP. Even those
features have different discovery and precedence. The Agent Plugins
specification is useful packaging prior art but remains a Working Draft and
deliberately excludes several lifecycle and policy classes.

### Claude Code

Claude Code composes `CLAUDE.md` and `.claude/rules/*.md` instructions, has
scope-specific settings, supports Agent Skills, plugin manifests, Markdown
subagents, hooks, MCP, and a provider-specific permission model. Settings
precedence and instruction discovery are separate mechanisms, so Yard must
compile both rather than flattening them.

Primary documentation:

- [Settings](https://code.claude.com/docs/en/settings)
- [Memory and CLAUDE.md](https://code.claude.com/docs/en/memory)
- [Agent Skills](https://code.claude.com/docs/en/skills)
- [Plugins](https://code.claude.com/docs/en/plugins)
- [Subagents](https://code.claude.com/docs/en/sub-agents)
- [Hooks](https://code.claude.com/docs/en/hooks)
- [MCP](https://code.claude.com/docs/en/mcp)
- [Permissions](https://code.claude.com/docs/en/permissions)

Public page publication/update dates were not exposed. All pages were
retrieved on 2026-08-18.

### OpenAI Codex

Codex natively discovers hierarchical `AGENTS.md` and
`AGENTS.override.md`, supports project/user/system configuration layers,
Agent Skills and plugins, MCP, hooks, and TOML multi-agent roles.
Administrative requirements are stronger than prompt instructions and must
remain enforcement policy in Yard.

Primary documentation:

- [AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
- [Basic configuration](https://learn.chatgpt.com/docs/config-file/config-basic)
- [Configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- [Skills and plugins](https://learn.chatgpt.com/docs/skills-and-plugins)
- [Build skills](https://developers.openai.com/plugins/build/skills)
- [Build plugins](https://developers.openai.com/plugins/build/plugins)
- [Plugin concepts](https://developers.openai.com/plugins/concepts/plugins)
- [App server](https://learn.chatgpt.com/docs/app-server)
- [Changelog](https://learn.chatgpt.com/docs/changelog)

Public reference pages did not expose reliable update dates. The retrieved
changelog dates Agent Skills to 2025-12-19, plugins to 2026-03-25, and CLI
import/multi-agent V2 to 2026-07-21.

### Kiro

Kiro has global, project, and custom-agent scopes. Precedence differs by
feature: same-name project skills/agents override global definitions, MCP
agent scope overrides project/global, steering and hooks merge, and
permission denies override grants. Kiro also distinguishes Agent Skills,
Powers, custom agents, hooks, MCP, and permissions. A Yard adapter must report
the target Kiro surface because web/mobile support can be a subset of CLI/IDE
support.

Primary documentation and displayed update dates:

- [Configuration scopes](https://kiro.dev/docs/configuration/), updated
  2026-08-12
- [Steering](https://kiro.dev/docs/steering/), updated 2026-08-12
- [Agent Skills](https://kiro.dev/docs/skills/), updated 2026-08-04
- [Subagents](https://kiro.dev/docs/custom-agents/subagents/), updated
  2026-08-04
- [MCP configuration](https://kiro.dev/docs/mcp/configuration/), updated
  2026-08-04
- [Powers](https://kiro.dev/docs/powers/), updated 2026-08-06
- [Agent configuration reference](https://kiro.dev/docs/custom-agents/configuration-reference/),
  updated 2026-08-06
- [Hooks](https://kiro.dev/docs/hooks/), updated 2026-08-06
- [Permissions](https://kiro.dev/docs/permissions/), updated 2026-08-14

### Hermes

Hermes uses provider-specific `config.yaml`, managed pins, context-file type
selection and hierarchy, skills, plugins, hooks, MCP, and `delegate_task`.
`HERMES_HOME` makes provider state relocatable, but that is not itself a
portable Yard package. The dedicated context-file documentation is treated as
authoritative where it is newer than the general configuration page.

Primary documentation:

- [Configuration](https://hermes-agent.nousresearch.com/docs/user-guide/configuration)
- [Context Files](https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files)
- [Skills](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills)
- [Plugins](https://hermes-agent.nousresearch.com/docs/user-guide/features/plugins)
- [Subagent Delegation](https://hermes-agent.nousresearch.com/docs/user-guide/features/delegation)
- [Event Hooks](https://hermes-agent.nousresearch.com/docs/user-guide/features/hooks)
- [MCP](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp)
- [Security](https://hermes-agent.nousresearch.com/docs/user-guide/security)
- [Public Subagent Lifecycle API](https://hermes-agent.nousresearch.com/docs/developer-guide/subagent-lifecycle-api)

The website did not expose page update metadata. Corresponding public source
files were last committed on 2026-08-17, with the Skills source updated on
2026-08-18 UTC.

### Cross-Provider Conventions

- [Agent Skills Specification](https://agentskills.io/specification): requires
  `SKILL.md` with `name` and `description` frontmatter; latest source change
  observed 2026-08-04. It does not define a top-level package version, so Yard
  must version the containing bundle.
- [AGENTS.md convention](https://agents.md/): schema-free Markdown with root
  and nested scopes; page update date unavailable. The
  [source repository](https://github.com/agentsmd/agents.md) was created
  2025-08-19.
- [Agent Plugins Specification](https://agent-plugins.org/specification):
  version 1.0.0, status Working Draft, published in source 2026-07-24 and
  repository updated 2026-08-06. It defines contained relative resources and
  namespaced extensions, but v1 excludes portable hooks, subagents, policies,
  and OAuth credential configuration.

## Embedded Herdr Orchestrator Kit

The current `herdr-orchestration` skill has the correct baseline operating
shape: classify before creating durable lanes, capture the orchestrator pane's
workspace once, use plain tabs there for read-only research, attach isolated
write-lane worktrees as tabs there, prompt with bounded deliverables, poll
rather than treating timeout as failure, and make cleanup explicit. Evidence
is in:

- `/local/home/arvinmaa/.codex/skills/herdr-orchestration/SKILL.md:13`
- `/local/home/arvinmaa/.codex/skills/herdr-orchestration/SKILL.md:35`
- `/local/home/arvinmaa/.codex/skills/herdr-orchestration/SKILL.md:88`
- `/local/home/arvinmaa/.codex/skills/herdr-orchestration/SKILL.md:117`
- `/local/home/arvinmaa/.codex/skills/herdr-orchestration/SKILL.md:158`
- `/local/home/arvinmaa/.codex/skills/herdr-cli/SKILL.md:8`
- `/local/home/arvinmaa/.codex/skills/herdr-cli/SKILL.md:46`
- `/local/home/arvinmaa/.codex/skills/herdr-cli/SKILL.md:104`

Those absolute paths are research evidence only. They must never appear in the
installed kit.

### Packaging Contract

1. Embed a versioned, hashed kit in Yard's distributable.
2. Before starting a Yard-created project orchestrator, materialize it
   atomically under a project-relative Yard-managed directory.
3. Include both `herdr-orchestration` and `herdr-cli`; the former depends on
   the latter for exact CLI semantics.
4. Let each provider adapter expose the same canonical skill through that
   provider's supported project skill/plugin discovery mechanism. Do not
   assume one discovery directory is universal.
5. Use relative references inside the kit. Resolve the repository at runtime
   with `git rev-parse --show-toplevel`, resolve executables with
   `command -v`, and inspect `herdr ... --help` for supported flags.
6. Put transient manifests under `$YARD_RUN_DIR` or an XDG-derived path and
   record workspace, worktree, terminal, pane, tab, provider session, process,
   agent, branch, checkout, and isolated worktree CWD identities.
7. Default to a dry-run plan. Capture once the workspace ID that owns the
   orchestrator pane, use plain tabs there for declared read-only lanes, and
   attach isolated git worktrees there as tabs for declared write lanes. Never
   infer the ID from project or repository identity, current focus, or a
   workspace naming convention.
8. Require explicit, non-persisted `--approve-agent-spend` plus maximum agent
   and depth bounds before any `herdr agent start` or prompt that incurs model
   usage. Require a separate explicit grant for permission-bypass settings;
   never infer one from spend approval or runtime settings.
9. Require structured lane output with conclusions, evidence, files changed,
   checks, risks, and next action.
10. Keep close/remove operations separate. Never infer completion from agent
    status and never automatically delete a transcript, branch, or worktree.

### Captured Parent-Workspace Contract

A 2026-08-18 live run exposed a required boundary that the initial packaging
contract did not state. The old `herdr worktree create` flow produced
top-level workspaces observed as `w1V` through `w1Z`, while the orchestrator's
workspace was observed as `wN`. Those IDs are historical observations, not a
discovery convention. Moving the resulting panes recovered that run, but
post-creation movement is not the normal provisioning contract.

Accepted ADR-0008 rejects live cross-workspace allocation. The supported
sequence prevents that topology:

1. Capture the orchestrator pane once before delegation and retain its exact
   workspace ID for the fleet.
2. Resolve the explicit repository root. For each write lane, create an
   isolated checkout with `git worktree add`; read-only lanes retain the source
   checkout.
3. Create each lane with `herdr tab create`, passing `--workspace <captured>`
   and `--cwd <checkout>`. Herdr 0.8.0 treats the `worktree create` selectors
   `--workspace` and `--cwd` as alternatives, so never combine them.
4. Verify the returned tab and pane carry the captured workspace ID, then
   start the agent and capture terminal, provider session, process, checkout,
   pane, and tab identity.
5. Fetch fresh Herdr and Yard inventory. Require exactly one matching runtime
   in the captured workspace and let Yard reconcile it as observed and
   unassigned.
6. Invoke normal Yard confirmed allocation only after workspace verification
   and reconciliation. Allocation creates the active Yard assignment.

The wrapper must fail closed:

- If worktree, tab, or agent creation fails, do not allocate.
- If the tab reports a different workspace, do not allocate or blindly move
  it. Refresh both inventories and retain the resources for inspection.
- If no unique runtime matches the captured terminal/provider identity,
  process, and CWD in the captured workspace, stop.
- If Yard reconciliation does not converge, do not allocate, recreate, or
  blindly move the pane. Preserve the live runtime for inspection.
- If confirmed allocation fails after workspace verification and reconciliation,
  keep the worker observed and unassigned in the target workspace. Do not
  recreate it; reload current versions and retry under the allocation
  command's idempotency semantics.
- Cleanup remains a separate explicit operation. Before any destructive close
  or worktree removal, freshly revalidate the captured runtime identity,
  project-owned workspace, and owned topology. Never remove a worktree,
  branch, or transcript merely because allocation failed or runtime status
  appears terminal.

The current project creation path only passes an instruction reference into a
prompt and starts the selected kind. It has no kit materialization step
(`crates/yard-server/src/project_service.rs:234`). Therefore packaging is
required product work, not a profile-template change.

## Orchestrator Lifecycle Audit

### Existing Project-Orchestrator Handoff

The cross-project handoff API is
`POST /api/v1/projects/{project}/assignments/{assignment}/handoffs`
(`crates/yard-server/src/http/mod.rs:307`). The service:

1. reserves source and target state;
2. prepares a replacement runtime;
3. durably claims its topology;
4. starts it;
5. freshly verifies target workspace, topology, and provider identity;
6. finalizes the role swap in one SQLite immediate transaction;
7. processes durable old-runtime cleanup after commit.

Evidence:

- prepare/claim/start/verify:
  `crates/yard-server/src/allocation_service.rs:651`,
  `crates/yard-server/src/allocation_service.rs:682`,
  `crates/yard-server/src/allocation_service.rs:693`,
  `crates/yard-server/src/allocation_service.rs:829`
- transactional revalidation and swap:
  `crates/yard-store/src/lib.rs:3492`,
  `crates/yard-store/src/lib.rs:3524`,
  `crates/yard-store/src/lib.rs:3545`,
  `crates/yard-store/src/lib.rs:3758`
- post-commit cleanup:
  `crates/yard-server/src/allocation_service.rs:717`,
  `crates/yard-server/src/runtime_cleanup_service.rs:30`

This is a safe durable cutover for its existing semantics, but it is not the
requested general replacement primitive:

- it requires an active non-orchestrator assignment in another project;
- it changes that source assignment to `handed_off`;
- it cannot create a fresh orchestrator directly from a profile in the same
  project;
- it offers no old-session disposition;
- it has a displaced-orchestrator cleanup identity gap.

The gap is concrete. Finalization inserts a retired binding only for the
handoff source (`crates/yard-store/src/lib.rs:3556`) and queues the displaced
target orchestrator separately (`crates/yard-store/src/lib.rs:3635`).
Pending cleanup recovers provider identity by joining on both command and
worker (`crates/yard-store/src/lib.rs:4158`). No retired row exists for the
displaced target. Cleanup therefore has no captured provider identity.

Runtime retirement itself remains fail-closed for destruction. Fresh
inventory must match provider identity, or exact topology when no identity
was captured, and workspace equality; tab closure is allowed only when the
tab still contains one pane
(`crates/yard-server/src/inventory_service.rs:283`). A mismatch issues no
destructive Herdr call. However, normal queued cleanup treats a mismatch as
converged (`crates/yard-server/src/runtime_cleanup_service.rs:120`), so the
old orchestrator can leak while the job becomes successful. Exposing this as
the general replacement control would hide an unresolved lifecycle defect.

### Existing Superintendent Lifecycle

`YardOrchestratorService::recover` restarts/reconciles the dedicated session
while retaining the same worker and provider identity
(`crates/yard-server/src/yard_orchestrator_service.rs:163`). This is correctly
distinct from replacement.

Provision validates a profile and reconciled dedicated worker
(`crates/yard-server/src/yard_orchestrator_service.rs:64`). If already
dedicated, it adopts the existing matching runtime rather than creating a
fresh context.

The raw `PUT /api/v1/yard/orchestrator` reassigns ownership in the store
(`crates/yard-server/src/http/mod.rs:658`,
`crates/yard-store/src/lib.rs:1015`). It checks versions and candidate
availability, but it does not freshly validate the runtime through the
service, does not capture the displaced runtime for cleanup, and does not
offer a fresh dedicated worker. It must not back a "Replace orchestrator"
button.

### Status Is Not Completion

Replacement is user intent, never a deduction from `idle`, `done`, process
exit, or a status report. Yard's ownership rule explicitly forbids inferring
completion from runtime status (`CONTRIBUTING.md:5`). Completion requires a
manual evidence-backed receipt (`crates/yard-domain/src/assignment.rs:174`),
and a regression test confirms process exit does not complete an assignment
(`crates/yard-store/src/lib.rs:12106`).

## Required Replacement Contract

Implement one service-level saga per target kind, sharing a transaction model
but not pretending their runtime topology is identical.

### Command

`ReplaceProjectOrchestrator`:

- `command_id`, `actor`, `project_id`;
- `expected_project_version`;
- `expected_orchestrator_worker_id` and worker version;
- complete expected old runtime binding version and captured identity;
- selected immutable `profile_id` and `expected_profile_version`;
- new objective and role;
- `old_session_disposition`;
- optional human-authored handoff/context artifact reference.

`ReplaceYardOrchestrator` additionally carries
`expected_orchestrator_version` and targets the dedicated central scope.

The command contains no "completed" flag and accepts no runtime status as
completion evidence.

### Durable Saga

1. Normalize and insert an idempotent command under a SQLite immediate
   transaction.
2. Re-read the current target, expected versions, active interventions, and
   exact old runtime binding. Reject stale state.
3. Persist a full immutable old-runtime snapshot, including adapter, session,
   workspace, terminal, tab, pane, provider identity, and ownership. The
   schema must permit both source and displaced-target snapshots for one
   command.
4. Reserve exactly one unfinished replacement per target. A replay returns
   the committed result; pending/ambiguous commands do not repeat external
   mutations.
5. Resolve the target's Yard-owned workspace from durable state and fresh
   inventory. Reject an adapter/session/workspace mismatch or an unowned
   workspace.
6. Prepare a unique replacement runtime without changing ownership. For the
   Superintendent, use a unique temporary agent name so the old reserved name
   does not require destructive pre-cutover retirement.
7. Persist the prepared binding before start. Start, deliver the objective,
   and freshly verify interactive readiness, provider identity, workspace,
   terminal, tab, and pane.
8. In one SQLite immediate transaction, revalidate old ownership and both
   runtime snapshots, create/pin the replacement worker/assignment, switch
   orchestrator ownership, record the old assignment as superseded/handed off
   without a completion receipt, revoke old terminal leases, and enqueue the
   selected disposition.
9. Only after commit process a retirement disposition. Before any close,
   fetch fresh inventory and require exact captured provider identity (or the
   complete exact legacy topology), exact workspace, and current ownership.
   If validation fails, issue no close and leave cleanup pending with an
   actionable identity-mismatch error.
10. Cleanup failure never rolls back the committed ownership swap. It remains
    durable and retryable.

### Old-Session Dispositions

- `retain_for_inspection` (default): detach orchestration ownership, preserve
  the old Yard worker/runtime and transcript, and label it superseded. Do not
  mark work complete.
- `retire_after_cutover`: enqueue identity-checked pane/tab retirement after
  commit while retaining the durable worker, transcript metadata, and
  lifecycle history.
- `end_after_cutover`: same identity-safe runtime retirement plus explicit
  ended worker disposition. This still does not create a completion receipt.

No disposition deletes a branch, worktree, artifact, or transcript.

### Confirmation UX

Place "Replace orchestrator" in the project-orchestrator and Superintendent
inspectors, separate from "Restart orchestrator session." It is available
because the user chose replacement, not because status appears exhausted,
idle, done, or offline.

The confirmation dialog shows:

- current worker and provider-session identity;
- target project or Superintendent scope;
- replacement profile and exact revision;
- editable new objective;
- old-session disposition, defaulting to retain for inspection;
- warning when an intervention is pending or the runtime is working;
- explicit text that replacement does not mark the old task complete;
- a final confirmation naming the old worker that will lose ownership.

After submission, show separate states for preparing, verifying, committed
with cleanup pending, and complete. Do not report replacement as failed after
cutover solely because cleanup is pending.

### Required Tests

Domain and API:

- normalize all optimistic versions and dispositions;
- idempotent replay and command-payload conflict;
- stale project, worker, profile, orchestrator, and runtime versions;
- pending prompt/route conflict;
- no status field can trigger or complete replacement.

Service and adapter:

- prepare and claim occur before start;
- replacement workspace must equal the Yard-owned target workspace;
- provider identity/topology mismatch blocks cutover;
- prompt failure and transport ambiguity retain recoverable command state;
- old runtime is never retired before committed cutover;
- retirement revalidates captured identity and workspace immediately before
  close;
- reused terminal/provider identity never closes;
- shared tab degrades to pane-only close;
- cleanup failure retries after restart;
- Superintendent replacement uses a unique temporary runtime name.

Store:

- one transaction swaps ownership, pins profile revision, supersedes the old
  assignment without a receipt, captures both runtime identities, revokes
  leases, and enqueues disposition;
- injected failures roll the whole transaction back;
- retain disposition queues no destructive job;
- multiple retired bindings per command remain independently addressable;
- migration preserves existing handoff and cleanup history.

UI:

- explicit replacement is reachable from both inspectors;
- restart and replacement have distinct labels and commands;
- dialog defaults to retain and says no completion is inferred;
- submitted payload includes displayed IDs and versions;
- working/done/idle status never auto-submits or changes disposition;
- committed-with-cleanup-pending remains a success state;
- narrow viewport and keyboard focus behavior.

## Production Decision for This Lane

No production files are changed. The existing project handoff remains
available through its current worker-move UX, and Superintendent recovery
remains a same-identity restart. Wiring either existing low-level operation as
the requested general replacement control would overstate its safety and
semantics.

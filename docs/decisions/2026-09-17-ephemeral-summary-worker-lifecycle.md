# ADR: Ephemeral Summary-Worker Lifecycle

- Status: Proposed
- Date: 2026-09-17

## Context

An orchestrator needs a bounded worker that can produce a summary, hand the
result back durably, and then leave the active runtime fleet. Yard already has
durable `Worker`, `Assignment`, completion-receipt, `Artifact`, reconciliation,
runtime-cleanup, and lifecycle-event contracts. This proposal extends those
contracts; it does not create a second task or runtime framework.

The inspected control surface does not expose a native worker fork API.
Herdr 0.9.0 provides `workspace.create`, `tab.create`, `pane.split`,
`agent.start`, observation commands, and topology close commands. Its bundled
request schema contains 102 unique request methods and no method named fork,
clone, child, or spawn. Yard's Herdr adapter likewise creates workspaces or
tabs, starts an agent in an existing pane, prompts it, and observes it.
Provider child-agent data is observation metadata, not a control API.

Relevant evidence:

- `herdr --version`, `herdr workspace --help`, `herdr tab --help`,
  `herdr pane --help`, and `herdr agent --help` report the Herdr 0.9.0 command
  surface described above.
- `herdr api schema --json` and its 102-method request inventory
  contain `workspace.create`, `tab.create`, `agent.start`,
  `pane.process_info`, `pane.close`, and `tab.close`, but no native fork or
  provider child-spawn operation.
- `crates/yard-herdr/src/control.rs:231-340` implements workspace and tab
  creation; `crates/yard-herdr/src/control.rs:411-438` starts an agent in an
  existing pane.
- `crates/yard-server/src/allocation_service.rs:108-197` defines the
  provider-neutral runtime-control boundary. It supports provisioning,
  prepare/start, restart, bootstrap, and retirement, not fork.
- `crates/yard-domain/src/assignment.rs:39-172` defines assignment terminal
  states, structured completion receipts, and artifact references.
- `crates/yard-domain/src/artifact.rs:27-113` preserves artifact provenance
  across project, assignment, attempt, worker, digest, actor, and creation time.
- `crates/yard-server/src/runtime_cleanup_service.rs:103-203` already provides
  durable claimed cleanup, retry, and success/failure persistence.
- `crates/yard-server/src/inventory_service.rs:502-527` refuses destructive
  retirement when the installed Herdr protocol cannot atomically guard runtime
  identity.
- `crates/yard-server/src/terminal_service.rs:120-173` binds an in-memory
  interactive terminal lease to a target and captured runtime identity. It is
  not a durable cleanup precondition or authoritative no-lease query.

## Decision

Model an ephemeral summary worker as an ordinary Yard `Worker` plus
`Assignment`, with explicit lifecycle policy and parentage. It is not a
provider-native child agent.

### Durable identity and policy

The durable worker record gains:

- an explicit `ephemeral` policy marker, defaulting to false; and
- a required `parent_worker_id` when `ephemeral` is true.

Only an explicit creation command may set `ephemeral = true`. Staleness,
absence, failed observation, disconnection, ambiguity, process exit, or age
must never imply ephemeral policy.

The parent must be the orchestrator that authorized the child. The child has a
normal allocation and one bounded summary `Assignment`, so existing project,
worker, assignment, attempt, artifact, and command provenance remains
authoritative.

### Same captured Herdr workspace

Allocation captures the parent orchestrator's durable Herdr workspace identity
before creating the child. It creates a non-focused tab with
`tab.create --workspace <captured-parent-workspace>` and starts the selected
provider in that tab's root pane with `agent.start`.

The implementation must verify the returned workspace identity before
persisting allocation. It must not derive placement from repository name,
current UI focus, a later observation, or a provider session. Worktree
isolation remains an allocation choice, but the resulting checkout is attached
as a tab in the same captured workspace.

Parentage is Yard correlation metadata. A future provider-native child-spawn
path is allowed only behind an explicitly advertised adapter capability with
equivalent identity, placement, observation, handoff, and retirement
semantics. Observation of provider child metadata must not enable that path.

### Completion handoff precedes retirement

The child must persist its summary `Artifact` through the existing artifact
API before submitting completion. The completion command then records a
structured receipt that references the required summary Artifact ID and the
same project, assignment, attempt, and worker provenance.

The completion transaction must:

1. verify that the summary Artifact already exists durably and belongs to the
   current assignment and attempt;
2. persist the completion receipt and its Artifact link;
3. transition the assignment and attempt to terminal state; and
4. commit before the worker can become cleanup-eligible.

A cleanup transaction must read the committed receipt and linked Artifact
again. Runtime output, terminal bytes, agent narration, or an in-memory
completion signal is never sufficient handoff evidence.

### Required approval, lease, and observation authorities

Phase 1 introduces three durable authorities before positive cleanup
eligibility exists.

First, a durable `AssignmentApprovalState` record is authoritative for operator
decisions. It is created with each ephemeral Assignment attempt and versioned.
Its current gate state is `Clear`, `Pending`, `Blocked`, or `Ambiguous`;
decision history retains approved, rejected, and cancelled outcomes. Cleanup
may proceed only when a transactional read of the exact version returns
`Clear`. A missing record, incomplete state, stale version, or unavailable
approval store is unknown and fails closed. Terminal output and runtime status
are not approval authority.

Second, every interactive terminal session registers a durable lease before it
can open and durably releases or revokes that lease when authority ends.
Lease acquisition and retirement share one atomic fence. Acquisition succeeds
only while retirement is not fenced; retirement closes the acquisition fence
and verifies zero active durable registrations in the same transaction. There
is no separate best-effort or in-memory "no active lease" query. Restart,
disconnect, or process exit does not erase a durable lease or imply release.

Third, every successful reconciliation persists an observation-generation
token that includes the adapter/session epoch and monotonic inventory
generation. Cleanup captures the generation that observed the exact runtime
identity and topology as `Observed` and `Exited`. Immediately before the
destructive step, it must revalidate that the current generation is the same
captured generation, the state remains `Observed` and `Exited`, and the
workspace, tab, pane, terminal, and provider identity are unchanged.

Until all three authorities exist and can answer affirmatively, positive
cleanup eligibility is impossible. The only valid result is a durable
refusal/defer event.

### Fail-closed cleanup eligibility

One shared eligibility decision is authoritative for automatic retirement.
Every condition must be true in the same guarded decision:

1. the Worker is explicitly marked ephemeral;
2. its current Assignment and attempt are terminal;
3. the authoritative approval-state record is present at the expected version
   and reads `Clear`;
4. the structured completion receipt is durably persisted;
5. the required summary Artifact is durably persisted and linked to that
   receipt with matching provenance;
6. a successful observation generation records the exact captured runtime as
   `Observed` and `Exited`;
7. the configurable grace TTL, measured from committed completion, has
   elapsed;
8. the atomic lease-acquisition/retirement fence closes and affirms zero active
   durable lease registrations; and
9. immediate revalidation affirms the same observation generation,
   `Observed` and `Exited` state, unchanged topology and identity, and unchanged
   worker, assignment, runtime-binding, approval, and lease-fence versions.

Unknown is not eligible. Missing observation is not stopped. A disconnected,
not-observed, missing, ambiguous, or exited record without a fresh affirmative
`Observed` and `Exited` generation is not eligible. Approval-state,
artifact-store, durable-lease, runtime-adapter, reconciliation, or database
unavailability defers cleanup.

Restart, adapter/session epoch change, observation-generation change, topology
change, identity change, missing identity guard, or revalidation failure also
defers cleanup. A previously successful observation cannot be reused after any
of those events.

### Retirement, tombstones, and audit

Retirement is not deletion. Eligibility creates one durable, idempotent
retirement command keyed to the Worker, Assignment, completion receipt, and
summary Artifact. The command advances the Worker into a retirement-pending
tombstone only through the atomic retirement fence. Repeated sweeps replay the
same command or retry the same cleanup job.

Physical runtime cleanup may close only the captured child tab or pane. It must
not close the parent workspace. The destructive adapter operation must
atomically compare the captured generation, state, topology, and identity while
closing the exact target. The retirement command records that identity-
conditional close and the tombstone transition as one idempotent operation.
If the runtime boundary cannot provide that compare-and-close guarantee, Yard
must not detach, hide, or tombstone the Worker; it records refusal and defers.

Success moves the tombstone to retired. Active projections may omit an
explicitly retired ephemeral Worker, but history APIs retain the Worker,
parent, allocation, Assignment, attempts, receipt, Artifact, runtime identity,
retirement command, cleanup attempts, and lifecycle events. Automatic cleanup
never deletes those records, their branch, worktree, transcript, or artifacts.

Durable audit events record:

- eligibility accepted, including approval-state version, lease-fence version,
  observation epoch/generation, evaluated aggregate versions, identity, and
  grace deadline;
- eligibility refused or deferred, with stable reason codes;
- retirement attempted;
- retry scheduled or refused by the adapter; and
- retirement succeeded.

### Protection for ordinary workers

Workers without the explicit ephemeral marker are outside this cleanup
candidate set. Yard must never automatically delete, retire, replace, archive,
or hide an ordinary Worker merely because it is stale, old, exited,
disconnected, missing, not observed, ambiguous, or absent from an adapter
inventory.

This invariant applies before any runtime or assignment heuristic. Candidate
queries begin with explicit ephemeral policy; observation state can only
remove an ephemeral candidate from eligibility, never add a Worker.

## Smallest vertical slices

### Phase 1: policy, parentage, and guarded cleanup

Persist the explicit ephemeral marker and parent Worker ID, default all
existing Workers to ordinary, and add the authoritative `AssignmentApprovalState`
resource, durable terminal-lease registration, atomic acquisition/retirement
fence, persisted observation epoch/generation, same-generation revalidation,
identity-conditional close contract, retirement command, tombstone, audit
events, grace TTL, and durable retry integration.

Do not expose allocation or spawning yet. Cleanup remains hard-disabled for a
positive result until approval authority, durable lease fencing, observation
generation, and atomic identity-conditional close are all present. Tests may
create ephemeral fixtures directly to prove every refusal path and may use a
fully capable fake adapter to prove the positive path.

### Phase 2: completion handoff contract

Require one persisted summary Artifact and add its ID to the structured
completion receipt for ephemeral assignments. Enforce Artifact-first,
receipt-second ordering and provenance in the existing assignment-completion
transaction. Keep ordinary completion behavior unchanged.

### Phase 3: same-workspace allocation

Extend the existing allocation command to explicitly request an ephemeral
summary child and name its parent orchestrator. Validate the parent and capture
its workspace, then use the existing runtime-control and Herdr tab APIs to
create and start the child in that workspace. No native-fork abstraction is
added.

### Phase 4: reconciliation-driven retirement

Capture eligibility only after successful reconciliation, then immediately
revalidate the same observation generation and `Observed`/`Exited` identity
before the atomic identity-conditional close/tombstone operation. Feed the
resulting idempotent command to the existing runtime-cleanup service and expose
bounded operational status only if needed. Restart, generation or topology
change, missing guard support, and adapter or observation outage record
defer/retry audit events and perform no retirement.

No UI change is required for these slices unless operators cannot otherwise
inspect retirement-pending tombstones and refusal reasons.

## Test matrix

| Area | Required cases |
|---|---|
| Positive eligibility | Authoritative approval-state record at the expected version and `Clear`; durable lease registration with an atomically closed acquisition fence and zero active registrations; captured generation `G`; immediate revalidation of the same `G` as `Observed` and `Exited` with unchanged identity/topology; identity-conditional close support; explicit ephemeral child; correct parent; terminal assignment and attempt; persisted receipt and linked summary Artifact; TTL elapsed; one retirement command and complete audit sequence. |
| Impossible positive eligibility | Missing approval-state record, unavailable or unknown approval state, stale approval version, absent durable lease registry, non-atomic lease fence, absent observation generation, absent same-generation revalidation, or missing identity-conditional close capability each makes positive eligibility impossible and emits refusal/defer audit. |
| Explicit policy | Identical ordinary Worker is untouched. Ordinary stale, old, exited, disconnected, missing, not-observed, ambiguous, and inventory-absent Workers are each untouched. |
| Assignment gates | Allocating, active, handing-off, handed-off without the required receipt, stale attempt, and changed assignment version each refuse cleanup. |
| Approval gates | `Pending`, `Blocked`, `Ambiguous`, stale version, incomplete state, unknown state, missing authoritative record, and approval-store outage each defer cleanup. A current `Clear` gate with approved, rejected, or cancelled history permits only this gate. |
| Receipt and Artifact gates | Missing receipt, missing summary Artifact ID, missing Artifact, unlinked Artifact, wrong assignment/attempt/worker provenance, and Artifact-store outage each defer cleanup. |
| Ordering | A stopped child cannot retire before the Artifact commit; cannot retire after Artifact commit but before receipt commit; becomes eligible only after the committed receipt and other guards. |
| Observation generation | Capture generation `G`; same-`G` `Observed`/`Exited` revalidation permits this gate. Generation `G+1`, adapter/session epoch change, server or adapter restart, Running, Missing, Ambiguous, disconnected, stale observation, topology or provider-identity change, revalidation failure, and adapter outage each defer. |
| Identity-conditional retirement | Exact identity/generation compare-and-close succeeds once. Missing atomic guard, close target mismatch, changed tab/pane/terminal/provider identity, partial close result, and unavailable close receipt each refuse tombstone and remain retryable. |
| Grace TTL | Before deadline, exact boundary, after deadline, configurable values, clock rollback, and overflow-safe deadline calculation. |
| Durable leases | Opening a terminal first registers a durable lease. Active registration refuses cleanup; restart or disconnect does not erase it; durable release/revocation permits a later pass; concurrent acquisition versus retirement has exactly one atomic-fence winner; registry or fence failure makes positive eligibility impossible. |
| Placement | Child tab and root pane report the captured parent workspace; focus changes and similarly named workspaces do not alter placement; mismatch rolls allocation back. |
| Idempotency and retry | Repeated eligibility sweeps create one retirement command; repeated claims do not duplicate tombstones; adapter failure retries the same job; process restart preserves approval state, lease, generation, and command state; success replay is harmless. |
| Audit | Accepted, attempted, refused/deferred, retry, and succeeded events persist with Worker, Assignment, receipt, Artifact, command, approval-state version, lease-fence version, observation epoch/generation, identity/topology, aggregate versions, reason code, and time. |
| Provenance and visibility | Retirement preserves Worker/Assignment/Artifact/runtime history; only explicitly retired ephemeral Workers leave active projections; parent workspace and ordinary Workers remain visible and intact. |
| Provider neutrality | Herdr uses tab creation and agent start; an adapter without required capabilities refuses safely; provider child observation alone never selects a native-spawn path. |

## Consequences

- Summary-worker cleanup is explicit, durable, provider-neutral, auditable, and
  unable to race ahead of result handoff.
- The first implementation reuses Yard's existing domain, store, allocation,
  reconciliation, artifact, completion, and cleanup boundaries while adding
  the missing durable approval-state, lease, and observation-generation authorities.
- Positive cleanup eligibility is intentionally impossible until all Phase 1
  authorities and atomic fences exist.
- Runtime cleanup can remain pending indefinitely when observation or an
  identity-guarded close capability is unavailable; safety takes precedence
  over tidiness.
- Ordinary Worker lifecycle remains entirely explicit.

## Non-goals

- Inferring ephemeral policy from age, runtime status, adapter inventory, or
  provider child metadata.
- Parsing terminal bytes to determine completion, approval, or handoff.
- Creating a parallel task, artifact, cleanup, or runtime-control framework.
- Automatically deleting durable provenance, branches, worktrees,
  transcripts, or the parent Herdr workspace.
- Adding UI, provider-specific fork support, or a speculative adapter factory
  in the initial slices.

## Remaining decisions

- Choose the default grace TTL and its operator configuration boundary.
- Decide whether a failed ephemeral assignment may retire with a failure
  summary Artifact or requires only successful completion.
- Define the exact active-versus-history projection for retirement-pending and
  retired tombstones.
- Decide whether eligible parents include only project orchestrators or also
  Yard and coordination orchestrators.
- Define durable lease abandonment and operator-revocation policy without
  treating process restart or disconnect as release.
- Define the adapter-neutral composition of observation epoch and monotonic
  generation.
- Add an identity-guarded Herdr compare-and-close capability; until then,
  stopped tabs remain ineligible for automatic tombstone retirement under
  Herdr 0.9.0.

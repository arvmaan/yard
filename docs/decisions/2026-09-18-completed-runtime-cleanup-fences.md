# ADR: Completed Runtime Cleanup Fences

- Status: Proposed
- Date: 2026-09-18

## Context

Yard can authoritatively identify a conservative backlog candidate only when the worker's latest allocation is `create_new` and that allocation's assignment has a persisted completion receipt and completed attempt. The completion transaction already persists receipt and artifact links before moving the assignment and attempt to completed.

Yard cannot yet authoritatively close that runtime. The inspected system has no explicit cleanup policy on a worker profile or assignment, no durable approval authority, no durable terminal-lease fence, and no persisted adapter/session epoch plus monotonic observation generation. Herdr protocol 22 closes a tab or pane by target ID only; it does not provide identity-generation compare-and-close. The existing retirement adapter therefore refuses a present runtime when an atomic identity guard is unavailable.

## Decision

Expose only a read-only, bounded preview of completed Yard-created runtime candidates. Every candidate is retained and includes stable reasons. The preview exposes no close, confirmation, queue, scheduler, or cleanup mutation. Missing data, inventory ambiguity, and unavailable fencing remain fail-closed. The request defaults to 50 candidates, rejects limits above 100, materializes at most `limit + 1` candidates before correlated retention checks, and returns explicit truncation metadata.

The preview is not retirement eligibility. It ranks all allocations for each worker before applying candidate filters and considers a worker only when the latest allocation is `create_new` and that allocation joins to a completed assignment, completed attempt, and persisted receipt. A normal worker reuse writes an `adopt_existing` allocation with an assignment; a completed worker handoff writes a `handoff` allocation with an assignment; a project-orchestrator transfer writes an assignmentless `adopt_existing` allocation. Any of those as the latest allocation excludes the worker. Workers whose durable worker record is ended, and allocations without an accepted completion receipt, are also excluded. Remaining candidates report missing artifacts, unresolved blockers, and project, Yard, or coordination-node ownership. Pending intervention and overlapping open-assignment reasons are defensive signals for legacy or inconsistent persisted state; current command paths prevent those states.

Before any confirmation or automatic closure is added, all of these authorities must exist:

1. An explicit cleanup policy on the profile or assignment, default-off unless temporary creation semantics are durable and unambiguous.
2. Durable approval state with a versioned authoritative `clear` decision; missing or unavailable state fails closed.
3. Durable terminal-lease registration where lease acquisition and retirement share one atomic fence.
4. A persisted adapter/session epoch and monotonic observation generation for the exact runtime identity and topology.
5. Adapter support for atomic identity-generation compare-and-close, followed by an atomic final revalidation of assignment, receipt, required artifact links, approvals, lease fence, runtime identity, observation generation, topology, and grace deadline.
6. Idempotent audited retry through the existing `runtime_cleanup_jobs` boundary, without a parallel scheduler and without deleting worker, assignment, receipt, artifact, or audit provenance.

Restart, inventory outage, ambiguity, stale observation, generation change, topology change, identity change, active lease, new assignment, pending approval, unsupported adapter, or unpersisted output must refuse closure.

## Consequences

The current product can enumerate and explain the backlog safely, but zero candidates are close-ready. A future destructive action requires the fences above and a separate reviewed change.

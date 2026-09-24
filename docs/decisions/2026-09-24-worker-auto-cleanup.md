# ADR: Worker Auto Cleanup Boundary

- Status: Proposed
- Date: 2026-09-24

## Decision

Yard persists deterministic cleanup policy, runs, and claimed jobs. Automation defaults off. Candidate selection revalidates ownership, assignment completion, artifacts, grace, and runtime identity. Missing panes are reconciled without a close; unavailable or conflicting observation, advisor, lease, and close outcomes stop at review.

Cleanup may close a live pane only through `close_if_management_leased` using an existing management lease. It never acquires a lease. The current Herdr adapter does not expose that lease protocol, so the provider-neutral production caller remains unsupported and fail-closed.

The typed advisor contract is retained, but real advisor worker spawning is not wired in this slice. The provider-neutral invoker returns unsupported and records review. Add runtime-specific spawning only when an adapter can return a completed assignment, receipt, and linked typed artifact through the existing contract.

## Consequences

The API and existing cleanup preview expose durable state without a new dashboard or speculative scheduler controls. Live retirement becomes available only when an adapter supplies the existing lease lookup and atomic close capability.

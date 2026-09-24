# ADR: Safe Pane Claims

- Status: Proposed
- Date: 2026-09-24

## Context

Yard can observe live Herdr agent panes, but observation does not grant control. Herdr's `pane_management_lease_v1` contract adds pane instance identity, explicit acquire/renew/release/status operations, exact request replay, and typed fail-closed errors. Herdr 0.9.0 does not expose this capability.

## Decision

Yard offers one bounded `Manage all agents` operation for at most 500 freshly observed panes. A pane is eligible only when one coherent interactive agent record matches one live pane instance and its Herdr workspace is explicitly bound to exactly one active Yard project. Existing Yard workers are never changed. Ambiguous, conflicting, unsupported, and already managed panes are reported separately.

Yard acquires the Herdr lease before atomically creating the worker, runtime binding, project allocation, and private lease record. If durable adoption fails, Yard releases the newly acquired lease. Command IDs and Herdr request IDs are deterministic and replay-safe. Lease tokens remain private SQLite data and redact from formatting and API types.

An existing background loop renews due leases. Typed lease loss marks the durable claim `recovery_required`; management controls then remain disabled. Inventory refresh remains read-only. This slice does not add unmanage, history, settings, terminal control, or automatic pane closure.

## Consequences

Unsupported Herdr versions show a disabled capability message. Partial batches return per-pane outcomes without changing existing workers. Recovery after lease loss is explicit future work; Yard does not infer control from runtime status.

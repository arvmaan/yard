import {
  resolveRuntimeCapabilities,
  runtimeCapabilityProcessState,
  runtimeCapabilityStatus,
} from './runtimeCapabilities'
import type {
  AssignmentLifecycle,
  RuntimeInventory,
  WorkerCandidate,
} from './types'

export type WorkerAttentionState = 'actionable' | 'quiet' | 'stale'

export function workerCurrentlyObserved(
  candidate: WorkerCandidate,
  snapshotCurrent: boolean,
  inventory: RuntimeInventory | null,
) {
  const runtime = candidate.worker.runtime
  if (
    !snapshotCurrent ||
    !runtime ||
    !inventory ||
    inventory.adapter !== runtime.adapter ||
    inventory.session !== runtime.session
  ) {
    return false
  }
  return (
    inventory.workers.some(
      (worker) => worker.terminal_id === runtime.terminal_id,
    ) ||
    inventory.panes.some(
      (pane) => pane.terminal_id === runtime.terminal_id,
    )
  )
}

export function workerAttentionState(
  candidate: WorkerCandidate,
  snapshotCurrent: boolean,
  inventory: RuntimeInventory | null,
  latestAssignmentLifecycle: AssignmentLifecycle | null = null,
): WorkerAttentionState {
  // Ended sessions and work the user ended without completion are settled:
  // they never ask for attention and are never offered as stale.
  if (
    candidate.worker.desired_state === 'ended' ||
    candidate.availability === 'ended' ||
    latestAssignmentLifecycle === 'cancelled'
  ) {
    return 'quiet'
  }

  const capabilities = resolveRuntimeCapabilities(
    snapshotCurrent,
    candidate.worker.runtime,
    inventory,
  )
  const recoverableOrchestrator =
    (candidate.availability === 'orchestrator' ||
      candidate.availability === 'yard_orchestrator') &&
    (capabilities.reason === 'binding_missing' ||
      capabilities.reason === 'identity_mismatch')
  if (recoverableOrchestrator) return 'actionable'

  const currentlyObserved = workerCurrentlyObserved(
    candidate,
    snapshotCurrent,
    inventory,
  )
  if (
    candidate.worker.ownership_kind === 'external' &&
    candidate.worker.runtime &&
    snapshotCurrent &&
    inventory?.adapter === candidate.worker.runtime.adapter &&
    inventory.session === candidate.worker.runtime.session &&
    !currentlyObserved
  ) {
    return 'stale'
  }
  if (candidate.worker.ownership_kind === 'system_ephemeral') {
    return 'quiet'
  }

  const status = runtimeCapabilityStatus(
    candidate.worker.runtime,
    capabilities,
  )
  if (currentlyObserved && status === 'blocked') return 'actionable'
  if (candidate.worker.ownership_kind !== 'yard_owned') return 'quiet'

  const processState = runtimeCapabilityProcessState(
    candidate.worker.runtime,
    capabilities,
  )
  return status === 'blocked' ||
    status === 'done' ||
    status === 'unknown' ||
    processState === 'exited' ||
    processState === 'unknown' ||
    candidate.availability === 'unavailable' ||
    candidate.availability === 'ambiguous'
    ? 'actionable'
    : 'quiet'
}

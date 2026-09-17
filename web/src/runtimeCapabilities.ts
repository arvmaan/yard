import type {
  ObservedStatus,
  ObservedWorker,
  PaneObservation,
  ProviderSessionRef,
  RuntimeInventory,
  RuntimeObservationState,
  RuntimeProcessState,
  WorkerRuntimeBinding,
} from './types'

export type RuntimeCapabilityReason =
  | 'ready'
  | 'stale'
  | 'binding_missing'
  | 'ambiguous'
  | 'identity_mismatch'
  | 'launch_pending'
  | 'exited'
  | 'unknown_process'
  | 'no_foreground_agent'

export interface RuntimeCapabilities {
  chat: boolean
  reason: RuntimeCapabilityReason
  terminal: boolean
}

export interface ResolvedRuntimeCapabilities extends RuntimeCapabilities {
  observedPane: PaneObservation | null
  observedWorker: ObservedWorker | null
}

export function providerSessionEquals(
  left: ProviderSessionRef | null,
  right: ProviderSessionRef | null,
) {
  if (!left || !right) return left === right
  return (
    left.source === right.source &&
    left.provider === right.provider &&
    left.kind === right.kind &&
    left.value === right.value
  )
}

function workerMatchesRuntimeIdentity(
  runtime: WorkerRuntimeBinding,
  observed: Pick<
    ObservedWorker,
    'pane_id' | 'provider_session' | 'tab_id' | 'terminal_id' | 'workspace_id'
  >,
) {
  return (
    observed.terminal_id === runtime.terminal_id &&
    observed.workspace_id === runtime.workspace_id &&
    observed.pane_id === runtime.pane_id &&
    observed.tab_id === (runtime.tab_id ?? '') &&
    providerSessionEquals(observed.provider_session, runtime.provider_session)
  )
}

function paneMatchesRuntimeTopology(
  runtime: WorkerRuntimeBinding,
  pane: Pick<
    PaneObservation,
    'runtime_id' | 'tab_id' | 'terminal_id' | 'workspace_id'
  >,
) {
  return (
    pane.terminal_id === runtime.terminal_id &&
    pane.workspace_id === runtime.workspace_id &&
    pane.runtime_id === runtime.pane_id &&
    pane.tab_id === (runtime.tab_id ?? '')
  )
}

function paneCoheresWithWorker(
  worker: Pick<
    ObservedWorker,
    'pane_id' | 'provider_session' | 'tab_id' | 'terminal_id' | 'workspace_id'
  >,
  pane: Pick<
    PaneObservation,
    'runtime_id' | 'provider_session' | 'tab_id' | 'terminal_id' | 'workspace_id'
  >,
) {
  return (
    pane.terminal_id === worker.terminal_id &&
    pane.workspace_id === worker.workspace_id &&
    pane.runtime_id === worker.pane_id &&
    pane.tab_id === worker.tab_id &&
    (!pane.provider_session ||
      !worker.provider_session ||
      providerSessionEquals(pane.provider_session, worker.provider_session))
  )
}

function currentTerminalIdentityKnown(
  observed:
    | Pick<
        ObservedWorker | PaneObservation,
        'display_provider' | 'provider' | 'provider_session'
      >
    | null,
) {
  if (!observed) return false
  return Boolean(
    observed.provider_session ?? observed.display_provider ?? observed.provider,
  )
}

export function resolveRuntimeCapabilities(
  snapshotCurrent: boolean,
  runtime: WorkerRuntimeBinding | null | undefined,
  inventory: RuntimeInventory | null | undefined,
): ResolvedRuntimeCapabilities {
  const unresolved = (
    reason: RuntimeCapabilityReason,
  ): ResolvedRuntimeCapabilities => ({
    chat: false,
    observedPane: null,
    observedWorker: null,
    reason,
    terminal: false,
  })

  if (!runtime) return unresolved('binding_missing')
  if (!snapshotCurrent) return unresolved('stale')
  if (
    !inventory ||
    inventory.adapter !== runtime.adapter ||
    inventory.session !== runtime.session
  ) {
    return unresolved('binding_missing')
  }

  const workerTerminalMatches = inventory.workers.filter(
    (worker) => worker.terminal_id === runtime.terminal_id,
  )
  const paneTerminalMatches = inventory.panes.filter(
    (pane) => pane.terminal_id === runtime.terminal_id,
  )
  if (workerTerminalMatches.length > 1 || paneTerminalMatches.length > 1) {
    return unresolved('ambiguous')
  }

  const workerTerminalMatch = workerTerminalMatches[0] ?? null
  const paneTerminalMatch = paneTerminalMatches[0] ?? null
  const workerIdentityMatches = workerTerminalMatches.filter((worker) =>
    workerMatchesRuntimeIdentity(runtime, worker),
  )
  const paneTopologyMatches = paneTerminalMatches.filter((pane) =>
    paneMatchesRuntimeTopology(runtime, pane),
  )
  if (workerIdentityMatches.length > 1 || paneTopologyMatches.length > 1) {
    return unresolved('ambiguous')
  }

  if (workerTerminalMatch && paneTerminalMatch) {
    if (!paneCoheresWithWorker(workerTerminalMatch, paneTerminalMatch)) {
      return unresolved('ambiguous')
    }
    const workerExact = workerIdentityMatches.length === 1
    const paneExact = paneTopologyMatches.length === 1
    if (workerExact !== paneExact) {
      return unresolved('ambiguous')
    }
    if (!workerExact && !paneExact) {
      return unresolved('identity_mismatch')
    }
  }

  const observedWorker = workerIdentityMatches[0] ?? null
  const observedPane = paneTopologyMatches[0] ?? null
  if (!observedWorker && !observedPane) {
    if (workerTerminalMatch || paneTerminalMatch) {
      return unresolved('identity_mismatch')
    }
    return unresolved('binding_missing')
  }

  const resolved = (
    reason: RuntimeCapabilityReason,
    terminal: boolean,
    chat: boolean,
  ): ResolvedRuntimeCapabilities => ({
    chat,
    observedPane,
    observedWorker,
    reason,
    terminal,
  })

  if (runtime.process_state === 'exited') {
    return resolved('exited', false, false)
  }
  if (runtime.process_state !== 'running') {
    return resolved('unknown_process', false, false)
  }
  if (observedWorker?.launch_pending) {
    return resolved('launch_pending', false, false)
  }
  if (!observedWorker) {
    return resolved('no_foreground_agent', true, false)
  }
  if (!currentTerminalIdentityKnown(observedWorker)) {
    return resolved('no_foreground_agent', true, false)
  }
  return resolved('ready', true, true)
}

export function runtimeCapabilityStatus(
  runtime: WorkerRuntimeBinding | null | undefined,
  capabilities: ResolvedRuntimeCapabilities,
): ObservedStatus {
  return (
    capabilities.observedWorker?.status ??
    capabilities.observedPane?.status ??
    runtime?.status ??
    'unknown'
  )
}

export function runtimeCapabilityProcessState(
  runtime: WorkerRuntimeBinding | null | undefined,
  capabilities: ResolvedRuntimeCapabilities,
): RuntimeProcessState {
  return (
    runtime?.process_state ??
    (capabilities.observedWorker || capabilities.observedPane
      ? ('running' as const)
      : ('unknown' as const))
  )
}

export function runtimeCapabilityLabel(
  capabilities: RuntimeCapabilities,
) {
  switch (capabilities.reason) {
    case 'stale':
      return 'Connection status stale'
    case 'binding_missing':
      return 'Binding missing'
    case 'ambiguous':
      return 'Observed · ambiguous'
    case 'identity_mismatch':
      return 'Observed · identity changed'
    case 'launch_pending':
      return 'Observed · launch pending'
    case 'exited':
      return 'Observed · process exited'
    case 'unknown_process':
      return 'Observed · process unknown'
    case 'no_foreground_agent':
      return 'Observed · terminal only'
    case 'ready':
      return 'observed'
  }
}

export function runtimeCapabilityDetail(
  capabilities: RuntimeCapabilities,
) {
  switch (capabilities.reason) {
    case 'stale':
      return 'Latest Herdr snapshot refresh failed. Showing last-known details; live controls are unavailable.'
    case 'binding_missing':
      return 'Worker not found in latest Herdr snapshot.'
    case 'ambiguous':
      return 'More than one live Herdr pane or worker matched this durable terminal identity.'
    case 'identity_mismatch':
      return "Latest Herdr observation no longer matches Yard's durable workspace, tab, pane, or provider binding."
    case 'launch_pending':
      return 'Managed launch is still pending. Wait for Herdr to finish attaching the foreground process.'
    case 'exited':
      return 'Yard no longer sees a running foreground process for this worker.'
    case 'unknown_process':
      return 'Yard cannot confirm that the foreground process is still running.'
    case 'no_foreground_agent':
      return 'Terminal reattach is still available, but chat stays disabled until Yard reports a foreground agent.'
    case 'ready':
      return undefined
  }
}

export function runtimeCapabilityObservationState(
  capabilities: ResolvedRuntimeCapabilities,
  runtime: WorkerRuntimeBinding | null | undefined,
): RuntimeObservationState {
  if (capabilities.reason === 'ambiguous') {
    return 'ambiguous'
  }
  if (capabilities.reason === 'binding_missing') {
    return 'missing'
  }
  return capabilities.observedWorker || capabilities.observedPane
    ? (runtime?.observation_state ?? ('observed' as const))
    : ('missing' as const)
}

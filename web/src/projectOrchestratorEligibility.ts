import type {
  ObservedWorker,
  Project,
  ProviderSessionRef,
  RuntimeInventory,
  WorkerCandidate,
  WorkerRuntimeBinding,
} from './types'

export type ProjectOrchestratorEligibilityReason =
  | 'eligible'
  | 'inventory_unavailable'
  | 'inventory_identity_changed'
  | 'project_workspace_unavailable'
  | 'current_orchestrator_unavailable'
  | 'no_eligible_workers'

export interface ProjectOrchestratorEligibility {
  candidates: WorkerCandidate[]
  reason: ProjectOrchestratorEligibilityReason
}

function providerSessionEquals(
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

function runtimeMatchesProject(
  project: Project,
  runtime: WorkerRuntimeBinding,
) {
  return (
    runtime.adapter === project.runtime.adapter &&
    runtime.session === project.runtime.session &&
    runtime.workspace_id === project.runtime.workspace_id &&
    runtime.observation_state === 'observed' &&
    runtime.process_state === 'running'
  )
}

function uniqueObservedWorker(
  inventory: RuntimeInventory,
  runtime: WorkerRuntimeBinding,
): ObservedWorker | undefined {
  const terminalMatches = inventory.workers.filter(
    (worker) => worker.terminal_id === runtime.terminal_id,
  )
  if (terminalMatches.length !== 1) return undefined

  const observed = terminalMatches[0]
  if (
    observed.workspace_id !== runtime.workspace_id ||
    observed.pane_id !== runtime.pane_id ||
    observed.tab_id !== runtime.tab_id ||
    !providerSessionEquals(
      observed.provider_session,
      runtime.provider_session,
    ) ||
    !observed.interactive_ready ||
    observed.launch_pending
  ) {
    return undefined
  }

  if (
    runtime.provider_session &&
    inventory.workers.filter((worker) =>
      providerSessionEquals(
        worker.provider_session,
        runtime.provider_session,
      ),
    ).length !== 1
  ) {
    return undefined
  }

  return observed
}

export function projectOrchestratorEligibility(
  inventory: RuntimeInventory | null,
  project: Project,
  candidates: WorkerCandidate[],
): ProjectOrchestratorEligibility {
  const orchestratorRuntime = project.orchestrator.runtime
  if (!inventory) {
    return { candidates: [], reason: 'inventory_unavailable' }
  }
  if (
    inventory.adapter !== project.runtime.adapter ||
    inventory.session !== project.runtime.session
  ) {
    return { candidates: [], reason: 'inventory_identity_changed' }
  }
  if (
    inventory.workspaces.filter(
      (workspace) =>
        workspace.runtime_id === project.runtime.workspace_id,
    ).length !== 1
  ) {
    return { candidates: [], reason: 'project_workspace_unavailable' }
  }
  if (
    !orchestratorRuntime ||
    !runtimeMatchesProject(project, orchestratorRuntime) ||
    orchestratorRuntime.last_observed_at_unix_ms !==
      inventory.observed_at_unix_ms
  ) {
    return { candidates: [], reason: 'current_orchestrator_unavailable' }
  }

  const displaced = uniqueObservedWorker(inventory, orchestratorRuntime)
  if (!displaced) {
    return { candidates: [], reason: 'current_orchestrator_unavailable' }
  }

  const eligibleCandidates = candidates.filter((candidate) => {
    const runtime = candidate.worker.runtime
    if (
      candidate.worker.id === project.orchestrator.id ||
      candidate.availability !== 'unassigned_live' ||
      !runtime ||
      !runtimeMatchesProject(project, runtime) ||
      runtime.last_observed_at_unix_ms !== inventory.observed_at_unix_ms
    ) {
      return false
    }

    const promoted = uniqueObservedWorker(inventory, runtime)
    return Boolean(
      promoted && promoted.runtime_id !== displaced.runtime_id,
    )
  })

  return eligibleCandidates.length > 0
    ? { candidates: eligibleCandidates, reason: 'eligible' }
    : { candidates: [], reason: 'no_eligible_workers' }
}

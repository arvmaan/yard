import {
  providerSessionEquals,
  resolveRuntimeCapabilities,
} from './runtimeCapabilities'
import type {
  ObservedWorker,
  Project,
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

function eligibleObservedWorker(
  inventory: RuntimeInventory,
  runtime: WorkerRuntimeBinding,
): ObservedWorker | undefined {
  const capabilities = resolveRuntimeCapabilities(true, runtime, inventory)
  const observed = capabilities.observedWorker
  if (
    !observed ||
    capabilities.reason !== 'ready' ||
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

  const displaced = eligibleObservedWorker(inventory, orchestratorRuntime)
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

    const promoted = eligibleObservedWorker(inventory, runtime)
    return Boolean(
      promoted && promoted.runtime_id !== displaced.runtime_id,
    )
  })

  return eligibleCandidates.length > 0
    ? { candidates: eligibleCandidates, reason: 'eligible' }
    : { candidates: [], reason: 'no_eligible_workers' }
}

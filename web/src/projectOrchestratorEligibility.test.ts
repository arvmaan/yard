import { describe, expect, it } from 'vitest'
import { projectOrchestratorEligibility } from './projectOrchestratorEligibility'
import type {
  ObservedWorker,
  Project,
  ProviderSessionRef,
  RuntimeInventory,
  Worker,
  WorkerCandidate,
  WorkerRuntimeBinding,
  WorkspaceObservation,
} from './types'

const now = 1_786_400_000_000

function provider(value: string): ProviderSessionRef {
  return {
    source: 'herdr:codex',
    provider: 'codex',
    kind: 'id',
    value,
  }
}

function runtime(
  terminalId: string,
  session = 'alpha',
  providerSession: ProviderSessionRef | null = provider(terminalId),
): WorkerRuntimeBinding {
  return {
    adapter: 'herdr',
    session,
    workspace_id: 'workspace-1',
    terminal_id: terminalId,
    tab_id: `tab-${terminalId}`,
    pane_id: `pane-${terminalId}`,
    provider_session: providerSession,
    owns_tab: true,
    observation_state: 'observed',
    process_state: 'running',
    status: 'idle',
    state_change_sequence: '1',
    revision: '1',
    version: '1',
    last_observed_at_unix_ms: now,
  }
}

function worker(id: string, workerRuntime: WorkerRuntimeBinding): Worker {
  return {
    id,
    profile_id: null,
    profile_version: null,
    desired_state: 'running',
    runtime: workerRuntime,
    version: '1',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  }
}

function observed(
  workerRuntime: WorkerRuntimeBinding,
  runtimeId = workerRuntime.terminal_id,
): ObservedWorker {
  return {
    runtime_id: runtimeId,
    terminal_id: workerRuntime.terminal_id,
    workspace_id: workerRuntime.workspace_id,
    tab_id: workerRuntime.tab_id ?? '',
    pane_id: workerRuntime.pane_id,
    name: workerRuntime.terminal_id,
    provider: 'codex',
    display_provider: 'Codex',
    status: 'idle',
    focused: false,
    launch_pending: false,
    interactive_ready: true,
    state_change_sequence: '1',
    cwd: '/tmp/yard',
    foreground_cwd: '/tmp/yard',
    tokens: {},
    provider_session: workerRuntime.provider_session,
    revision: '1',
  }
}

function workspace(): WorkspaceObservation {
  return {
    runtime_id: 'workspace-1',
    order: 1,
    label: 'Yard',
    focused: true,
    active_tab_id: 'tab-current',
    pane_count: 2,
    tab_count: 2,
    status: 'idle',
    tokens: {},
    worktree: null,
  }
}

function fixture(session = 'alpha') {
  const currentRuntime = runtime('current', session)
  const candidateRuntime = runtime('candidate', session)
  const project: Project = {
    id: 'project-1',
    name: 'Yard',
    runtime: {
      adapter: 'herdr',
      session,
      workspace_id: 'workspace-1',
    },
    orchestrator: worker('current-worker', currentRuntime),
    placement: {
      geometry: { x: 0, y: 0, width: 320, height: 240 },
      version: '1',
      updated_at_unix_ms: now,
    },
    workflow_profile: {
      profile_id: 'profile-1',
      profile_version: '1',
      pinned_by: 'local-user',
      pinned_at_unix_ms: now,
    },
    version: '1',
    created_at_unix_ms: now,
    updated_at_unix_ms: now,
  }
  const candidate: WorkerCandidate = {
    worker: worker('candidate-worker', candidateRuntime),
    availability: 'unassigned_live',
  }
  const inventory: RuntimeInventory = {
    adapter: 'herdr',
    session,
    runtime_version: '1',
    protocol: 1,
    observed_at_unix_ms: now,
    focus: {
      workspace_id: 'workspace-1',
      tab_id: 'tab-current',
      pane_id: 'pane-current',
    },
    workspaces: [workspace()],
    tabs: [],
    panes: [],
    workers: [observed(currentRuntime), observed(candidateRuntime)],
    child_agents: [],
  }
  return { candidate, candidateRuntime, currentRuntime, inventory, project }
}

describe('projectOrchestratorEligibility', () => {
  it('accepts uniquely observed live runtimes in the project workspace', () => {
    const { candidate, inventory, project } = fixture()

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]),
    ).toEqual({ candidates: [candidate], reason: 'eligible' })
  })

  it('uses inventory from the project runtime session', () => {
    const { candidate, inventory, project } = fixture('beta')

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('eligible')
    expect(
      projectOrchestratorEligibility(
        { ...inventory, session: 'alpha' },
        project,
        [candidate],
      ).reason,
    ).toBe('inventory_identity_changed')
  })

  it('rejects stale candidate pane topology', () => {
    const { candidate, inventory, project } = fixture()
    inventory.workers[1] = {
      ...inventory.workers[1],
      pane_id: 'pane-stale',
    }

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')
  })

  it('rejects stale current orchestrator topology', () => {
    const { candidate, inventory, project } = fixture()
    inventory.workers[0] = {
      ...inventory.workers[0],
      tab_id: 'tab-stale',
    }

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('current_orchestrator_unavailable')
  })

  it('rejects a displaced orchestrator runtime observation timestamp mismatch', () => {
    const { candidate, inventory, project } = fixture()
    project.orchestrator.runtime = {
      ...project.orchestrator.runtime!,
      last_observed_at_unix_ms: now - 1,
    }

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('current_orchestrator_unavailable')
  })

  it('rejects a promoted worker runtime observation timestamp mismatch', () => {
    const { candidate, inventory, project } = fixture()
    candidate.worker.runtime = {
      ...candidate.worker.runtime!,
      last_observed_at_unix_ms: now - 1,
    }

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')
  })

  it('rejects ambiguous provider identity for either runtime', () => {
    const { candidate, candidateRuntime, inventory, project } = fixture()
    inventory.workers.push({
      ...observed(runtime('other')),
      provider_session: candidateRuntime.provider_session,
    })

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')

    inventory.workers[2] = {
      ...inventory.workers[2],
      provider_session: project.orchestrator.runtime?.provider_session ?? null,
    }
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('current_orchestrator_unavailable')
  })

  it('requires exactly one project workspace and terminal match', () => {
    const { candidate, inventory, project } = fixture()
    inventory.workspaces.push(workspace())
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('project_workspace_unavailable')

    inventory.workspaces.pop()
    inventory.workers.push({ ...inventory.workers[1] })
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')
  })

  it('requires observed running durable state and interactive readiness', () => {
    const { candidate, inventory, project } = fixture()
    candidate.worker.runtime = {
      ...candidate.worker.runtime!,
      observation_state: 'missing',
    }
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')

    candidate.worker.runtime = {
      ...candidate.worker.runtime!,
      observation_state: 'observed',
    }
    inventory.workers[1] = {
      ...inventory.workers[1],
      launch_pending: true,
    }
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')
  })

  it('requires exact provider equality and distinct observed runtimes', () => {
    const { candidate, inventory, project } = fixture()
    inventory.workers[1] = {
      ...inventory.workers[1],
      provider_session: provider('stale-provider'),
    }
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')

    inventory.workers[1] = {
      ...observed(candidate.worker.runtime!),
      runtime_id: inventory.workers[0].runtime_id,
    }
    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')
  })

  it('rejects the current orchestrator worker even with eligible topology', () => {
    const { candidate, inventory, project } = fixture()
    candidate.worker.id = project.orchestrator.id

    expect(
      projectOrchestratorEligibility(inventory, project, [candidate]).reason,
    ).toBe('no_eligible_workers')
  })
})

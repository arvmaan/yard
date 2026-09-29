import { describe, expect, it } from 'vitest'
import { workerAttentionState } from './workerAttention'
import type {
  RuntimeInventory,
  WorkerCandidate,
  WorkerOwnershipKind,
} from './types'

function candidate(
  ownership_kind: WorkerOwnershipKind,
  availability: WorkerCandidate['availability'] = 'unavailable',
): WorkerCandidate {
  return {
    availability,
    worker: {
      created_at_unix_ms: 1,
      desired_state: 'running',
      id: `worker-${ownership_kind}`,
      ownership_kind,
      profile_id: null,
      profile_version: null,
      runtime: {
        adapter: 'herdr',
        last_observed_at_unix_ms: 1,
        observation_state: 'missing',
        owns_tab: false,
        pane_id: 'pane-1',
        process_state: 'unknown',
        provider_session: null,
        revision: '1',
        session: 'alpha',
        state_change_sequence: '1',
        status: 'unknown',
        tab_id: 'tab-1',
        terminal_id: 'terminal-1',
        version: '1',
        workspace_id: 'workspace-1',
      },
      updated_at_unix_ms: 1,
      version: '1',
    },
  }
}

const inventory = {
  adapter: 'herdr',
  child_agents: [],
  focus: { pane_id: null, tab_id: null, workspace_id: null },
  observed_at_unix_ms: 1,
  panes: [],
  protocol: 1,
  runtime_version: '1',
  session: 'alpha',
  tabs: [],
  workers: [],
  workspaces: [],
} satisfies RuntimeInventory

describe('workerAttentionState', () => {
  it('separates stale external workers from actionable owned workers', () => {
    expect(workerAttentionState(candidate('external'), true, inventory)).toBe(
      'stale',
    )
    expect(workerAttentionState(candidate('yard_owned'), true, inventory)).toBe(
      'actionable',
    )
    expect(
      workerAttentionState(candidate('system_ephemeral'), true, inventory),
    ).toBe('quiet')
  })

  it('keeps recoverable project orchestrators actionable', () => {
    expect(
      workerAttentionState(
        candidate('external', 'orchestrator'),
        true,
        inventory,
      ),
    ).toBe('actionable')
  })

  it('keeps an observed blocked external worker actionable', () => {
    const observed = candidate('external', 'unassigned_live')
    observed.worker.runtime!.status = 'blocked'
    expect(
      workerAttentionState(observed, true, {
        ...inventory,
        panes: [
          {
            cwd: null,
            display_provider: 'Codex',
            focused: false,
            foreground_cwd: null,
            label: null,
            provider: 'codex',
            provider_session: null,
            revision: '1',
            runtime_id: 'pane-1',
            status: 'blocked',
            tab_id: 'tab-1',
            terminal_id: 'terminal-1',
            tokens: {},
            workspace_id: 'workspace-1',
          },
        ],
      }),
    ).toBe('actionable')
  })
})

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

  it('keeps an adopted external worker with open work quiet when done (D10)', () => {
    // Fork-era auto-adopted workers and restored orchestrators stay
    // `external` (mainline backfills `yard_owned` only for `create_new`
    // workers). Mainline asks for attention only for `yard_owned` workers, so
    // an external one with an open allocation that is done stays quiet.
    const adopted = candidate('external', 'assigned')
    adopted.worker.runtime!.status = 'done'
    adopted.worker.runtime!.observation_state = 'observed'
    adopted.worker.runtime!.process_state = 'running'
    expect(
      workerAttentionState(adopted, true, {
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
            status: 'done',
            tab_id: 'tab-1',
            terminal_id: 'terminal-1',
            tokens: {},
            workspace_id: 'workspace-1',
          },
        ],
      }),
    ).toBe('quiet')
    // The same worker owned by Yard asks for attention.
    adopted.worker.ownership_kind = 'yard_owned'
    expect(workerAttentionState(adopted, true, inventory)).toBe('actionable')
  })

  it('keeps cancelled and ended workers quiet instead of stale', () => {
    // Unobserved external workers are stale, unless their latest work was
    // ended without completion or their session has ended.
    expect(
      workerAttentionState(candidate('external'), true, inventory, 'completed'),
    ).toBe('stale')
    expect(
      workerAttentionState(candidate('external'), true, inventory, 'cancelled'),
    ).toBe('quiet')
    expect(
      workerAttentionState(
        candidate('yard_owned'),
        true,
        inventory,
        'cancelled',
      ),
    ).toBe('quiet')
    expect(
      workerAttentionState(candidate('external', 'ended'), true, inventory),
    ).toBe('quiet')
    const ended = candidate('external')
    ended.worker.desired_state = 'ended'
    expect(workerAttentionState(ended, true, inventory)).toBe('quiet')
  })
})

import { describe, expect, it } from 'vitest'
import {
  reconcileInventorySnapshot,
  reconcileJsonSnapshot,
  reconcileRuntimeProjectionSnapshot,
} from './inventoryState'
import type { RuntimeInventory } from './types'

function inventory(): RuntimeInventory {
  return {
    adapter: 'herdr',
    session: 'alpha',
    runtime_version: '1',
    protocol: 1,
    observed_at_unix_ms: 100,
    focus: {
      workspace_id: 'workspace-1',
      tab_id: 'tab-1',
      pane_id: 'pane-1',
    },
    workspaces: [],
    tabs: [],
    panes: [],
    workers: [
      {
        runtime_id: 'runtime-1',
        terminal_id: 'terminal-1',
        workspace_id: 'workspace-1',
        tab_id: 'tab-1',
        pane_id: 'pane-1',
        name: 'worker-1',
        provider: 'codex',
        display_provider: 'Codex',
        status: 'working',
        focused: true,
        launch_pending: false,
        interactive_ready: true,
        state_change_sequence: '1',
        cwd: '/workspace',
        foreground_cwd: '/workspace',
        tokens: { input: '10', output: '5' },
        provider_session: null,
        revision: '1',
      },
    ],
    child_agents: [],
  }
}

describe('reconcileInventorySnapshot', () => {
  it('uses the first observed inventory', () => {
    const next = inventory()
    expect(reconcileInventorySnapshot(null, next)).toBe(next)
  })

  it('preserves state identity when only observation time and key order change', () => {
    const current = inventory()
    const next = {
      ...current,
      observed_at_unix_ms: 200,
      workers: current.workers.map((worker) => ({
        ...worker,
        tokens: { output: '5', input: '10' },
      })),
    }

    expect(reconcileInventorySnapshot(current, next)).toBe(current)
  })

  it('adopts a snapshot when observed runtime data changes', () => {
    const current = inventory()
    const next: RuntimeInventory = {
      ...current,
      observed_at_unix_ms: 200,
      workers: current.workers.map((worker) => ({
        ...worker,
        status: 'blocked',
      })),
    }

    expect(reconcileInventorySnapshot(current, next)).toBe(next)
  })
})

describe('reconcileJsonSnapshot', () => {
  it('preserves associated polling snapshots when their values are unchanged', () => {
    const current = [{ id: 'worker-1', status: 'working' }]
    const unchanged = [{ id: 'worker-1', status: 'working' }]
    const changed = [{ id: 'worker-1', status: 'blocked' }]

    expect(reconcileJsonSnapshot(current, unchanged)).toBe(current)
    expect(reconcileJsonSnapshot(current, changed)).toBe(changed)
  })
})

describe('reconcileRuntimeProjectionSnapshot', () => {
  it('ignores only volatile nested observation timestamps', () => {
    const current = {
      worker: {
        runtime: {
          status: 'working',
          last_observed_at_unix_ms: 100,
        },
      },
    }
    const observedLater = {
      worker: {
        runtime: {
          status: 'working',
          last_observed_at_unix_ms: 200,
        },
      },
    }
    const changed = {
      worker: {
        runtime: {
          status: 'blocked',
          last_observed_at_unix_ms: 200,
        },
      },
    }

    expect(
      reconcileRuntimeProjectionSnapshot(current, observedLater),
    ).toBe(current)
    expect(reconcileRuntimeProjectionSnapshot(current, changed)).toBe(
      changed,
    )
  })
})

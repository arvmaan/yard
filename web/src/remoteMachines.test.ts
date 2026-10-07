import { describe, expect, it } from 'vitest'
import {
  buildAddMachineCommand,
  buildReconnectCommand,
  endpointKey,
  parseMachineInventory,
  parseRuntimeEndpoints,
  selectedEndpointAfterRefresh,
} from './remoteMachines'
import type { RuntimeEndpoint, RuntimeInventory } from './types'

const a = '0123456789abcdef0123456789abcdef'
const b = 'abcdef0123456789abcdef0123456789'

const machine = (id: string, label = 'Box'): RuntimeEndpoint => ({
  endpoint: { kind: 'machine', machine_id: id },
  label,
  enabled: true,
  connection_state: 'reachable',
  capabilities: {
    inventory_read: true,
    mutations: false,
    terminal_streaming: false,
  },
  sessions: [
    { name: 'default', is_default: true, observed_running: true },
  ],
})

const local: RuntimeEndpoint = {
  endpoint: { kind: 'local' },
  label: 'Local',
  enabled: true,
  connection_state: 'reachable',
  capabilities: {
    inventory_read: true,
    mutations: true,
    terminal_streaming: true,
  },
  sessions: [{ name: 'alpha', is_default: true, observed_running: true }],
}

const inventory: RuntimeInventory = {
  adapter: 'herdr',
  session: 'default',
  runtime_version: '0.9.3',
  protocol: 19,
  observed_at_unix_ms: 1,
  focus: { workspace_id: 'w1', tab_id: 't1', pane_id: 'p1' },
  workspaces: [
    {
      runtime_id: 'w1',
      order: 1,
      label: 'Workspace',
      focused: true,
      active_tab_id: 't1',
      pane_count: 1,
      tab_count: 1,
      status: 'working',
      tokens: {},
      worktree: null,
    },
  ],
  tabs: [
    {
      runtime_id: 't1',
      workspace_id: 'w1',
      order: 1,
      label: 'Tab',
      focused: true,
      pane_count: 1,
      status: 'working',
    },
  ],
  panes: [
    {
      runtime_id: 'p1',
      pane_instance_id: null,
      terminal_id: 'terminal-1',
      workspace_id: 'w1',
      tab_id: 't1',
      focused: true,
      cwd: '/work',
      foreground_cwd: '/work',
      label: 'Pane',
      provider: 'codex',
      display_provider: 'Codex',
      status: 'working',
      tokens: {},
      provider_session: null,
      revision: '1',
    },
  ],
  workers: [
    {
      runtime_id: 'terminal-1',
      pane_instance_id: null,
      terminal_id: 'terminal-1',
      workspace_id: 'w1',
      tab_id: 't1',
      pane_id: 'p1',
      name: 'Agent',
      provider: 'codex',
      display_provider: 'Codex',
      status: 'working',
      focused: true,
      launch_pending: false,
      interactive_ready: true,
      state_change_sequence: '1',
      cwd: '/work',
      foreground_cwd: '/work',
      tokens: {},
      provider_session: null,
      revision: '1',
    },
  ],
  child_agents: [],
}

const response = (value: unknown = inventory, id = a) => ({
  endpoint: { kind: 'machine', machine_id: id },
  inventory: value,
})

const changedInventory = (change: (value: Record<string, any>) => void) => {
  const value = structuredClone(inventory) as unknown as Record<string, any>
  change(value)
  return value
}

describe('remote machine boundary', () => {
  it('parses capability and disconnected states', () => {
    const result = parseRuntimeEndpoints({
      adapter: 'herdr',
      endpoints: [
        local,
        machine(a),
        { ...machine(b), enabled: false, connection_state: 'disabled' },
      ],
    })
    expect(
      result.endpoints.map((candidate) => endpointKey(candidate.endpoint)),
    ).toEqual(['local', `machine:${a}`, `machine:${b}`])
    expect(result.endpoints[1].capabilities.mutations).toBe(false)
  })

  it('accepts an explicitly empty endpoint list', () => {
    expect(
      parseRuntimeEndpoints({ adapter: 'herdr', endpoints: [] }),
    ).toEqual({ adapter: 'herdr', endpoints: [] })
  })

  it('rejects malformed, duplicate, or missing Local endpoints', () => {
    expect(() =>
      parseRuntimeEndpoints({ adapter: 'herdr', endpoints: [machine(a)] }),
    ).toThrow()
    expect(() =>
      parseRuntimeEndpoints({
        adapter: 'herdr',
        endpoints: [local, machine(a), machine(a)],
      }),
    ).toThrow()
    expect(() =>
      parseRuntimeEndpoints({
        adapter: 'herdr',
        endpoints: [
          local,
          { ...machine(a), connection_state: 'connected' },
        ],
      }),
    ).toThrow()
  })

  it('preserves selection across rename and returns missing machine to Local', () => {
    const key = `machine:${a}`
    expect(
      selectedEndpointAfterRefresh(key, [local, machine(a, 'Renamed')]),
    ).toBe(key)
    expect(selectedEndpointAfterRefresh(key, [local, machine(b)])).toBe(
      'local',
    )
  })

  it('requires matching endpoint-qualified inventory', () => {
    const parsed = parseMachineInventory(response(), a).inventory
    expect(parsed).toEqual(inventory)
    expect(parsed).not.toBe(inventory)
    expect(() =>
      parseMachineInventory({ endpoint: { kind: 'local' }, inventory }, a),
    ).toThrow('identity mismatch')
    expect(() => parseMachineInventory(response(inventory, b), a)).toThrow(
      'identity mismatch',
    )
  })

  it('rejects null, primitive, and array-shaped topology members', () => {
    for (const collection of ['workspaces', 'tabs', 'panes', 'workers']) {
      for (const member of [null, 'member', []]) {
        const value = changedInventory((candidate) => {
          candidate[collection][0] = member
        })
        expect(() => parseMachineInventory(response(value), a)).toThrow(
          `Invalid ${collection.slice(0, -1)}[0]`,
        )
      }
    }
  })

  it('rejects missing required scalar fields and invalid statuses', () => {
    const missing = changedInventory((value) => {
      delete value.workers[0].focused
    })
    const invalidStatus = changedInventory((value) => {
      value.tabs[0].status = 'busy'
    })
    expect(() => parseMachineInventory(response(missing), a)).toThrow(
      'worker[0].focused',
    )
    expect(() => parseMachineInventory(response(invalidStatus), a)).toThrow(
      'tab[0].status',
    )
  })

  it('rejects invalid nested topology and focus references', () => {
    const orphanedPane = changedInventory((value) => {
      value.panes[0].tab_id = 'missing-tab'
    })
    const wrongWorker = changedInventory((value) => {
      value.workers[0].workspace_id = 'another-workspace'
    })
    const orphanedFocus = changedInventory((value) => {
      value.focus.pane_id = 'missing-pane'
    })
    expect(() => parseMachineInventory(response(orphanedPane), a)).toThrow(
      'pane ancestry',
    )
    expect(() => parseMachineInventory(response(wrongWorker), a)).toThrow(
      'worker ancestry',
    )
    expect(() => parseMachineInventory(response(orphanedFocus), a)).toThrow(
      'focus ancestry',
    )
  })

  it('rejects collections beyond the backend 10,000-record limit', () => {
    const value = changedInventory((candidate) => {
      candidate.child_agents = Array.from({ length: 10_001 }, () => null)
    })
    expect(() => parseMachineInventory(response(value), a)).toThrow(
      'inventory.child_agents',
    )
  })

  it('rejects overlong identity and display strings and control characters', () => {
    const overlongIdentity = changedInventory((value) => {
      value.panes[0].runtime_id = 'p'.repeat(513)
    })
    const overlongDisplay = changedInventory((value) => {
      value.workspaces[0].label = 'x'.repeat(129)
    })
    const controlled = changedInventory((value) => {
      value.workers[0].name = 'agent\nspoofed'
    })
    const unicodeControlled = changedInventory((value) => {
      value.workers[0].name = 'agent\u0085spoofed'
    })
    expect(() => parseMachineInventory(response(overlongIdentity), a)).toThrow(
      'pane[0].runtime_id',
    )
    expect(() => parseMachineInventory(response(overlongDisplay), a)).toThrow(
      'workspace[0].label',
    )
    expect(() => parseMachineInventory(response(controlled), a)).toThrow(
      'worker[0].name',
    )
    expect(() => parseMachineInventory(response(unicodeControlled), a)).toThrow(
      'worker[0].name',
    )
  })

  it('rejects malformed and overflowing string-encoded counters', () => {
    for (const counter of [1, '-1', '01', '18446744073709551616']) {
      const value = changedInventory((candidate) => {
        candidate.workers[0].state_change_sequence = counter
      })
      expect(() => parseMachineInventory(response(value), a)).toThrow(
        'state_change_sequence',
      )
    }
    const revision = changedInventory((value) => {
      value.panes[0].revision = '1.0'
    })
    expect(() => parseMachineInventory(response(revision), a)).toThrow(
      'pane[0].revision',
    )
  })

  it('rejects duplicate topology identifiers', () => {
    const duplicatePane = changedInventory((value) => {
      value.panes.push({ ...value.panes[0] })
    })
    const duplicateWorker = changedInventory((value) => {
      value.workers.push({ ...value.workers[0] })
    })
    expect(() => parseMachineInventory(response(duplicatePane), a)).toThrow(
      'Duplicate pane identity',
    )
    expect(() => parseMachineInventory(response(duplicateWorker), a)).toThrow(
      'Duplicate worker identity',
    )
  })

  it('rejects malformed protocol, timestamp, and nested provider identity', () => {
    const protocol = changedInventory((value) => {
      value.protocol = 23
    })
    const timestamp = changedInventory((value) => {
      value.observed_at_unix_ms = Number.MAX_SAFE_INTEGER + 1
    })
    const provider = changedInventory((value) => {
      value.panes[0].provider_session = {
        source: 'herdr',
        provider: null,
        kind: 'session',
        value: 'provider-session',
      }
    })
    expect(() => parseMachineInventory(response(protocol), a)).toThrow(
      'inventory.protocol',
    )
    expect(() => parseMachineInventory(response(timestamp), a)).toThrow(
      'observed_at_unix_ms',
    )
    expect(() => parseMachineInventory(response(provider), a)).toThrow(
      'provider_session.provider',
    )
  })

  it('keeps duplicate topology ids separate by endpoint', () => {
    expect(
      new Set(
        [local, machine(a), machine(b)].map(
          (candidate) => `${endpointKey(candidate.endpoint)}:w1:p1`,
        ),
      ).size,
    ).toBe(3)
  })
})

describe('command construction', () => {
  it('quotes spaces, quotes, and option-looking inputs', () => {
    expect(
      buildAddMachineCommand({
        sshTarget: '-host name',
        label: "Builder's box",
        remoteSession: 'release one',
      }),
    ).toBe(
      `herdr machine add --label='Builder'"'"'s box' --remote-session='release one' -- '-host name'`,
    )
    expect(
      buildAddMachineCommand({
        sshTarget: 'user@host',
        label: '',
        remoteSession: '',
      }),
    ).toBe("herdr machine add -- 'user@host'")
  })

  it('rejects newline and NUL injection', () => {
    expect(() =>
      buildAddMachineCommand({
        sshTarget: 'host\ncommand',
        label: '',
        remoteSession: '',
      }),
    ).toThrow('newlines')
    expect(() =>
      buildAddMachineCommand({
        sshTarget: 'host',
        label: 'bad\0label',
        remoteSession: '',
      }),
    ).toThrow('NUL')
    expect(() =>
      buildAddMachineCommand({
        sshTarget: 'host',
        label: '',
        remoteSession: 'bad\rname',
      }),
    ).toThrow('newlines')
  })

  it('reconnects only by validated stable id', () => {
    expect(buildReconnectCommand(a)).toBe(
      `herdr machine reconnect '${a}'`,
    )
    expect(() => buildReconnectCommand('--label')).toThrow(
      'validated stable machine ID',
    )
  })
})

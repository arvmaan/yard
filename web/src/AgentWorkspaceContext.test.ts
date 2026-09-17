import { describe, expect, it } from 'vitest'
import {
  DEFAULT_TERMINAL_PRESENTATION,
  isTerminalWorkspaceMode,
} from './AgentWorkspaceContext'
import {
  resolveRuntimeCapabilities,
  runtimeCapabilityDetail,
  runtimeCapabilityLabel,
} from './runtimeCapabilities'
import type {
  ObservedWorker,
  PaneObservation,
  RuntimeInventory,
  WorkerRuntimeBinding,
} from './types'

function runtime(
  overrides: Partial<WorkerRuntimeBinding> = {},
): WorkerRuntimeBinding {
  return {
    adapter: 'herdr',
    session: 'default',
    workspace_id: 'workspace-1',
    terminal_id: 'terminal-1',
    tab_id: 'tab-1',
    pane_id: 'pane-1',
    provider_session: {
      source: 'herdr:codex',
      provider: 'codex',
      kind: 'id',
      value: 'session-1',
    },
    owns_tab: false,
    observation_state: 'observed',
    process_state: 'running',
    status: 'working',
    state_change_sequence: '1',
    revision: '1',
    version: '1',
    last_observed_at_unix_ms: 1,
    ...overrides,
  }
}

function observed(
  overrides: Partial<ObservedWorker> = {},
): ObservedWorker {
  return {
    runtime_id: 'terminal-1',
    terminal_id: 'terminal-1',
    workspace_id: 'workspace-1',
    tab_id: 'tab-1',
    pane_id: 'pane-1',
    name: 'worker',
    provider: 'codex',
    display_provider: 'Codex',
    status: 'working',
    focused: false,
    launch_pending: false,
    interactive_ready: false,
    state_change_sequence: '1',
    cwd: '/tmp/project',
    foreground_cwd: '/tmp/project',
    tokens: {},
    provider_session: {
      source: 'herdr:codex',
      provider: 'codex',
      kind: 'id',
      value: 'session-1',
    },
    revision: '1',
    ...overrides,
  }
}

function pane(
  overrides: Partial<PaneObservation> = {},
): PaneObservation {
  return {
    runtime_id: 'pane-1',
    terminal_id: 'terminal-1',
    workspace_id: 'workspace-1',
    tab_id: 'tab-1',
    focused: false,
    cwd: '/tmp/project',
    foreground_cwd: '/tmp/project',
    label: 'shell',
    provider: null,
    display_provider: null,
    status: 'working',
    tokens: {},
    provider_session: null,
    revision: '1',
    ...overrides,
  }
}

function inventory(
  options: {
    panes?: PaneObservation[]
    workers?: ObservedWorker[]
  } = {},
): RuntimeInventory {
  return {
    adapter: 'herdr',
    session: 'default',
    runtime_version: '0.9.0',
    protocol: 19,
    observed_at_unix_ms: 1,
    focus: {
      workspace_id: 'workspace-1',
      tab_id: 'tab-1',
      pane_id: 'pane-1',
    },
    workspaces: [],
    tabs: [],
    panes: options.panes ?? [pane()],
    workers: options.workers ?? [observed()],
    child_agents: [],
  }
}

describe('agent workspace terminal presentation', () => {
  it('defaults single-target terminal work to the full terminal presentation', () => {
    expect(DEFAULT_TERMINAL_PRESENTATION).toBe('terminal')
  })

  it('treats coherent worker-and-pane evidence as ready even when interactive_ready is false', () => {
    const binding = runtime()
    const capabilities = resolveRuntimeCapabilities(
      true,
      binding,
      inventory({
        panes: [pane({ provider_session: binding.provider_session })],
        workers: [observed()],
      }),
    )
    expect(capabilities).toMatchObject({
      chat: true,
      reason: 'ready',
      terminal: true,
    })
    expect(capabilities.observedWorker).toBeTruthy()
    expect(capabilities.observedPane).toBeTruthy()
  })

  it('disables an exact worker when a same-terminal pane contradicts it', () => {
    expect(
      resolveRuntimeCapabilities(
        true,
        runtime(),
        inventory({ panes: [pane({ runtime_id: 'pane-2' })] }),
      ),
    ).toMatchObject({
      chat: false,
      reason: 'ambiguous',
      terminal: false,
    })
  })

  it('disables an exact pane when a same-terminal worker contradicts it', () => {
    expect(
      resolveRuntimeCapabilities(
        true,
        runtime({ provider_session: null }),
        inventory({
          panes: [pane()],
          workers: [observed({ pane_id: 'pane-2', provider_session: null })],
        }),
      ),
    ).toMatchObject({
      chat: false,
      reason: 'ambiguous',
      terminal: false,
    })
  })

  it('keeps terminal-only access for a topology-only shell pane', () => {
    expect(
      resolveRuntimeCapabilities(
        true,
        runtime({ provider_session: null }),
        inventory({ panes: [pane()], workers: [] }),
      ),
    ).toMatchObject({
      chat: false,
      reason: 'no_foreground_agent',
      terminal: true,
    })
  })

  it('disables both capabilities for launch-pending or stale observations', () => {
    expect(
      resolveRuntimeCapabilities(
        true,
        runtime(),
        inventory({ workers: [observed({ launch_pending: true })] }),
      ),
    ).toMatchObject({
      chat: false,
      reason: 'launch_pending',
      terminal: false,
    })
    expect(
      resolveRuntimeCapabilities(false, runtime(), inventory()),
    ).toMatchObject({
      chat: false,
      reason: 'stale',
      terminal: false,
    })
  })

  it('distinguishes a fresh missing binding from a stale inventory envelope', () => {
    const freshMissing = resolveRuntimeCapabilities(
      true,
      runtime(),
      inventory({ workers: [], panes: [] }),
    )
    expect(freshMissing).toMatchObject({
      chat: false,
      reason: 'binding_missing',
      terminal: false,
    })
    expect(runtimeCapabilityLabel(freshMissing)).toBe('Binding missing')
    expect(runtimeCapabilityDetail(freshMissing)).toBe(
      'Worker not found in latest Herdr snapshot.',
    )

    const stale = resolveRuntimeCapabilities(
      false,
      runtime(),
      inventory({ workers: [], panes: [] }),
    )
    expect(stale).toMatchObject({
      chat: false,
      reason: 'stale',
      terminal: false,
    })
    expect(runtimeCapabilityLabel(stale)).toBe('Connection status stale')
  })

  it('disables capabilities for binding-missing, ambiguous, or identity-mismatched evidence', () => {
    expect(
      resolveRuntimeCapabilities(true, runtime(), inventory({ workers: [], panes: [] })),
    ).toMatchObject({
      chat: false,
      reason: 'binding_missing',
      terminal: false,
    })
    expect(
      resolveRuntimeCapabilities(
        true,
        runtime(),
        inventory({
          workers: [observed(), observed({ pane_id: 'pane-2', runtime_id: 'terminal-dup' })],
        }),
      ),
    ).toMatchObject({
      chat: false,
      reason: 'ambiguous',
      terminal: false,
    })
    expect(
      resolveRuntimeCapabilities(
        true,
        runtime(),
        inventory({ workers: [observed({ pane_id: 'pane-other' })], panes: [] }),
      ),
    ).toMatchObject({
      chat: false,
      reason: 'identity_mismatch',
      terminal: false,
    })
  })

  it('keeps presentation changes inside terminal workspace mode', () => {
    expect(isTerminalWorkspaceMode('terminal')).toBe(true)
    expect(isTerminalWorkspaceMode('chat')).toBe(false)
    expect(isTerminalWorkspaceMode('map')).toBe(false)
  })
})

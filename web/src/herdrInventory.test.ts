import { describe, expect, it } from 'vitest'
import {
  clearHerdrInventoryFailures,
  countHerdrInventory,
  dedupeHerdrPanes,
  filterHerdrFailures,
  filterHerdrInventory,
  herdrInventoryStatus,
  herdrPaneAccessibleDescription,
  herdrPaneAccessibleName,
  herdrPaneLabel,
  herdrRequestFailureStatus,
  mergeHerdrInventorySnapshot,
  safeHerdrFailureReason,
  safeHerdrPaneMetadataReason,
  safeHerdrPaneReason,
  safeHerdrRequestError,
} from './herdrInventory'
import type { HerdrFleetInventory, HerdrLivePane } from './types'

function pane(overrides: Partial<HerdrLivePane> = {}): HerdrLivePane {
  return {
    identity_key: 'identity-default',
    kind: 'agent',
    observation: 'observed',
    reason: 'Live agent observed.',
    metadata_reason: null,
    has_live_pane: true,
    session: 'default',
    workspace_id: 'wN',
    workspace_label: 'Yard',
    tab_id: 'wN:t1',
    pane_id: 'wN:p2',
    terminal_id: 'wN:p2',
    label: null,
    name: null,
    provider: 'codex',
    display_provider: 'Codex',
    status: 'working',
    cwd: '/work/yard',
    foreground_cwd: '/work/yard',
    ...overrides,
  }
}

function inventory(panes: HerdrLivePane[]): HerdrFleetInventory {
  return {
    adapter: 'herdr',
    live_pane_count: panes.length,
    sessions: [{
      id: 'default',
      name: 'default',
      is_default: true,
      metadata_ambiguous: false,
      metadata_reason: null,
      observed_at_unix_ms: '1',
      pane_count: panes.length,
      panes,
    }],
    failures: [],
  }
}

describe('Herdr inventory projection', () => {
  it('groups 500+ panes and filters without collapsing workspaces', () => {
    const panes = Array.from({ length: 501 }, (_, index) =>
      pane({
        identity_key: `identity-${index}`,
        kind: index % 2 ? 'runtime' : 'agent',
        workspace_id: `workspace-${index % 5}`,
        workspace_label: `Workspace ${index % 5}`,
        pane_id: `pane-${index}`,
        terminal_id: `terminal-${index}`,
        name: index === 2 ? 'Needle agent' : null,
      }),
    )
    const groups = filterHerdrInventory(inventory(panes), '')
    expect(groups).toHaveLength(1)
    expect(groups[0].panes).toBe(501)
    expect(groups[0].workspaces).toHaveLength(5)
    expect(filterHerdrInventory(inventory(panes), 'Needle agent')[0].panes).toBe(1)
  })

  it('labels an unnamed live agent with its terminal identity', () => {
    expect(herdrPaneLabel(pane())).toBe('Unnamed agent · wN:p2')
  })

  it('builds concise accessible row text with status, stale state, and reason', () => {
    const unattached = pane({
      has_live_pane: false,
      observation: 'ambiguous',
      reason: 'No live pane observed.',
      metadata_reason: 'Conflicting or incomplete metadata: title, process status.',
      workspace_id: null,
      workspace_label: null,
      status: 'blocked',
      name: 'Detached Codex',
    })

    expect(
      herdrPaneAccessibleName(unattached, 'default', 'Ambiguous location'),
    ).toBe('Unattached agent · Detached Codex, default, Ambiguous location')
    expect(herdrPaneAccessibleDescription(unattached, false)).toBe(
      'blocked No live pane observed. Conflicting or incomplete metadata: title, process status.',
    )
    expect(herdrPaneAccessibleDescription(unattached, true)).toBe(
      'blocked Stale snapshot; last observed. No live pane observed. Conflicting or incomplete metadata: title, process status.',
    )
  })

  it('retains only failed sessions as stale context and keeps duplicate terminal ids distinct', () => {
    const prior = inventory([
      pane({ identity_key: 'alpha-one', terminal_id: 'shared' }),
    ])
    prior.sessions.push({
      id: 'beta',
      name: 'beta',
      is_default: false,
      metadata_ambiguous: false,
      metadata_reason: null,
      observed_at_unix_ms: '1',
      pane_count: 1,
      panes: [
        pane({
          identity_key: 'beta-one',
          session: 'beta',
          terminal_id: 'shared',
        }),
      ],
    })
    prior.live_pane_count = 2
    const next = inventory([
      pane({ identity_key: 'alpha-two', terminal_id: 'shared' }),
    ])
    next.failures = [
      { session: 'beta', code: 'herdr_snapshot_failed', reason: 'raw detail' },
    ]

    const merged = mergeHerdrInventorySnapshot(prior, next)
    expect(merged.staleSessions).toEqual(['beta'])
    expect(merged.inventory.sessions.flatMap(({ panes }) => panes)).toHaveLength(2)
    expect(
      new Set(
        merged.inventory.sessions.flatMap(({ panes }) =>
          panes.map(({ identity_key }) => identity_key),
        ),
      ).size,
    ).toBe(2)
  })


  it('clears prior per-session failures when the latest refresh fails totally', () => {
    const previous = inventory([pane({ identity_key: 'alpha' })])
    previous.failures = [
      { session: 'beta', code: 'herdr_snapshot_failed', reason: 'old' },
    ]

    const retained = clearHerdrInventoryFailures(previous)

    expect(retained?.sessions).toEqual(previous.sessions)
    expect(retained?.failures).toEqual([])
  })

  it('clears a session failure as soon as that session succeeds', () => {
    const previous = inventory([pane({ identity_key: 'alpha' })])
    previous.sessions.push({
      id: 'beta',
      name: 'beta',
      is_default: false,
      metadata_ambiguous: false,
      metadata_reason: null,
      observed_at_unix_ms: '1',
      pane_count: 1,
      panes: [pane({ identity_key: 'beta-stale', session: 'beta' })],
    })
    previous.failures = [
      { session: 'beta', code: 'herdr_snapshot_failed', reason: 'old' },
    ]
    const next = inventory([pane({ identity_key: 'alpha-fresh' })])
    next.sessions.push({
      id: 'beta',
      name: 'beta',
      is_default: false,
      metadata_ambiguous: false,
      metadata_reason: null,
      observed_at_unix_ms: '2',
      pane_count: 1,
      panes: [pane({ identity_key: 'beta-fresh', session: 'beta' })],
    })
    next.failures = [
      { session: 'alpha', code: 'herdr_snapshot_failed', reason: 'current' },
    ]

    const merged = mergeHerdrInventorySnapshot(previous, next)
    expect(merged.inventory.failures.map(({ session }) => session)).toEqual([
      'alpha',
    ])
    expect(merged.staleSessions).toEqual([])
    expect(merged.inventory.sessions.find(({ name }) => name === 'beta')?.panes[0].identity_key).toBe('beta-fresh')
  })

  it('keeps stable selection keys across reorder and mutable metadata changes', () => {
    const original = pane({ identity_key: 'stable-runtime' })
    const changed = pane({
      identity_key: 'stable-runtime',
      display_provider: 'Claude CLI',
      provider: 'claude',
      label: 'Renamed pane',
      name: 'Renamed agent',
      status: 'blocked',
    })
    const other = pane({
      identity_key: 'other-runtime',
      pane_id: 'wN:p3',
      terminal_id: 'wN:p3',
    })

    expect(
      filterHerdrInventory(inventory([original, other]), '')
        .flatMap(({ workspaces }) => workspaces)
        .flatMap(({ panes }) => panes)
        .map(({ identity_key }) => identity_key),
    ).toEqual(['stable-runtime', 'other-runtime'])
    expect(
      filterHerdrInventory(inventory([other, changed]), '')
        .flatMap(({ workspaces }) => workspaces)
        .flatMap(({ panes }) => panes)
        .map(({ identity_key }) => identity_key),
    ).toEqual(['other-runtime', 'stable-runtime'])
    expect(changed.identity_key).toBe(original.identity_key)
    expect(dedupeHerdrPanes([changed, original])).toHaveLength(1)
    expect(dedupeHerdrPanes([original, changed])).toEqual(
      dedupeHerdrPanes([changed, original]),
    )
  })

  it('counts fresh, filtered, agent, runtime, stale, and failed sessions', () => {
    const current = inventory([
      pane({ identity_key: 'agent' }),
      pane({
        identity_key: 'runtime',
        kind: 'runtime',
        name: null,
        provider: null,
        terminal_id: 'runtime-terminal',
      }),
    ])
    current.sessions.push({
      id: 'beta',
      name: 'beta',
      is_default: false,
      metadata_ambiguous: false,
      metadata_reason: null,
      observed_at_unix_ms: '1',
      pane_count: 1,
      panes: [pane({ identity_key: 'stale', session: 'beta' })],
    })
    current.failures = [
      { session: 'beta', code: 'herdr_snapshot_failed', reason: 'hidden' },
    ]
    const groups = filterHerdrInventory(current, 'runtime-terminal')
    expect(
      countHerdrInventory(current, groups, new Set(['beta']), false),
    ).toEqual({
      freshTotal: 2,
      visibleTotal: 1,
      visibleAgents: 0,
      visibleRuntime: 1,
      visibleUnattached: 0,
      staleTotal: 1,
      failedSessions: 1,
    })
  })

  it('searches the visible classification, status, and sanitized reason text', () => {
    const ambiguous = pane({
      kind: 'agent',
      observation: 'ambiguous',
      reason: 'Partial agent evidence.',
      status: 'unknown',
      provider: null,
      display_provider: null,
    })
    const unsafe = pane({
      identity_key: 'unsafe',
      kind: 'runtime',
      observation: 'ambiguous',
      reason: '/private/provider/session',
      status: 'blocked',
    })
    const current = inventory([ambiguous, unsafe])

    expect(filterHerdrInventory(current, 'agent pane')[0].panes).toBe(1)
    expect(filterHerdrInventory(current, 'Status not reported')[0].panes).toBe(1)
    expect(filterHerdrInventory(current, 'Partial agent evidence')[0].panes).toBe(1)
    expect(filterHerdrInventory(current, 'blocked')[0].panes).toBe(1)
    expect(filterHerdrInventory(current, '/private/provider/session')).toEqual([])
    expect(filterHerdrInventory(current, 'Live identity evidence conflicts')[0].panes).toBe(1)
  })

  it('keeps conflicting tab identity optional and bounded', () => {
    const conflicting = pane({
      tab_id: null,
      observation: 'ambiguous',
      reason: 'Conflicting tab identity.',
      provider: null,
      display_provider: null,
      name: null,
      status: 'unknown',
    })

    expect(filterHerdrInventory(inventory([conflicting]), 'null')).toEqual([])
    expect(safeHerdrPaneReason(conflicting)).toBe('Conflicting tab identity.')
  })

  it('keeps duplicate display names separate by immutable session id', () => {
    const current = inventory([pane({ identity_key: 'alpha', session: 'session-alpha' })])
    current.sessions[0] = {
      ...current.sessions[0],
      id: 'session-alpha',
      name: 'shared',
    }
    current.sessions.push({
      id: 'session-beta',
      name: 'shared',
      is_default: false,
      metadata_ambiguous: false,
      metadata_reason: null,
      observed_at_unix_ms: '1',
      pane_count: 1,
      panes: [pane({ identity_key: 'beta', session: 'session-beta' })],
    })
    current.live_pane_count = 2

    const groups = filterHerdrInventory(current, '')
    expect(groups.map(({ id, label }) => [id, label])).toEqual([
      ['session-alpha', 'shared · session-alpha'],
      ['session-beta', 'shared · session-beta'],
    ])
    expect(filterHerdrInventory(current, 'session-beta').map(({ id }) => id)).toEqual([
      'session-beta',
    ])
  })

  it('shows unattached agent evidence without inflating pane counts', () => {
    const unattached = pane({
      identity_key: 'orphan',
      has_live_pane: false,
      observation: 'ambiguous',
      reason: 'No live pane observed.',
      terminal_id: null,
      pane_id: 'claimed-pane',
      name: null,
      label: null,
      provider: null,
      display_provider: null,
      status: 'unknown',
    })
    const current = inventory([pane(), unattached])
    current.live_pane_count = 1
    current.sessions[0].pane_count = 1
    const groups = filterHerdrInventory(current, '')

    expect(groups[0].panes).toBe(1)
    expect(groups[0].unattached).toBe(1)
    expect(herdrPaneLabel(unattached)).toBe('Unattached agent')
    expect(countHerdrInventory(current, groups, new Set(), false)).toEqual({
      freshTotal: 1,
      visibleTotal: 1,
      visibleAgents: 1,
      visibleRuntime: 0,
      visibleUnattached: 1,
      staleTotal: 0,
      failedSessions: 0,
    })
  })

  it('searches every visible classification, status, stale label, and bounded reason', () => {
    const current = inventory([
      pane({
        observation: 'ambiguous',
        reason: 'No live pane observed.',
        metadata_reason: 'Conflicting or incomplete metadata: title, process status.',
        has_live_pane: false,
        workspace_id: null,
        workspace_label: null,
        status: 'unknown',
      }),
      pane({
        identity_key: 'runtime',
        kind: 'runtime',
        reason: 'Live runtime pane observed.',
        name: null,
        provider: null,
        display_provider: null,
        status: 'working',
      }),
      pane({ identity_key: 'blocked', status: 'blocked' }),
      pane({ identity_key: 'idle', status: 'idle' }),
    ])
    const stale = new Set(['default'])
    for (const term of [
      'default',
      'stale',
      'Last observed',
      'Last observed details',
      'Refresh for current information',
      'Unattached agent',
      'Ambiguous location',
      'Runtime pane',
      'Agent evidence',
      'No live pane observed',
      'process status',
      'working',
      'blocked',
      'idle',
    ]) {
      expect(filterHerdrInventory(current, term, stale)).not.toEqual([])
    }
    expect(safeHerdrPaneMetadataReason(current.sessions[0].panes[0])).toBe(
      'Conflicting or incomplete metadata: title, process status.',
    )
  })

  it('searches sanitized session failure text without changing pane counts', () => {
    const current = inventory([pane()])
    current.failures = [{
      session: 'default',
      code: 'herdr_snapshot_failed',
      reason: '/private/herdr.sock',
    }]

    const groups = filterHerdrInventory(
      current,
      'Herdr snapshot was unavailable for this session',
    )
    expect(groups[0].panes).toBe(1)
    expect(filterHerdrFailures(current, 'snapshot was unavailable')).toHaveLength(1)
    expect(filterHerdrFailures(current, '/private/herdr.sock')).toEqual([])
    expect(countHerdrInventory(current, groups, new Set(), false).visibleTotal).toBe(1)
  })

  it('announces singular and plural inventory counts precisely', () => {
    const counts = {
      freshTotal: 1,
      visibleTotal: 1,
      visibleAgents: 1,
      visibleRuntime: 0,
      visibleUnattached: 0,
      staleTotal: 0,
      failedSessions: 0,
    }
    expect(herdrInventoryStatus(counts)).toBe(
      'Showing 1 result: 1 of 1 Herdr pane; 0 session failures.',
    )
    expect(
      herdrInventoryStatus({
        ...counts,
        freshTotal: 0,
        visibleTotal: 0,
        visibleAgents: 0,
        visibleUnattached: 1,
      }),
    ).toBe(
      'Showing 1 result: 0 of 0 Herdr panes; 1 unattached agent; 0 session failures.',
    )
    expect(
      herdrInventoryStatus({
        ...counts,
        freshTotal: 2,
        visibleTotal: 2,
        failedSessions: 1,
      }),
    ).toBe('Showing 3 results: 2 of 2 Herdr panes; 1 session failure.')
    expect(herdrRequestFailureStatus(1, 1)).toBe(
      '1 of 1 Herdr session snapshot failed.',
    )
    expect(herdrRequestFailureStatus(2, 2)).toBe(
      '2 of 2 Herdr session snapshots failed.',
    )
  })

  it('never renders raw backend reason strings', () => {
    expect(safeHerdrRequestError('unexpected')).not.toContain('unexpected')
    expect(safeHerdrFailureReason('unexpected')).not.toContain('unexpected')
    expect(safeHerdrFailureReason('session_metadata_ambiguous')).toBe(
      'Herdr reported conflicting session metadata.',
    )
    expect(
      safeHerdrPaneReason(
        pane({ observation: 'ambiguous', reason: '/private/socket/path' }),
      ),
    ).toBe('Live identity evidence conflicts.')
    for (const reason of [
      'Missing workspace ancestry.',
      'Missing tab ancestry.',
      'Missing workspace and tab ancestry.',
      'Conflicting pane ancestry.',
    ]) {
      expect(safeHerdrPaneReason(pane({ observation: 'ambiguous', reason }))).toBe(
        reason,
      )
    }
  })

})

import { describe, expect, it } from 'vitest'
import {
  beginProjectTransferRefresh,
  emptyProjectTransferContext,
  failProjectTransferRefresh,
  resolveProjectTransferRefresh,
  type ProjectTransferSnapshot,
} from './projectTransferContext'

function snapshot(observedAt: number): ProjectTransferSnapshot {
  return {
    candidates: [],
    inventory: {
      adapter: 'herdr',
      session: 'alpha',
      runtime_version: '1',
      protocol: 1,
      observed_at_unix_ms: observedAt,
      focus: {
        workspace_id: null,
        tab_id: null,
        pane_id: null,
      },
      workspaces: [],
      tabs: [],
      panes: [],
      workers: [],
      child_agents: [],
    },
    project: {} as ProjectTransferSnapshot['project'],
  }
}

describe('project transfer context refresh state', () => {
  it('ignores a late response from an older generation', () => {
    let entry = beginProjectTransferRefresh(
      emptyProjectTransferContext(),
      1,
    )
    entry = beginProjectTransferRefresh(entry, 2)
    entry = resolveProjectTransferRefresh(
      entry,
      2,
      snapshot(200),
    ).entry

    const late = resolveProjectTransferRefresh(
      entry,
      1,
      snapshot(100),
    )
    expect(late.entry).toBe(entry)
    expect(late.snapshot).toBeNull()
    expect(late.entry.snapshot?.inventory.observed_at_unix_ms).toBe(200)
  })

  it('does not regress a newer observed inventory timestamp', () => {
    let entry = beginProjectTransferRefresh(
      emptyProjectTransferContext(),
      1,
    )
    entry = resolveProjectTransferRefresh(
      entry,
      1,
      snapshot(300),
    ).entry
    entry = beginProjectTransferRefresh(entry, 2)

    const resolved = resolveProjectTransferRefresh(
      entry,
      2,
      snapshot(250),
    )
    expect(resolved.entry.loading).toBe(false)
    expect(resolved.entry.error).toContain('is older')
    expect(resolved.entry.snapshot?.inventory.observed_at_unix_ms).toBe(300)
    expect(resolved.snapshot).toBeNull()
  })

  it('keeps a failed snapshot non-current while a retry is in flight', () => {
    let entry = beginProjectTransferRefresh(
      emptyProjectTransferContext(),
      1,
    )
    entry = resolveProjectTransferRefresh(
      entry,
      1,
      snapshot(300),
    ).entry
    entry = beginProjectTransferRefresh(entry, 2)
    entry = failProjectTransferRefresh(entry, 2, 'refresh failed')
    entry = beginProjectTransferRefresh(entry, 3)

    expect(entry).toMatchObject({
      error: 'refresh failed',
      generation: 3,
      loading: true,
    })
    expect(entry.snapshot?.inventory.observed_at_unix_ms).toBe(300)
  })

  it('accepts an equal observation after rejecting a lower timestamp', () => {
    let entry = beginProjectTransferRefresh(
      emptyProjectTransferContext(),
      1,
    )
    entry = resolveProjectTransferRefresh(
      entry,
      1,
      snapshot(300),
    ).entry
    entry = beginProjectTransferRefresh(entry, 2)
    entry = resolveProjectTransferRefresh(
      entry,
      2,
      snapshot(250),
    ).entry
    expect(entry.error).toContain('older')

    entry = beginProjectTransferRefresh(entry, 3)
    const resolved = resolveProjectTransferRefresh(
      entry,
      3,
      snapshot(300),
    )
    expect(resolved.entry).toMatchObject({
      error: null,
      loading: false,
    })
    expect(resolved.snapshot?.inventory.observed_at_unix_ms).toBe(300)
  })

  it('clears loading on current abort and preserves a newer reopen', () => {
    let entry = beginProjectTransferRefresh(
      emptyProjectTransferContext(),
      1,
    )
    entry = failProjectTransferRefresh(entry, 1, null)
    expect(entry).toMatchObject({
      error: null,
      loading: false,
      snapshot: null,
    })

    entry = beginProjectTransferRefresh(entry, 2)
    const staleAbort = failProjectTransferRefresh(entry, 1, null)
    expect(staleAbort).toBe(entry)
    expect(staleAbort).toMatchObject({ generation: 2, loading: true })
  })
})

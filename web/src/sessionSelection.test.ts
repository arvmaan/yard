import { describe, expect, it } from 'vitest'
import {
  countSessionRoles,
  pickDefaultSession,
  resolveSelectedSession,
  sessionOptionLabel,
} from './sessionSelection'
import type { RuntimeSession } from './types'

const session = (
  name: string,
  overrides: Partial<RuntimeSession> = {},
): RuntimeSession => ({ name, is_default: false, running: true, ...overrides })

const project = (session: string) => ({
  runtime: { adapter: 'herdr', session, workspace_id: 'w' },
})

const workstream = (session: string | null) =>
  ({
    kind: 'workstream',
    worker: session ? { runtime: { session } } : null,
  }) as unknown as Parameters<typeof countSessionRoles>[1][number]

const sessions = [
  session('default', { is_default: true }),
  session('yard-coordination'),
  session('yard-orchestrator'),
]

const counts = countSessionRoles(
  [project('yard-orchestrator'), project('yard-orchestrator'), project('x')],
  [workstream('yard-coordination'), workstream('yard-coordination'), workstream(null)],
)

describe('countSessionRoles', () => {
  it('counts projects and workstreams per session', () => {
    expect(counts.get('yard-orchestrator')).toEqual({ projects: 2, workstreams: 0 })
    expect(counts.get('yard-coordination')).toEqual({ projects: 0, workstreams: 2 })
    expect(counts.get('x')).toEqual({ projects: 1, workstreams: 0 })
  })

  it('ignores non-workstream nodes', () => {
    const result = countSessionRoles([], [
      { kind: 'knowledge_store', worker: { runtime: { session: 'a' } } } as never,
    ])
    expect(result.size).toBe(0)
  })
})

describe('pickDefaultSession', () => {
  it('prefers the running session with the most projects', () => {
    expect(pickDefaultSession(sessions, counts)).toBe('yard-orchestrator')
  })

  it('skips stopped sessions even when they hold projects', () => {
    expect(
      pickDefaultSession(
        [session('default', { is_default: true }), session('yard-orchestrator', { running: false })],
        counts,
      ),
    ).toBe('default')
  })

  it('falls back to Herdr default, then first running, then empty', () => {
    expect(pickDefaultSession(sessions, new Map())).toBe('default')
    expect(
      pickDefaultSession([session('a', { running: false }), session('b'), session('c')], new Map()),
    ).toBe('b')
    expect(pickDefaultSession([session('a', { running: false })], new Map())).toBe('')
    expect(pickDefaultSession([], new Map())).toBe('')
  })
})

describe('resolveSelectedSession', () => {
  const base = { sessions, counts, current: '', explicit: null, settled: false }

  it('honors a running explicit choice', () => {
    expect(resolveSelectedSession({ ...base, explicit: 'default' })).toBe('default')
    expect(
      resolveSelectedSession({ ...base, explicit: 'default', current: 'yard-orchestrator', settled: true }),
    ).toBe('default')
  })

  it('ignores an explicit choice that is not running', () => {
    expect(resolveSelectedSession({ ...base, explicit: 'gone' })).toBe('yard-orchestrator')
  })

  it('keeps a settled running selection instead of recomputing', () => {
    expect(
      resolveSelectedSession({ ...base, current: 'yard-coordination', settled: true }),
    ).toBe('yard-coordination')
  })

  it('recomputes an unsettled or stopped selection', () => {
    expect(resolveSelectedSession({ ...base, current: 'default' })).toBe('yard-orchestrator')
    expect(
      resolveSelectedSession({ ...base, current: 'missing', settled: true }),
    ).toBe('yard-orchestrator')
  })
})

describe('sessionOptionLabel', () => {
  it('adds role hints and a stopped marker', () => {
    expect(sessionOptionLabel(session('yard-orchestrator'), counts)).toBe(
      'yard-orchestrator — 2 projects',
    )
    expect(sessionOptionLabel(session('yard-coordination'), counts)).toBe(
      'yard-coordination — 2 workstreams',
    )
    expect(sessionOptionLabel(session('default', { is_default: true }), counts)).toBe(
      'default — Herdr default',
    )
    expect(sessionOptionLabel(session('x', { running: false }), counts)).toBe(
      'x — 1 project (stopped)',
    )
    expect(sessionOptionLabel(session('plain'), counts)).toBe('plain')
  })
})

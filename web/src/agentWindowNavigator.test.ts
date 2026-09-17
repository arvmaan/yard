import { describe, expect, it } from 'vitest'
import {
  filterAndSortAgentWindowGroups,
  groupAgentWindowTargets,
  type FilterableAgentWindowTarget,
} from './agentWindowNavigator'
import type { WorkspaceObservation } from './types'

function target(
  key: string,
  workspaceId: string,
  overrides: Partial<FilterableAgentWindowTarget> = {},
): FilterableAgentWindowTarget {
  return {
    contextLabel: 'Project',
    cwd: `/work/${workspaceId}`,
    harness: 'Codex',
    key,
    label: key,
    observation: 'observed',
    paneId: `${workspaceId}:pane-1`,
    role: 'worker',
    roleLabel: 'Implementer',
    runtimeAdapter: 'herdr',
    session: 'alpha',
    status: 'idle',
    tabId: `${workspaceId}:tab-1`,
    terminalId: `${workspaceId}:terminal-1`,
    workspaceId,
    ...overrides,
  }
}

function workspace(
  runtimeId: string,
  order: number,
  label: string,
): WorkspaceObservation {
  return {
    active_tab_id: `${runtimeId}:tab-1`,
    focused: false,
    label,
    order,
    pane_count: 1,
    runtime_id: runtimeId,
    status: 'idle',
    tab_count: 1,
    tokens: {},
    worktree: null,
  }
}

const sessions = [
  { is_default: true, name: 'alpha', running: true },
  { is_default: false, name: 'beta', running: false },
]

describe('groupAgentWindowTargets', () => {
  it('uses observed workspace order before deterministic label and id ties', () => {
    const groups = groupAgentWindowTargets(
      [
        target('worker-c', 'workspace-c'),
        target('worker-b', 'workspace-b'),
        target('worker-a', 'workspace-a'),
      ],
      {
        adapter: 'herdr',
        session: 'alpha',
        workspaces: [
          workspace('workspace-a', 2, 'Alpha'),
          workspace('workspace-b', 1, 'Zulu'),
          workspace('workspace-c', 1, 'Alpha'),
        ],
      },
      sessions,
    )

    expect(groups.map((group) => group.workspaceId)).toEqual([
      'workspace-c',
      'workspace-b',
      'workspace-a',
    ])
  })

  it('keeps missing observations in namespaced durable fallback groups', () => {
    const groups = groupAgentWindowTargets(
      [
        target('alpha-worker', 'shared'),
        target('beta-worker', 'shared', { session: 'beta' }),
      ],
      {
        adapter: 'herdr',
        session: 'alpha',
        workspaces: [],
      },
      sessions,
    )

    expect(groups).toMatchObject([
      {
        observation: null,
        session: 'alpha',
        sessionRunning: true,
        workspaceId: 'shared',
      },
      {
        observation: null,
        session: 'beta',
        sessionRunning: false,
        workspaceId: 'shared',
      },
    ])
  })

  it('orders stable roles and labels without using mutable runtime state', () => {
    const groups = groupAgentWindowTargets(
      [
        target('worker-z', 'workspace-1', {
          contextLabel: 'Zulu',
          label: 'Builder',
        }),
        target('worker-a', 'workspace-1', {
          contextLabel: 'Alpha',
          label: 'Reviewer',
        }),
        target('orchestrator', 'workspace-1', {
          label: 'Project orchestrator',
          role: 'orchestrator',
        }),
        target('superintendent', 'workspace-1', {
          label: 'Superintendent',
          role: 'superintendent',
        }),
      ],
      null,
      sessions,
    )

    expect(groups[0].targets.map((candidate) => candidate.key)).toEqual([
      'superintendent',
      'orchestrator',
      'worker-a',
      'worker-z',
    ])
  })

  it('places every unique target in exactly one group', () => {
    const repeated = target('worker-a', 'workspace-a')
    const groups = groupAgentWindowTargets(
      [
        repeated,
        target('worker-b', 'workspace-b'),
        repeated,
        target('worker-c', 'workspace-a'),
      ],
      null,
      sessions,
    )
    const flattenedKeys = groups.flatMap((group) =>
      group.targets.map((candidate) => candidate.key),
    )

    expect(flattenedKeys).toEqual([
      'worker-a',
      'worker-c',
      'worker-b',
    ])
    expect(new Set(flattenedKeys).size).toBe(flattenedKeys.length)
  })

  it('namespaces workspace identity by runtime adapter', () => {
    const groups = groupAgentWindowTargets(
      [
        target('herdr-worker', 'shared'),
        target('other-worker', 'shared', {
          runtimeAdapter: 'other',
        }),
      ],
      {
        adapter: 'herdr',
        session: 'alpha',
        workspaces: [workspace('shared', 1, 'Observed Herdr workspace')],
      },
      sessions,
    )

    expect(groups).toMatchObject([
      {
        observation: {
          label: 'Observed Herdr workspace',
        },
        runtimeAdapter: 'herdr',
      },
      {
        observation: null,
        runtimeAdapter: 'other',
      },
    ])
  })

  it('resolves conflicting duplicate keys independently of input order', () => {
    const candidates = [
      target('repeated-worker', 'workspace-z'),
      target('repeated-worker', 'workspace-a'),
    ]
    const forward = groupAgentWindowTargets(candidates, null, sessions)
    const reversed = groupAgentWindowTargets(
      [...candidates].reverse(),
      null,
      sessions,
    )

    expect(forward).toMatchObject([
      {
        targets: [{ key: 'repeated-worker' }],
        workspaceId: 'workspace-a',
      },
    ])
    expect(reversed).toEqual(forward)
  })

  it('filters window metadata and supports activity and runtime order', () => {
    const groups = groupAgentWindowTargets(
      [
        target('builder', 'workspace-a', {
          label: 'Builder',
          status: 'working',
          tabId: 'workspace-a:tab-2',
        }),
        target('planner', 'workspace-a', {
          label: 'Planner',
          status: 'idle',
          tabId: 'workspace-a:tab-1',
        }),
        target('reviewer', 'workspace-z', {
          cwd: '/work/release-tools/checks',
          label: 'Reviewer',
          status: 'blocked',
        }),
        target('offline', 'workspace-offline', {
          label: 'Offline worker',
          observation: 'durable',
          session: 'beta',
        }),
      ],
      {
        adapter: 'herdr',
        session: 'alpha',
        tabs: [
          {
            focused: false,
            label: 'Planner',
            order: 1,
            pane_count: 1,
            runtime_id: 'workspace-a:tab-1',
            status: 'idle',
            workspace_id: 'workspace-a',
          },
          {
            focused: false,
            label: 'Builder',
            order: 2,
            pane_count: 1,
            runtime_id: 'workspace-a:tab-2',
            status: 'working',
            workspace_id: 'workspace-a',
          },
        ],
        workspaces: [
          workspace('workspace-a', 1, 'API migration'),
          workspace('workspace-z', 2, 'Release checks'),
        ],
      },
      sessions,
    )

    expect(
      filterAndSortAgentWindowGroups(groups, '', 'activity').map(
        (group) => group.workspaceId,
      ),
    ).toEqual(['workspace-z', 'workspace-a', 'workspace-offline'])
    expect(
      filterAndSortAgentWindowGroups(groups, '', 'runtime').map(
        (group) => group.workspaceId,
      ),
    ).toEqual(['workspace-a', 'workspace-z', 'workspace-offline'])
    expect(
      filterAndSortAgentWindowGroups(groups, '', 'runtime')[0].targets.map(
        (candidate) => candidate.key,
      ),
    ).toEqual(['planner', 'builder'])
    expect(
      filterAndSortAgentWindowGroups(
        groups,
        'release-tools',
        'name',
      ).flatMap((group) => group.targets.map((candidate) => candidate.key)),
    ).toEqual(['reviewer'])
  })
})

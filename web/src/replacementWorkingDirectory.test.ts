import { describe, expect, it } from 'vitest'

import { replacementWorkingDirectoryDefault } from './replacementWorkingDirectory'
import type { PaneObservation, RuntimeInventory } from './types'

const project = {
  runtime: { adapter: 'herdr', session: 'alpha', workspace_id: 'workspace-1' },
}

function pane(
  runtimeId: string,
  workspaceId: string,
  cwd: string | null,
  focused = false,
): PaneObservation {
  return {
    runtime_id: runtimeId,
    terminal_id: `terminal-${runtimeId}`,
    workspace_id: workspaceId,
    tab_id: `${workspaceId}:tab-1`,
    focused,
    cwd,
    foreground_cwd: cwd,
    label: null,
    provider: null,
    display_provider: null,
    status: 'idle',
    tokens: {},
    provider_session: null,
    revision: '1',
  }
}

function inventory(
  panes: PaneObservation[],
  checkoutPath: string | null = null,
): RuntimeInventory {
  return {
    adapter: 'herdr',
    session: 'alpha',
    runtime_version: '0.8.0',
    protocol: 19,
    observed_at_unix_ms: 1,
    focus: { workspace_id: null, tab_id: null, pane_id: null },
    workspaces: [
      {
        runtime_id: 'workspace-1',
        order: 1,
        label: 'Project',
        focused: true,
        active_tab_id: 'workspace-1:tab-1',
        pane_count: panes.length,
        tab_count: 1,
        status: 'idle',
        tokens: {},
        worktree: checkoutPath
          ? {
              repository_key: 'repo',
              repository_name: 'repo',
              repository_root: checkoutPath,
              checkout_path: checkoutPath,
              is_linked: false,
            }
          : null,
      },
    ],
    tabs: [],
    panes,
    workers: [],
    child_agents: [],
  } as unknown as RuntimeInventory
}

describe('replacementWorkingDirectoryDefault', () => {
  it('prefers the workspace worktree checkout', () => {
    expect(
      replacementWorkingDirectoryDefault(
        project,
        inventory([pane('p1', 'workspace-1', '/work/pane')], '/work/checkout'),
      ),
    ).toBe('/work/checkout')
  })

  it('never prefills a pane directory, even a focused one', () => {
    expect(
      replacementWorkingDirectoryDefault(
        project,
        inventory([
          pane('other', 'workspace-2', '/work/other', true),
          pane('p1', 'workspace-1', '/work/first'),
          pane('p2', 'workspace-1', '/work/focused', true),
        ]),
      ),
    ).toBe('')
  })

  it('is empty without inventory for the project session', () => {
    expect(replacementWorkingDirectoryDefault(project, null)).toBe('')
    expect(
      replacementWorkingDirectoryDefault(
        { runtime: { ...project.runtime, session: 'beta' } },
        inventory([pane('p1', 'workspace-1', '/work/first')]),
      ),
    ).toBe('')
    expect(
      replacementWorkingDirectoryDefault(
        project,
        inventory([pane('p1', 'workspace-1', null)]),
      ),
    ).toBe('')
  })
})

import { describe, expect, it } from 'vitest'
import { YardApiError } from './api'
import {
  projectArchivePreconditions,
  projectDispositionBlocker,
  projectDispositionConfirmLabel,
  projectDispositionNotice,
  projectRestoreErrorMessage,
  projectRestoreNotice,
  projectRestoreRetryable,
  snapshotProgressLabel,
  workstreamDispositionBlocker,
  workstreamDispositionImpact,
  workstreamDispositionNotice,
} from './backgroundStatus'
import type {
  CoordinationNodeDispositionPreview,
  ProjectDispositionPreview,
} from './types'

function projectPreview(
  overrides: Partial<ProjectDispositionPreview> = {},
): ProjectDispositionPreview {
  return {
    project_id: 'project-1',
    name: 'Runtime API',
    project_version: '3',
    orchestrator_worker_id: 'orchestrator-1',
    orchestrator_worker_version: '4',
    orchestrator_runtime_version: null,
    active_assignments: [
      {
        assignment_id: 'assignment-1',
        assignment_version: '2',
        lifecycle: 'active',
        objective: 'Ship the parser.',
        role: 'implementer',
        profile_name: 'Implementer',
        worker_id: 'worker-1',
        runtime_present: true,
      },
      {
        assignment_id: 'assignment-2',
        assignment_version: '5',
        lifecycle: 'handing_off',
        objective: 'Review the parser.',
        role: 'reviewer',
        profile_name: 'Reviewer',
        worker_id: 'worker-2',
        runtime_present: false,
      },
    ],
    ...overrides,
  }
}

describe('project disposition background status', () => {
  it('keeps the plain notice when nothing continues in the background', () => {
    expect(
      projectDispositionNotice('archived', {
        cleanup_pending: false,
        background: { snapshots_pending: 0, snapshots_abandoned: 0 },
      }),
    ).toBe('Project archived.')
    expect(projectDispositionNotice('deleted', { cleanup_pending: false })).toBe(
      'Project deleted from Yard.',
    )
  })

  it('reports cleanup and snapshot work as status text', () => {
    expect(
      projectDispositionNotice('archived', {
        cleanup_pending: true,
        background: { snapshots_pending: 1, snapshots_abandoned: 2 },
      }),
    ).toBe(
      'Project archived. Verified orchestrator cleanup is queued. ' +
        'Snapshot pending: 1 knowledge snapshot can still collect this project. ' +
        'Snapshot expired: 2 knowledge snapshots stopped waiting for this project.',
    )
    expect(
      projectDispositionNotice('deleted', {
        cleanup_pending: true,
        background: { snapshots_pending: 2, snapshots_abandoned: 0 },
      }),
    ).toBe(
      'Project deleted from Yard. Runtime cleanup continues in the background. ' +
        'Snapshot pending: 2 knowledge snapshots can still collect this project.',
    )
  })

  it('labels expired snapshot collections without treating them as failures', () => {
    expect(
      snapshotProgressLabel({ completed: 1, total: 2, abandoned: 0 }),
    ).toBe('1/2 collected')
    expect(
      snapshotProgressLabel({ completed: 0, total: 2, abandoned: 1 }),
    ).toBe('0/2 collected · 1 expired')
  })

  it('sends exactly the versions and active workers the preview showed', () => {
    expect(projectArchivePreconditions(projectPreview())).toEqual({
      expected_project_version: '3',
      expected_orchestrator_worker_id: 'orchestrator-1',
      expected_orchestrator_worker_version: '4',
      expected_orchestrator_runtime_version: null,
      active_work: 'cancel',
      expected_active_assignments: [
        { assignment_id: 'assignment-1', expected_assignment_version: '2' },
        { assignment_id: 'assignment-2', expected_assignment_version: '5' },
      ],
    })
    expect(
      projectArchivePreconditions(projectPreview({ active_assignments: [] }))
        .expected_active_assignments,
    ).toEqual([])
  })

  it('also sends and counts the summary workers listed separately', () => {
    const preview = projectPreview({
      active_assignments: [],
      summary_worker_assignments: [
        {
          assignment_id: 'summary-assignment-1',
          assignment_version: '7',
          lifecycle: 'active',
          objective: 'Summarize the durable state.',
          role: 'summary_worker',
          profile_name: 'Summarizer',
          worker_id: 'summary-worker-1',
          runtime_present: true,
        },
      ],
    })
    expect(projectArchivePreconditions(preview).expected_active_assignments).toEqual([
      { assignment_id: 'summary-assignment-1', expected_assignment_version: '7' },
    ])
    expect(projectDispositionConfirmLabel('archive', preview)).toBe(
      'Archive and end 1 active worker',
    )
    expect(
      projectArchivePreconditions(
        projectPreview({ summary_worker_assignments: preview.summary_worker_assignments }),
      ).expected_active_assignments?.map((expected) => expected.assignment_id),
    ).toEqual(['assignment-1', 'assignment-2', 'summary-assignment-1'])
  })

  it('names the active workers a confirmation ends', () => {
    expect(projectDispositionConfirmLabel('archive', null)).toBe(
      'Archive project',
    )
    expect(
      projectDispositionConfirmLabel(
        'delete',
        projectPreview({ active_assignments: [] }),
      ),
    ).toBe('Delete project')
    expect(projectDispositionConfirmLabel('archive', projectPreview())).toBe(
      'Archive and end 2 active workers',
    )
    const one = projectPreview()
    one.active_assignments.pop()
    expect(projectDispositionConfirmLabel('delete', one)).toBe(
      'Delete and end 1 active worker',
    )
  })

  it('waits for workers that are still starting instead of confirming', () => {
    expect(projectDispositionBlocker(null)).toBeNull()
    expect(projectDispositionBlocker(projectPreview())).toBeNull()
    const starting = projectPreview()
    starting.active_assignments[0].lifecycle = 'allocating'
    expect(projectDispositionBlocker(starting)).toBe(
      'A worker is still starting. Try again once it has started or failed.',
    )
    starting.active_assignments[1].lifecycle = 'allocating'
    expect(projectDispositionBlocker(starting)).toBe(
      '2 workers are still starting. Try again once they have started or failed.',
    )
  })

  it('reports the workers an archive recorded as cancelled', () => {
    expect(
      projectDispositionNotice('archived', {
        cleanup_pending: true,
        cancelled_assignment_ids: ['assignment-1', 'assignment-2'],
      }),
    ).toBe(
      'Project archived. Ended 2 active workers; their assignments are recorded as cancelled. ' +
        'Verified orchestrator cleanup is queued.',
    )
    expect(
      projectDispositionNotice('deleted', {
        cleanup_pending: false,
        cancelled_assignment_ids: ['assignment-1'],
      }),
    ).toBe(
      'Project deleted from Yard. Ended 1 active worker; its assignment is recorded as cancelled.',
    )
  })
})

function workstreamPreview(
  overrides: Partial<CoordinationNodeDispositionPreview> = {},
): CoordinationNodeDispositionPreview {
  return {
    node_id: 'node-1',
    name: 'BAR <-> Nexus',
    kind: 'workstream',
    node_version: '3',
    supported: true,
    worker: {
      worker_id: 'worker-1',
      worker_version: '5',
      profile_name: 'Codex orchestrator',
      runtime_present: true,
      will_end: true,
    },
    attached_projects: [
      { project_id: 'project-1', name: 'BAR' },
      { project_id: 'project-2', name: 'Nexus' },
    ],
    automations: [
      { automation_id: 'automation-1', name: 'Daily', state: 'active' },
      { automation_id: 'automation-2', name: 'Weekly', state: 'paused' },
    ],
    blockers: [],
    ...overrides,
  }
}

describe('workstream disposition preview', () => {
  it('itemizes the worker, untouched projects, and paused automations', () => {
    expect(workstreamDispositionImpact(workstreamPreview())).toEqual([
      'Dedicated worker Codex orchestrator will be ended — its Herdr tab stays open until you close it.',
      '2 attached projects are not affected: BAR, Nexus.',
      '2 automations will be paused and leave the map with it.',
    ])
    expect(
      workstreamDispositionImpact(
        workstreamPreview({ worker: null, attached_projects: [], automations: [] }),
      ),
    ).toEqual(['No dedicated worker is running.', 'No projects are attached.'])
  })

  it('names the dedicated worker by its chosen name', () => {
    const worker = workstreamPreview().worker!
    expect(
      workstreamDispositionImpact(
        workstreamPreview({ worker: { ...worker, display_name: 'Docs lead' } }),
      )[0],
    ).toBe(
      'Dedicated worker Docs lead · Codex orchestrator will be ended — its Herdr tab stays open until you close it.',
    )
  })

  it('only mentions a Herdr tab when the worker still has one', () => {
    const worker = workstreamPreview().worker!
    expect(
      workstreamDispositionImpact(
        workstreamPreview({ worker: { ...worker, runtime_present: false } }),
      )[0],
    ).toBe('Dedicated worker Codex orchestrator will be ended (it has no open Herdr tab).')
    expect(
      workstreamDispositionImpact(
        workstreamPreview({ worker: { ...worker, profile_name: null } }),
      )[0],
    ).toBe(
      'The dedicated worker will be ended — its Herdr tab stays open until you close it.',
    )
    expect(
      workstreamDispositionImpact(
        workstreamPreview({
          worker: { ...worker, profile_name: null, will_end: false },
        }),
      )[0],
    ).toBe('The dedicated worker has already ended.')
  })

  it('names in-flight prompts and routes as the only blockers', () => {
    expect(workstreamDispositionBlocker(workstreamPreview())).toBeNull()
    expect(
      workstreamDispositionBlocker(
        workstreamPreview({
          blockers: [
            { kind: 'prompt', command_id: 'prompt-1', started_at_unix_ms: 1 },
            { kind: 'route', command_id: 'route-1', started_at_unix_ms: 2 },
            { kind: 'route', command_id: 'route-2', started_at_unix_ms: 3 },
          ],
        }),
      ),
    ).toBe('Wait for 1 pending prompt and 2 pending routes to finish, then reopen this dialog.')
  })

  it('reports worker cleanup and paused automations as status text', () => {
    expect(
      workstreamDispositionNotice('archived', {
        worker_id: 'worker-1',
        cleanup_pending: true,
        paused_automation_ids: ['automation-1'],
      }),
    ).toBe(
      'Workstream archived. Its worker was ended; cleanup is pending until you close its Herdr tab. Paused 1 automation and removed it from the map.',
    )
    expect(
      workstreamDispositionNotice('deleted', {
        worker_id: null,
        cleanup_pending: false,
        paused_automation_ids: [],
      }),
    ).toBe('Workstream deleted from Yard.')
  })
})

describe('project restore status', () => {
  it('says what Restore brought back and what stays cancelled', () => {
    expect(
      projectRestoreNotice('Runtime API', {
        cancelled_assignment_ids: [],
        orchestrator_runtime: 'rebound',
      }),
    ).toBe('Restored “Runtime API”.')
    expect(
      projectRestoreNotice('Runtime API', {
        cancelled_assignment_ids: ['assignment-1', 'assignment-2'],
        orchestrator_runtime: 'unbound',
      }),
    ).toBe(
      'Restored “Runtime API”. Its orchestrator’s Herdr tab was closed, so it is restored without a runtime. 2 cancelled assignments stay cancelled; their workers stay ended.',
    )
  })

  it('explains each refused Restore by its reason', () => {
    expect(
      projectRestoreErrorMessage(
        new YardApiError(
          'project_restore_unavailable',
          'server text',
          null,
          null,
          null,
          'herdr_unreachable',
        ),
      ),
    ).toBe(
      'Herdr is unreachable, so Yard cannot tell whether the orchestrator is still running. Retry when Herdr is back.',
    )
    expect(
      projectRestoreErrorMessage(
        new YardApiError(
          'project_restore_unavailable',
          'server text',
          null,
          null,
          null,
          'something_new',
        ),
      ),
    ).toBe('server text')
    expect(
      projectRestoreErrorMessage(
        new YardApiError('project_not_archived', 'Yard project is not archived'),
      ),
    ).toBe('This project is no longer archived.')
    expect(projectRestoreErrorMessage(new Error('offline'))).toBe('offline')
  })

  it('offers Retry only when resending the same Restore can succeed', () => {
    const unavailable = (reason: string) =>
      new YardApiError(
        'project_restore_unavailable',
        'text',
        null,
        null,
        null,
        reason,
      )
    expect(projectRestoreRetryable(unavailable('herdr_unreachable'))).toBe(true)
    expect(projectRestoreRetryable(new TypeError('Failed to fetch'))).toBe(true)
    expect(
      projectRestoreRetryable(new YardApiError('http_502', 'Bad gateway')),
    ).toBe(true)
    for (const reason of [
      'workspace_reserved',
      'runtime_reserved',
      'archive_changed',
      'project_deleted',
      'orchestrator_unavailable',
    ]) {
      expect(projectRestoreRetryable(unavailable(reason))).toBe(false)
    }
    expect(
      projectRestoreRetryable(
        new YardApiError('project_not_archived', 'Yard project is not archived'),
      ),
    ).toBe(false)
  })
})

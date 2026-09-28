import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  DispositionCommandIds,
  dispositionInput,
  dispositionKey,
  dispositionNotice,
  reconcileDisposition,
  submitDisposition,
  withLatestSessionVersions,
} from './assignmentDisposition'
import type { Assignment, CompletionReceipt } from './types'

function activeAssignment(): Assignment {
  return {
    id: 'assignment-1',
    project_id: 'project-1',
    allocation_id: 'allocation-1',
    worker: {
      id: 'worker-1',
      profile_id: 'profile-1',
      profile_version: '1',
      desired_state: 'running',
      ownership_kind: 'yard_owned',
      runtime: {
        adapter: 'herdr',
        session: 'default',
        workspace_id: 'workspace-1',
        terminal_id: 'terminal-1',
        tab_id: 'tab-1',
        pane_id: 'pane-1',
        provider_session: null,
        owns_tab: true,
        observation_state: 'observed',
        process_state: 'running',
        status: 'done',
        state_change_sequence: '1',
        revision: '1',
        version: '3',
        last_observed_at_unix_ms: 1,
      },
      version: '5',
      created_at_unix_ms: 1,
      updated_at_unix_ms: 1,
    },
    profile_id: 'profile-1',
    profile_version: '1',
    profile_name: 'Implementer',
    objective: 'Ship the API.',
    role: 'implementer',
    isolation_policy: 'project_workspace',
    lifecycle: 'active',
    attempt: {
      id: 'attempt-1',
      assignment_id: 'assignment-1',
      ordinal: 1,
      lifecycle: 'active',
      error: null,
      version: '2',
      created_at_unix_ms: 1,
      updated_at_unix_ms: 1,
    },
    completion_receipt: null,
    cancellation: null,
    version: '2',
    created_at_unix_ms: 1,
    updated_at_unix_ms: 1,
  }
}

function receipt(): CompletionReceipt {
  return {
    id: 'receipt-1',
    assignment_id: 'assignment-1',
    attempt_id: 'attempt-1',
    outcome: 'completed',
    detail_level: 'minimal',
    objective_snapshot: 'Ship the API.',
    summary: 'Completed without a detailed handoff.',
    artifact_refs: [],
    artifacts: [],
    evidence_refs: [],
    unresolved_blockers: [],
    actor: 'local-user',
    created_at_unix_ms: 2,
  }
}

function completed(): Assignment {
  return {
    ...activeAssignment(),
    lifecycle: 'completed',
    completion_receipt: receipt(),
    version: '3',
  }
}

function cancelled(): Assignment {
  return {
    ...activeAssignment(),
    lifecycle: 'cancelled',
    cancellation: {
      assignment_id: 'assignment-1',
      attempt_id: 'attempt-1',
      command_id: 'other-command',
      reason: 'ended_without_completion',
      actor: 'local-user',
      request_origin: 'browser',
      objective_snapshot: 'Ship the API.',
      cancelled_at_unix_ms: 2,
    },
    version: '3',
  }
}

function sequentialIds() {
  let next = 0
  return new DispositionCommandIds(() => `command-${++next}`)
}

function requestBodies(fetchMock: ReturnType<typeof vi.fn>) {
  return fetchMock.mock.calls.map((call) =>
    JSON.parse(String((call[1] as RequestInit).body)),
  )
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('assignment disposition', () => {
  it('sends session versions only when ending the session', () => {
    const assignment = activeAssignment()
    expect(dispositionInput(assignment, 'completed', false, 'c-1')).toEqual({
      command_id: 'c-1',
      actor: 'local-user',
      attempt_id: 'attempt-1',
      expected_assignment_version: '2',
      expected_attempt_version: '2',
      outcome: 'completed',
      end_session: false,
    })
    expect(dispositionInput(assignment, 'cancelled', true, 'c-2')).toMatchObject({
      end_session: true,
      expected_runtime_version: '3',
      expected_worker_version: '5',
      outcome: 'cancelled',
    })
    const noRuntime = {
      ...assignment,
      worker: { ...assignment.worker, runtime: null },
    }
    expect(
      dispositionInput(noRuntime, 'completed', true, 'c-3'),
    ).not.toHaveProperty('expected_runtime_version')
    expect(dispositionKey(assignment, 'completed', true)).not.toBe(
      dispositionKey(assignment, 'completed', false),
    )
  })

  it('reuses the command ID on retry until the request commits', async () => {
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new TypeError('Failed to fetch'))
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({ assignment: completed(), command_id: 'command-1' }),
          { status: 200 },
        ),
      )
    vi.stubGlobal('fetch', fetchMock)
    const commands = sequentialIds()
    const assignment = activeAssignment()
    const reload = vi.fn(async () => assignment)

    const failed = await submitDisposition({
      assignment,
      commands,
      endSession: true,
      outcome: 'completed',
      reload,
    })
    expect(failed).toEqual({
      code: null,
      kind: 'failed',
      message: 'Failed to fetch',
    })
    const retried = await submitDisposition({
      assignment,
      commands,
      endSession: true,
      outcome: 'completed',
      reload,
    })
    expect(retried.kind).toBe('committed')
    const bodies = requestBodies(fetchMock)
    expect(bodies[0]).toEqual(bodies[1])
    expect(bodies[0].command_id).toBe('command-1')
    // Once settled, a later action gets a fresh command ID.
    expect(commands.commandIdFor(dispositionKey(assignment, 'completed', true))).toBe(
      'command-2',
    )
  })

  it('reconciles a conflict as success only for the matching outcome', async () => {
    const conflict = () =>
      new Response(
        JSON.stringify({
          error: {
            code: 'assignment_not_active',
            message: 'This assignment is no longer active',
          },
        }),
        { status: 409 },
      )
    vi.stubGlobal('fetch', vi.fn(async () => conflict()))
    const assignment = activeAssignment()

    const matching = await submitDisposition({
      assignment,
      commands: sequentialIds(),
      endSession: false,
      outcome: 'completed',
      reload: async () => completed(),
    })
    expect(matching).toMatchObject({ disposed: null, kind: 'committed' })

    const other = await submitDisposition({
      assignment,
      commands: sequentialIds(),
      endSession: true,
      outcome: 'cancelled',
      reload: async () => completed(),
    })
    expect(other).toMatchObject({
      kind: 'other_outcome',
      message: 'Already completed by another action.',
    })

    const cancelledElsewhere = await submitDisposition({
      assignment,
      commands: sequentialIds(),
      endSession: false,
      outcome: 'completed',
      reload: async () => cancelled(),
    })
    expect(cancelledElsewhere).toMatchObject({
      kind: 'other_outcome',
      message: 'Already cancelled by another action.',
    })

    const stillOpen = await submitDisposition({
      assignment,
      commands: sequentialIds(),
      endSession: false,
      outcome: 'completed',
      reload: async () => assignment,
    })
    expect(stillOpen).toEqual({
      code: 'assignment_not_active',
      kind: 'failed',
      message: 'This assignment is no longer active',
    })
  })

  it('never treats a receipt-less completed row or open work as committed', () => {
    expect(
      reconcileDisposition(
        { ...completed(), completion_receipt: null },
        'completed',
      ),
    ).toBe('other_outcome')
    expect(reconcileDisposition(activeAssignment(), 'cancelled')).toBe('open')
    expect(reconcileDisposition(undefined, 'completed')).toBe('open')
    expect(reconcileDisposition(cancelled(), 'cancelled')).toBe('committed')
  })

  it('sends the latest session versions only for the decided assignment version', () => {
    const chosen = activeAssignment()
    const runtime = chosen.worker.runtime
    if (!runtime) throw new Error('fixture needs a runtime')
    const refreshed: Assignment = {
      ...chosen,
      worker: {
        ...chosen.worker,
        version: '6',
        runtime: { ...runtime, version: '4', state_change_sequence: '2' },
      },
    }
    const sent = withLatestSessionVersions(chosen, refreshed)
    expect(dispositionInput(sent, 'completed', true, 'command-1')).toMatchObject({
      expected_assignment_version: '2',
      expected_worker_version: '6',
      expected_runtime_version: '4',
    })
    // A changed assignment or attempt is not what the user decided on.
    expect(
      withLatestSessionVersions(chosen, { ...refreshed, version: '3' }),
    ).toBe(chosen)
    expect(
      withLatestSessionVersions(chosen, {
        ...refreshed,
        attempt: { ...refreshed.attempt, version: '3' },
      }),
    ).toBe(chosen)
    expect(withLatestSessionVersions(chosen, undefined)).toBe(chosen)
  })

  it('reports queued background work after a committed disposition', () => {
    expect(
      dispositionNotice('completed', true, {
        cleanup_pending: true,
        transcript_pending: true,
      }),
    ).toBe(
      'Completed. Yard ended the session; the agent keeps running until its Herdr tab is closed. Runtime cleanup continues in the background. Yard is still saving the transcript.',
    )
    expect(
      dispositionNotice('cancelled', true, {
        cleanup_pending: false,
        transcript_pending: false,
      }),
    ).toBe(
      'Yard stopped tracking this worker. The agent keeps running until its Herdr tab is closed.',
    )
  })
})

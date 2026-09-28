import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  YardApiError,
  disposeAssignment,
  fetchAssignmentTranscript,
  restoreProject,
} from './api'
import type { DisposeAssignmentInput } from './types'

const input: DisposeAssignmentInput = {
  command_id: 'command-1',
  actor: 'local-user',
  attempt_id: 'attempt-1',
  expected_assignment_version: '2',
  expected_attempt_version: '2',
  outcome: 'completed',
  end_session: true,
  expected_worker_version: '4',
  expected_runtime_version: '1',
}

function jsonResponse(status: number, body: unknown) {
  return new Response(JSON.stringify(body), {
    headers: { 'Content-Type': 'application/json' },
    status,
  })
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('assignment disposition client', () => {
  it('posts the disposition body as JSON to the assignment route', async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse(200, { command_id: 'command-1', replayed: false }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await disposeAssignment('project 1', 'assignment/1', input)

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [path, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ]
    expect(path).toBe(
      '/api/v1/projects/project%201/assignments/assignment%2F1/disposition',
    )
    expect(init.method).toBe('POST')
    expect(init.headers).toMatchObject({
      Accept: 'application/json',
      'Content-Type': 'application/json',
    })
    expect(JSON.parse(String(init.body))).toEqual(input)
  })

  it('surfaces the server error code for reconciliation', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        jsonResponse(409, {
          error: {
            code: 'assignment_not_active',
            message: 'This assignment is no longer active',
          },
        }),
      ),
    )

    const error = await disposeAssignment('project-1', 'assignment-1', input)
      .then(() => null)
      .catch((caught: unknown) => caught)
    expect(error).toBeInstanceOf(YardApiError)
    expect((error as YardApiError).code).toBe('assignment_not_active')
    expect((error as YardApiError).message).toBe(
      'This assignment is no longer active',
    )
  })

  it('reads the retained transcript with a GET', async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse(200, { status: 'captured', text: 'done' }),
    )
    vi.stubGlobal('fetch', fetchMock)

    const transcript = await fetchAssignmentTranscript(
      'project-1',
      'assignment-1',
    )

    expect(transcript.status).toBe('captured')
    const [path, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ]
    expect(path).toBe(
      '/api/v1/projects/project-1/assignments/assignment-1/transcript',
    )
    expect(init.method).toBeUndefined()
  })
})

describe('project restore client', () => {
  it('posts the restore command and surfaces the refusal reason', async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse(409, {
        error: {
          code: 'project_restore_unavailable',
          message: 'Herdr is unreachable',
          reason: 'herdr_unreachable',
        },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)
    const command = {
      command_id: 'restore-1',
      actor: 'local-user',
      expected_archive_command_id: 'archive-1',
    }

    const error = await restoreProject('project 1', command).catch(
      (caught: unknown) => caught,
    )

    const [path, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ]
    expect(path).toBe('/api/v1/projects/project%201/restore')
    expect(init.method).toBe('POST')
    expect(JSON.parse(String(init.body))).toEqual(command)
    expect(error).toBeInstanceOf(YardApiError)
    expect((error as YardApiError).code).toBe('project_restore_unavailable')
    expect((error as YardApiError).reason).toBe('herdr_unreachable')
  })
})

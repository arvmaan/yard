import { describe, expect, it } from 'vitest'
import { projectUpdates } from './projectUpdates'
import type { Assignment, Project, Worker } from './types'

const worker: Worker = {
  id: 'worker-1',
  profile_id: 'profile-1',
  profile_version: '1',
  desired_state: 'ended',
  ownership_kind: 'yard_owned',
  runtime: null,
  version: '4',
  created_at_unix_ms: 1,
  updated_at_unix_ms: 1,
}

const project: Project = {
  id: 'project-1',
  name: 'API migration',
  runtime: { adapter: 'herdr', session: 'default', workspace_id: 'workspace-1' },
  orchestrator: { ...worker, id: 'orchestrator-1', desired_state: 'running' },
  placement: {
    geometry: { x: 0, y: 0, width: 322, height: 240 },
    version: '1',
    updated_at_unix_ms: 1,
  },
  workflow_profile: {
    profile_id: 'yard:standard-orchestrator',
    profile_version: '1',
    pinned_by: 'local-user',
    pinned_at_unix_ms: 1,
  },
  version: '1',
  created_at_unix_ms: 1,
  updated_at_unix_ms: 1,
}

function ended(
  id: string,
  objective: string,
  outcome: 'minimal' | 'detailed' | 'cancelled',
  at: number,
): Assignment {
  return {
    id,
    project_id: project.id,
    allocation_id: `${id}-allocation`,
    worker,
    profile_id: 'profile-1',
    profile_version: '1',
    profile_name: 'Implementer',
    objective,
    role: 'implementer',
    isolation_policy: 'project_workspace',
    lifecycle: outcome === 'cancelled' ? 'cancelled' : 'completed',
    attempt: {
      id: `${id}-attempt`,
      assignment_id: id,
      ordinal: 1,
      lifecycle: outcome === 'cancelled' ? 'cancelled' : 'completed',
      error: null,
      version: '3',
      created_at_unix_ms: 1,
      updated_at_unix_ms: at,
    },
    completion_receipt:
      outcome === 'cancelled'
        ? null
        : {
            id: `${id}-receipt`,
            assignment_id: id,
            attempt_id: `${id}-attempt`,
            outcome: 'completed',
            detail_level: outcome,
            objective_snapshot: outcome === 'minimal' ? objective : null,
            summary:
              outcome === 'minimal'
                ? 'Completed without a detailed handoff.'
                : 'Shipped the endpoint with tests.',
            artifact_refs: [],
            artifacts: [],
            evidence_refs: outcome === 'minimal' ? [] : ['test://suite'],
            unresolved_blockers: [],
            actor: 'local-user',
            created_at_unix_ms: at,
          },
    cancellation:
      outcome === 'cancelled'
        ? {
            assignment_id: id,
            attempt_id: `${id}-attempt`,
            command_id: `${id}-command`,
            reason: 'ended_without_completion',
            actor: 'local-user',
            request_origin: 'browser',
            objective_snapshot: objective,
            cancelled_at_unix_ms: at,
          }
        : null,
    version: '3',
    created_at_unix_ms: 1,
    updated_at_unix_ms: at,
  }
}

function lastLine(assignments: Assignment[]) {
  return projectUpdates([project], assignments, null, [])[0].last
}

describe('project updates for ended work', () => {
  it('names the objective of a minimal completion', () => {
    expect(lastLine([ended('a-1', 'Migrate the API.', 'minimal', 10)])).toBe(
      'Completed: Migrate the API.',
    )
    expect(lastLine([ended('a-1', 'Migrate the API.', 'detailed', 10)])).toBe(
      'Shipped the endpoint with tests.',
    )
  })

  it('never reports cancelled work as completed', () => {
    expect(
      lastLine([
        ended('a-1', 'Migrate the API.', 'minimal', 10),
        ended('a-2', 'Rewrite the client.', 'cancelled', 20),
      ]),
    ).toBe('Ended without completion: Rewrite the client.')
    expect(
      lastLine([
        ended('a-2', 'Rewrite the client.', 'cancelled', 5),
        ended('a-1', 'Migrate the API.', 'minimal', 10),
      ]),
    ).toBe('Completed: Migrate the API.')
  })

  it('dates a project by its newest cancellation', () => {
    const [update] = projectUpdates(
      [project],
      [
        ended('a-1', 'Migrate the API.', 'minimal', 10),
        ended('a-2', 'Rewrite the client.', 'cancelled', 40),
      ],
      null,
      [],
    )
    expect(update.updatedAt).toBe(40)
  })
})

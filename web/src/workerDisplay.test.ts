import { describe, expect, it } from 'vitest'
import { workerDisplayLabel, type WorkerLabelSource } from './workerDisplay'

const source = (
  overrides: Partial<WorkerLabelSource> = {},
): WorkerLabelSource => ({
  workerId: '019ff387-aaaa-bbbb-cccc-000000000001',
  ...overrides,
})

describe('workerDisplayLabel', () => {
  it('uses assignment, profile, orchestrator, and observed names in order', () => {
    expect(
      workerDisplayLabel(
        source({
          assignmentRole: 'Implementer',
          observedName: 'Observed agent',
          profileName: 'UI specialist',
          projectName: 'Yard',
          projectOrchestratorName: 'Yard',
        }),
      ),
    ).toBe('Implementer · Yard')
    expect(
      workerDisplayLabel(
        source({
          observedName: 'Observed agent',
          profileName: 'UI specialist',
          projectOrchestratorName: 'Yard',
        }),
      ),
    ).toBe('UI specialist')
    expect(
      workerDisplayLabel(
        source({
          observedName: 'Observed agent',
          projectOrchestratorName: 'Yard',
        }),
      ),
    ).toBe('Yard orchestrator')
    expect(
      workerDisplayLabel(
        source({
          observedDisplayProvider: 'Codex',
          tabLabel: 'Updates',
          workspaceLabel: 'Yard',
        }),
      ),
    ).toBe('Codex · Updates · Yard')
  })

  it('extends duplicate fallback prefixes until they are unique', () => {
    const peers = [
      '019ff387-aaaa-bbbb-cccc-000000000001',
      '019ff387-bbbb-cccc-dddd-000000000002',
    ]
    expect(workerDisplayLabel(source(), peers)).toBe('Worker 019ff387-a')
    expect(
      workerDisplayLabel(
        source({ workerId: peers[1] }),
        peers,
      ),
    ).toBe('Worker 019ff387-b')
  })
})

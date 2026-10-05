import { describe, expect, it } from 'vitest'
import {
  labelWithDisplayName,
  secondaryDefaultLabel,
  workerDefaultLabel,
  workerDisplayLabel,
  workerLabelWithDefault,
  type WorkerLabelSource,
} from './workerDisplay'

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

describe('user-chosen worker names', () => {
  it('puts the name above role/project, profile, orchestrator and observed names', () => {
    const everything = source({
      assignmentRole: 'Implementer',
      displayName: 'BAR CDK',
      observedName: 'Observed agent',
      profileName: 'UI specialist',
      projectName: 'Yard',
      projectOrchestratorName: 'Yard',
    })
    expect(workerDisplayLabel(everything)).toBe('BAR CDK')
    expect(workerDefaultLabel(everything)).toBe('Implementer · Yard')
    expect(
      workerDisplayLabel(source({ displayName: 'Docs', profileName: 'Generalist' })),
    ).toBe('Docs')
    expect(
      workerDisplayLabel(
        source({ displayName: 'Lead', projectOrchestratorName: 'Yard' }),
      ),
    ).toBe('Lead')
    expect(
      workerDisplayLabel(
        source({ displayName: 'Scout', observedDisplayProvider: 'Codex' }),
      ),
    ).toBe('Scout')
  })

  it('falls through to the default chain for a blank or missing name', () => {
    for (const displayName of [null, undefined, '', '   ']) {
      expect(
        workerDisplayLabel(
          source({
            assignmentRole: 'implementer',
            displayName,
            projectName: 'Yard',
          }),
        ),
      ).toBe('Implementer · Yard')
    }
    expect(workerDisplayLabel(source({ displayName: ' Docs ' }))).toBe('Docs')
  })

  it('keeps peer-unique id prefixes for unnamed duplicates', () => {
    const peers = [
      '019ff387-aaaa-bbbb-cccc-000000000001',
      '019ff387-bbbb-cccc-dddd-000000000002',
      '019ff387-cccc-dddd-eeee-000000000003',
    ]
    expect(workerDisplayLabel(source({ displayName: null }), peers)).toBe(
      'Worker 019ff387-a',
    )
    expect(
      workerDisplayLabel(
        source({ displayName: '  ', workerId: peers[1] }),
        peers,
      ),
    ).toBe('Worker 019ff387-b')
    // A named peer keeps its name; the unnamed one still gets its prefix.
    expect(
      workerDisplayLabel(source({ displayName: 'Docs', workerId: peers[2] }), peers),
    ).toBe('Docs')
    expect(
      workerDefaultLabel(source({ displayName: 'Docs', workerId: peers[2] }), peers),
    ).toBe('Worker 019ff387-c')
  })

  it('shows the default label beside a name only when it differs', () => {
    expect(
      workerLabelWithDefault(
        source({ displayName: 'BAR CDK', profileName: 'Generalist' }),
      ),
    ).toBe('BAR CDK · Generalist')
    expect(
      workerLabelWithDefault(source({ profileName: 'Generalist' })),
    ).toBe('Generalist')
    expect(
      workerLabelWithDefault(
        source({ displayName: 'Generalist', profileName: 'Generalist' }),
      ),
    ).toBe('Generalist')
    expect(secondaryDefaultLabel('BAR CDK', 'Generalist')).toBe('Generalist')
    expect(secondaryDefaultLabel(null, 'Generalist')).toBeNull()
    expect(labelWithDisplayName('  ', 'Superintendent')).toBe('Superintendent')
  })
})

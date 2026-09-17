import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AllocationDialog, type AllocationSubject } from './AllocationDialog'
import type { Project, WorkerCandidate } from './types'

const subject: AllocationSubject = {
  kind: 'worker',
  candidate: {
    availability: 'resumable',
    default_role: 'implementer',
    profile_name: 'Implementer',
    worker: {
      id: 'worker-1',
      profile_id: null,
      profile_version: null,
    },
  } as WorkerCandidate,
}

describe('AllocationDialog', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('names replacement allocation without claiming to resume a runtime', () => {
    vi.stubGlobal('HTMLElement', class {})
    vi.stubGlobal('document', { activeElement: null })
    const markup = renderToStaticMarkup(
      createElement(AllocationDialog, {
        busy: false,
        error: null,
        onClose: () => undefined,
        onConfirm: async () => undefined,
        profiles: [],
        project: { id: 'project-1', name: 'Yard' } as Project,
        subject,
      }),
    )

    expect(markup.match(/Replace runtime and assign/g)).toHaveLength(2)
    expect(markup).not.toContain('Resume worker')
  })
})

import { describe, expect, it } from 'vitest'
import {
  createWorkerProfileEditorState,
  emptyWorkerProfile,
  workerProfileEditorReducer,
  WORKER_PROFILE_TEMPLATES,
} from './workerProfileTemplates'

describe('worker profile template catalog', () => {
  it('offers four distinct general-purpose starting profiles', () => {
    expect(
      WORKER_PROFILE_TEMPLATES.map(
        ({ description, id, label, spec }) => ({
          description,
          id,
          label,
          name: spec.name,
          role: spec.default_role,
        }),
      ),
    ).toEqual([
      {
        description:
          'Broad execution from planning through follow-through.',
        id: 'generalist',
        label: 'Generalist',
        name: 'Generalist',
        role: 'generalist',
      },
      {
        description:
          'Decompose, delegate, monitor, and synthesize work.',
        id: 'orchestrator',
        label: 'Orchestrator',
        name: 'Orchestrator',
        role: 'orchestrator',
      },
      {
        description: 'Evidence-first exploration and debugging.',
        id: 'investigator',
        label: 'Investigator',
        name: 'Investigator',
        role: 'investigator',
      },
      {
        description:
          'Independent behavior, test, and risk validation.',
        id: 'verifier',
        label: 'Verifier',
        name: 'Verifier',
        role: 'verifier',
      },
    ])
  })

  it('starts a blank profile with a generalist role', () => {
    expect(emptyWorkerProfile()).toMatchObject({
      default_role: 'generalist',
      instructions_ref: null,
      name: '',
    })
  })
})

describe('profile editor template state', () => {
  it('applies every catalog template to the editable profile', () => {
    for (const template of WORKER_PROFILE_TEMPLATES) {
      const state = workerProfileEditorReducer(
        createWorkerProfileEditorState(null),
        {
          templateId: template.id,
          type: 'select-template',
        },
      )

      expect(state.spec).toMatchObject({
        default_role: template.spec.default_role,
        instructions_ref: 'AGENTS.md',
        name: template.spec.name,
      })
      expect(state.templateId).toBe(template.id)
    }
  })

  it('keeps provider and model editable independently of templates', () => {
    let state = createWorkerProfileEditorState(null)
    state = workerProfileEditorReducer(state, {
      templateId: 'generalist',
      type: 'select-template',
    })
    state = workerProfileEditorReducer(state, {
      patch: { model: 'sonnet', provider: 'claude' },
      type: 'update-spec',
    })
    state = workerProfileEditorReducer(state, {
      templateId: 'orchestrator',
      type: 'select-template',
    })

    expect(state).toMatchObject({
      spec: {
        default_role: 'orchestrator',
        instructions_ref: 'AGENTS.md',
        model: 'sonnet',
        name: 'Orchestrator',
        provider: 'claude',
      },
      templateId: 'orchestrator',
    })
  })

  it('returns to an editable blank profile without changing provider', () => {
    let state = createWorkerProfileEditorState(null)
    state = workerProfileEditorReducer(state, {
      patch: { provider: 'kiro' },
      type: 'update-spec',
    })
    state = workerProfileEditorReducer(state, {
      templateId: 'verifier',
      type: 'select-template',
    })
    state = workerProfileEditorReducer(state, {
      templateId: '',
      type: 'select-template',
    })

    expect(state).toMatchObject({
      spec: {
        default_role: 'generalist',
        instructions_ref: null,
        name: '',
        provider: 'kiro',
      },
      templateId: '',
    })
  })

  it('accepts any custom role after applying a template', () => {
    let state = createWorkerProfileEditorState(null)
    state = workerProfileEditorReducer(state, {
      templateId: 'investigator',
      type: 'select-template',
    })
    state = workerProfileEditorReducer(state, {
      patch: { default_role: 'database-migration-lead' },
      type: 'update-spec',
    })

    expect(state.spec.default_role).toBe('database-migration-lead')
  })
})

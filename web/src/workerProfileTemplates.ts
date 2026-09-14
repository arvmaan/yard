import type { WorkerProfile, WorkerProfileSpec } from './types'

export interface WorkerProfileTemplate {
  description: string
  id: string
  label: string
  spec: WorkerProfileSpec
}

const BASE_PERMISSION_OPTIONS = [
  { label: 'Runtime default', value: 'runtime_default' },
  { label: 'Full access', value: 'yolo' },
] as const

export function workerProfilePermissionOptions(provider: string) {
  if (provider !== 'claude') return BASE_PERMISSION_OPTIONS
  return [
    BASE_PERMISSION_OPTIONS[0],
    { label: 'Auto (Claude)', value: 'auto' },
    BASE_PERMISSION_OPTIONS[1],
  ] as const
}

const BASE_PROFILE: WorkerProfileSpec = {
  name: '',
  runtime_adapter: 'herdr',
  provider: 'codex',
  model: null,
  default_role: 'generalist',
  instructions_ref: 'AGENTS.md',
  tools: [],
  skills: [],
  mcp_servers: [],
  sandbox_policy: 'runtime_default',
  worktree_policy: 'project_workspace',
  permission_policy: 'runtime_default',
  completion_contract: 'manual_receipt',
}

function profile(
  fields: Pick<WorkerProfileSpec, 'name' | 'default_role'>,
): WorkerProfileSpec {
  return { ...BASE_PROFILE, ...fields }
}

export const WORKER_PROFILE_TEMPLATES: readonly WorkerProfileTemplate[] = [
  {
    description: 'Broad execution from planning through follow-through.',
    id: 'generalist',
    label: 'Generalist',
    spec: profile({
      name: 'Generalist',
      default_role: 'generalist',
    }),
  },
  {
    description: 'Decompose, delegate, monitor, and synthesize work.',
    id: 'orchestrator',
    label: 'Orchestrator',
    spec: profile({
      name: 'Orchestrator',
      default_role: 'orchestrator',
    }),
  },
  {
    description: 'Evidence-first exploration and debugging.',
    id: 'investigator',
    label: 'Investigator',
    spec: profile({
      name: 'Investigator',
      default_role: 'investigator',
    }),
  },
  {
    description: 'Independent behavior, test, and risk validation.',
    id: 'verifier',
    label: 'Verifier',
    spec: profile({
      name: 'Verifier',
      default_role: 'verifier',
    }),
  },
]

export function emptyWorkerProfile(): WorkerProfileSpec {
  return { ...BASE_PROFILE, instructions_ref: null }
}

export function applyWorkerProfileTemplate(
  current: WorkerProfileSpec,
  templateId: string,
): WorkerProfileSpec | null {
  const template = WORKER_PROFILE_TEMPLATES.find(
    ({ id }) => id === templateId,
  )
  if (templateId && !template) return null

  const next = template ? template.spec : emptyWorkerProfile()
  return {
    ...next,
    model: current.model,
    provider: current.provider,
    runtime_adapter: current.runtime_adapter,
  }
}

export interface WorkerProfileEditorState {
  spec: WorkerProfileSpec
  templateId: string
}

export type WorkerProfileEditorAction =
  | { profile: WorkerProfile | null; type: 'reset' }
  | { patch: Partial<WorkerProfileSpec>; type: 'update-spec' }
  | { templateId: string; type: 'select-template' }

export function createWorkerProfileEditorState(
  profile: WorkerProfile | null,
): WorkerProfileEditorState {
  return {
    spec: profile ?? emptyWorkerProfile(),
    templateId: '',
  }
}

export function workerProfileEditorReducer(
  state: WorkerProfileEditorState,
  action: WorkerProfileEditorAction,
): WorkerProfileEditorState {
  switch (action.type) {
    case 'reset':
      return createWorkerProfileEditorState(action.profile)
    case 'select-template': {
      const spec = applyWorkerProfileTemplate(
        state.spec,
        action.templateId,
      )
      if (!spec) return state
      return { spec, templateId: action.templateId }
    }
    case 'update-spec': {
      const spec = { ...state.spec, ...action.patch }
      if (spec.provider !== 'claude' && spec.permission_policy === 'auto') {
        spec.permission_policy = 'runtime_default'
      }
      return {
        ...state,
        spec,
      }
    }
  }
}

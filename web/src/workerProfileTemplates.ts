import type { WorkerProfileSpec } from './types'

export interface WorkerProfileTemplate {
  id: string
  label: string
  spec: WorkerProfileSpec
}

const BASE_PROFILE: WorkerProfileSpec = {
  name: '',
  runtime_adapter: 'herdr',
  provider: 'codex',
  model: null,
  default_role: 'implementer',
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
    id: 'implementer',
    label: 'Implementer',
    spec: profile({
      name: 'Implementer',
      default_role: 'implementer',
    }),
  },
  {
    id: 'reviewer',
    label: 'Reviewer',
    spec: profile({
      name: 'Reviewer',
      default_role: 'reviewer',
    }),
  },
  {
    id: 'investigator',
    label: 'Investigator',
    spec: profile({
      name: 'Investigator',
      default_role: 'investigator',
    }),
  },
  {
    id: 'orchestrator',
    label: 'Orchestrator',
    spec: profile({
      name: 'Orchestrator',
      default_role: 'orchestrator',
    }),
  },
]

export function emptyWorkerProfile(): WorkerProfileSpec {
  return { ...BASE_PROFILE, instructions_ref: null }
}

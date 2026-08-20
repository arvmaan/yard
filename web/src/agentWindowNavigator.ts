import type {
  RuntimeSession,
  WorkspaceObservation,
} from './types'

export type AgentWindowRole =
  | 'superintendent'
  | 'workstream'
  | 'orchestrator'
  | 'worker'

export interface AgentWindowTarget {
  contextLabel: string
  key: string
  label: string
  role: AgentWindowRole
  runtimeAdapter: string
  session: string
  workspaceId: string
}

export interface AgentWindowInventory {
  adapter: string
  session: string
  workspaces: WorkspaceObservation[]
}

export interface AgentWindowWorkspaceGroup<
  Target extends AgentWindowTarget = AgentWindowTarget,
> {
  key: string
  observation: WorkspaceObservation | null
  runtimeAdapter: string
  session: string
  sessionRunning: boolean | undefined
  targets: Target[]
  workspaceId: string
}

const roleOrder: Record<AgentWindowRole, number> = {
  superintendent: 0,
  workstream: 1,
  orchestrator: 2,
  worker: 3,
}

function compareText(left: string, right: string) {
  const normalizedLeft = left.toLocaleLowerCase('en-US')
  const normalizedRight = right.toLocaleLowerCase('en-US')
  if (normalizedLeft < normalizedRight) return -1
  if (normalizedLeft > normalizedRight) return 1
  if (left < right) return -1
  if (left > right) return 1
  return 0
}

function compareTargets(
  left: AgentWindowTarget,
  right: AgentWindowTarget,
) {
  return (
    roleOrder[left.role] - roleOrder[right.role] ||
    compareText(left.contextLabel, right.contextLabel) ||
    compareText(left.label, right.label) ||
    compareText(left.key, right.key) ||
    compareText(left.runtimeAdapter, right.runtimeAdapter) ||
    compareText(left.session, right.session) ||
    compareText(left.workspaceId, right.workspaceId)
  )
}

function compareGroups(
  left: AgentWindowWorkspaceGroup,
  right: AgentWindowWorkspaceGroup,
) {
  const leftOrder =
    left.observation?.order ?? Number.POSITIVE_INFINITY
  const rightOrder =
    right.observation?.order ?? Number.POSITIVE_INFINITY
  const leftLabel = left.observation?.label ?? ''
  const rightLabel = right.observation?.label ?? ''

  return (
    leftOrder - rightOrder ||
    compareText(leftLabel, rightLabel) ||
    compareText(left.workspaceId, right.workspaceId) ||
    compareText(left.runtimeAdapter, right.runtimeAdapter) ||
    compareText(left.session, right.session)
  )
}

export function groupAgentWindowTargets<
  Target extends AgentWindowTarget,
>(
  targets: Target[],
  inventory: AgentWindowInventory | null,
  sessions: RuntimeSession[],
): AgentWindowWorkspaceGroup<Target>[] {
  const groups = new Map<string, AgentWindowWorkspaceGroup<Target>>()
  const seenTargets = new Set<string>()
  const workspaceObservations = new Map(
    inventory?.workspaces.map((workspace) => [
      workspace.runtime_id,
      workspace,
    ]) ?? [],
  )
  const sessionStates = new Map(
    sessions.map((session) => [session.name, session.running]),
  )
  const sortedTargets = [...targets].sort(compareTargets)

  sortedTargets.forEach((target) => {
    if (seenTargets.has(target.key)) return
    seenTargets.add(target.key)

    const key = JSON.stringify([
      target.runtimeAdapter,
      target.session,
      target.workspaceId,
    ])
    let group = groups.get(key)
    if (!group) {
      const observation =
        inventory?.adapter === target.runtimeAdapter &&
        inventory.session === target.session
          ? workspaceObservations.get(target.workspaceId) ?? null
          : null
      group = {
        key,
        observation,
        runtimeAdapter: target.runtimeAdapter,
        session: target.session,
        sessionRunning:
          target.runtimeAdapter === 'herdr'
            ? sessionStates.get(target.session)
            : undefined,
        targets: [],
        workspaceId: target.workspaceId,
      }
      groups.set(key, group)
    }
    group.targets.push(target)
  })

  return [...groups.values()]
    .map((group) => ({
      ...group,
      targets: [...group.targets].sort(compareTargets),
    }))
    .sort(compareGroups)
}

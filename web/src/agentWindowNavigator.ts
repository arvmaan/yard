import type {
  ObservedStatus,
  RuntimeSession,
  TabObservation,
  WorkspaceObservation,
} from './types'

export type AgentWindowSort = 'activity' | 'runtime' | 'name'

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
  tabId: string | null
  workspaceId: string
}

export interface FilterableAgentWindowTarget extends AgentWindowTarget {
  cwd: string | null
  harness: string
  interactive: boolean
  observation: 'observed' | 'stale' | 'durable'
  paneId: string
  roleLabel: string
  status: ObservedStatus
  terminalId: string
}

export interface AgentWindowInventory {
  adapter: string
  session: string
  tabs?: TabObservation[]
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

export function selectAgentWindowSessionTargets<
  Target extends AgentWindowTarget,
>(targets: Target[], selectedSession: string) {
  const selected: Target[] = []
  const elsewhereKeys = new Set<string>()
  targets.forEach((target) => {
    if (target.session === selectedSession) {
      selected.push(target)
    } else {
      elsewhereKeys.add(target.key)
    }
  })
  return { elsewhereCount: elsewhereKeys.size, selected }
}

const roleOrder: Record<AgentWindowRole, number> = {
  superintendent: 0,
  workstream: 1,
  orchestrator: 2,
  worker: 3,
}

const activityOrder: Record<ObservedStatus, number> = {
  blocked: 0,
  working: 1,
  idle: 2,
  done: 3,
  unknown: 4,
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

function compareRuntimeTargets(
  left: AgentWindowTarget,
  right: AgentWindowTarget,
  tabOrders: Map<string, number>,
) {
  const leftOrder = left.tabId
    ? tabOrders.get(left.tabId) ?? Number.POSITIVE_INFINITY
    : Number.POSITIVE_INFINITY
  const rightOrder = right.tabId
    ? tabOrders.get(right.tabId) ?? Number.POSITIVE_INFINITY
    : Number.POSITIVE_INFINITY
  return leftOrder - rightOrder || compareTargets(left, right)
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

function targetActivityRank(target: FilterableAgentWindowTarget) {
  return target.interactive ? activityOrder[target.status] : 5
}

function groupActivityRank(
  group: AgentWindowWorkspaceGroup<FilterableAgentWindowTarget>,
) {
  if (group.sessionRunning === false) return 6
  return Math.min(...group.targets.map(targetActivityRank))
}

function groupSearchText(
  group: AgentWindowWorkspaceGroup<FilterableAgentWindowTarget>,
) {
  const workspace = group.observation
  return [
    group.runtimeAdapter,
    group.session,
    group.workspaceId,
    workspace?.label,
    workspace?.status,
    workspace?.worktree?.repository_name,
    workspace?.worktree?.checkout_path,
    workspace?.focused ? 'focused' : null,
    group.sessionRunning === false ? 'offline' : null,
    workspace ? 'observed' : 'durable not observed',
  ]
    .filter(Boolean)
    .join(' ')
    .toLocaleLowerCase('en-US')
}

function targetSearchText(target: FilterableAgentWindowTarget) {
  return [
    target.label,
    target.contextLabel,
    target.roleLabel,
    target.status,
    target.harness,
    target.session,
    target.workspaceId,
    target.terminalId,
    target.tabId,
    target.paneId,
    target.cwd,
    target.observation,
    target.interactive ? 'interactive' : 'live controls unavailable',
  ]
    .filter(Boolean)
    .join(' ')
    .toLocaleLowerCase('en-US')
}

export function filterAndSortAgentWindowGroups<
  Target extends FilterableAgentWindowTarget,
>(
  groups: AgentWindowWorkspaceGroup<Target>[],
  query: string,
  sort: AgentWindowSort,
): AgentWindowWorkspaceGroup<Target>[] {
  const normalizedQuery = query.trim().toLocaleLowerCase('en-US')
  const visibleGroups = groups.flatMap((group) => {
    const groupMatches =
      normalizedQuery.length === 0 ||
      groupSearchText(group).includes(normalizedQuery)
    const targets = groupMatches
      ? group.targets
      : group.targets.filter((target) =>
          targetSearchText(target).includes(normalizedQuery),
        )
    if (targets.length === 0) return []

    const sortedTargets = [...targets]
    if (sort === 'activity') {
      sortedTargets.sort(
        (left, right) =>
          targetActivityRank(left) - targetActivityRank(right) ||
          compareTargets(left, right),
      )
    } else if (sort === 'name') {
      sortedTargets.sort(
        (left, right) =>
          compareText(left.label, right.label) ||
          compareText(left.contextLabel, right.contextLabel) ||
          compareTargets(left, right),
      )
    }
    return [{ ...group, targets: sortedTargets }]
  })

  if (sort === 'activity') {
    visibleGroups.sort(
      (left, right) =>
        groupActivityRank(left) - groupActivityRank(right) ||
        Number(Boolean(right.observation?.focused)) -
          Number(Boolean(left.observation?.focused)) ||
        compareGroups(left, right),
    )
  } else if (sort === 'name') {
    visibleGroups.sort(
      (left, right) =>
        compareText(
          left.observation?.label ?? left.workspaceId,
          right.observation?.label ?? right.workspaceId,
        ) || compareGroups(left, right),
    )
  }

  return visibleGroups
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
  const tabOrders = new Map(
    inventory?.tabs?.map((tab) => [tab.runtime_id, tab.order]) ?? [],
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
      targets: [...group.targets].sort((left, right) =>
        compareRuntimeTargets(left, right, tabOrders),
      ),
    }))
    .sort(compareGroups)
}

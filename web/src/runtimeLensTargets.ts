import type {
  RuntimeInventory,
  RuntimeLensEntry,
} from './types'
import type { AgentWorkspaceTarget } from './AgentWorkspaceContext'

export interface RuntimeLensWorkspaceGroup {
  entries: RuntimeLensEntry[]
  label: string
  workspaceId: string
}

export function runtimeLensEntryKey(entry: RuntimeLensEntry) {
  return JSON.stringify([
    entry.session,
    entry.workspace_id,
    entry.tab_id,
    entry.pane_id,
    entry.terminal_id,
  ])
}

export function runtimeLensEntryLabel(entry: RuntimeLensEntry) {
  return (
    entry.profile_name ??
    entry.name ??
    entry.label ??
    entry.worker_id ??
    entry.terminal_id
  )
}


function targetWorkerId(target: AgentWorkspaceTarget) {
  if (target.target.kind === 'assignment') return target.target.assignment.worker.id
  if (target.target.kind === 'orchestrator') return target.target.project.orchestrator.id
  if (target.target.kind === 'coordination-node') return target.target.node.worker?.id
  return target.target.orchestrator.worker?.id
}

function targetRuntimeKey(target: AgentWorkspaceTarget) {
  return JSON.stringify([
    target.session,
    target.workspaceId,
    target.tabId,
    target.paneId,
    target.terminalId,
  ])
}

function searchText(entry: RuntimeLensEntry) {
  return [
    runtimeLensEntryLabel(entry),
    entry.classification,
    entry.reason,
    entry.session,
    entry.workspace_id,
    entry.tab_id,
    entry.pane_id,
    entry.terminal_id,
    entry.provider,
    entry.display_provider,
    entry.status,
  ]
    .filter(Boolean)
    .join(' ')
    .toLocaleLowerCase('en-US')
}

export function groupRuntimeLensEntries(
  entries: RuntimeLensEntry[],
  inventory: RuntimeInventory | null,
  query: string,
  controlledTargets: AgentWorkspaceTarget[] = [],
): RuntimeLensWorkspaceGroup[] {
  const normalizedQuery = query.trim().toLocaleLowerCase('en-US')
  const controlledWorkerIds = new Set(
    controlledTargets.map(targetWorkerId).filter((id): id is string => Boolean(id)),
  )
  const controlledRuntimeKeys = new Set(controlledTargets.map(targetRuntimeKey))
  const workspaceOrder = new Map(
    inventory?.workspaces.map((workspace) => [
      workspace.runtime_id,
      workspace.order,
    ]) ?? [],
  )
  const workspaceLabels = new Map(
    inventory?.workspaces.map((workspace) => [
      workspace.runtime_id,
      workspace.label,
    ]) ?? [],
  )
  const groups = new Map<string, RuntimeLensEntry[]>()

  entries
    .filter(
      (entry) =>
        entry.classification !== 'linked_yard_worker' ||
        (!(entry.worker_id && controlledWorkerIds.has(entry.worker_id)) &&
          !controlledRuntimeKeys.has(runtimeLensEntryKey(entry))),
    )
    .filter(
      (entry) =>
        normalizedQuery.length === 0 ||
        searchText(entry).includes(normalizedQuery),
    )
    .forEach((entry) => {
      const group = groups.get(entry.workspace_id) ?? []
      group.push(entry)
      groups.set(entry.workspace_id, group)
    })

  return [...groups.entries()]
    .map(([workspaceId, groupedEntries]) => ({
      entries: groupedEntries.sort(
        (left, right) =>
          (left.tab_id ?? '').localeCompare(right.tab_id ?? '') ||
          left.pane_id.localeCompare(right.pane_id) ||
          left.terminal_id.localeCompare(right.terminal_id),
      ),
      label: workspaceLabels.get(workspaceId) ?? 'Unobserved workspace',
      workspaceId,
    }))
    .sort(
      (left, right) =>
        (workspaceOrder.get(left.workspaceId) ?? Number.MAX_SAFE_INTEGER) -
          (workspaceOrder.get(right.workspaceId) ?? Number.MAX_SAFE_INTEGER) ||
        left.label.localeCompare(right.label) ||
        left.workspaceId.localeCompare(right.workspaceId),
    )
}

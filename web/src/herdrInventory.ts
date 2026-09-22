import type {
  HerdrFleetInventory,
  HerdrLivePane,
} from './types'

export interface HerdrWorkspaceGroup {
  id: string
  label: string
  panes: HerdrLivePane[]
  paneCount: number
  unattachedCount: number
}

export interface HerdrSessionGroup {
  id: string
  name: string
  label: string
  isDefault: boolean
  metadataReason: string | null
  panes: number
  unattached: number
  workspaces: HerdrWorkspaceGroup[]
}

export interface HerdrInventorySnapshot {
  inventory: HerdrFleetInventory
  staleSessions: string[]
}

export interface HerdrInventoryCounts {
  freshTotal: number
  visibleTotal: number
  visibleAgents: number
  visibleRuntime: number
  visibleUnattached: number
  staleTotal: number
  failedSessions: number
}

const SAFE_PANE_REASONS = new Set([
  'Live agent observed.',
  'Live providerless agent observed.',
  'Live runtime pane observed.',
  'Herdr reported this pane identity more than once.',
  'Multiple live agents report this pane identity.',
  'The pane and live agent report conflicting provider identity.',
  'The pane reported a provider session that the live agent did not confirm.',
  'The pane reports an agent provider without a matching live agent observation.',
  'Partial agent evidence.',
  'Partial or conflicting agent evidence.',
  'Conflicting tab identity.',
  'Missing workspace ancestry.',
  'Missing tab ancestry.',
  'Missing workspace and tab ancestry.',
  'Conflicting pane ancestry.',
  'No live pane observed.',
])

export function mergeHerdrInventorySnapshot(
  previous: HerdrFleetInventory | null,
  next: HerdrFleetInventory,
): HerdrInventorySnapshot {
  if (!previous || next.failures.length === 0) {
    return { inventory: next, staleSessions: [] }
  }
  const freshSessions = new Set(next.sessions.map((session) => session.id))
  const failedSessions = new Set(next.failures.map((failure) => failure.session))
  const retained = previous.sessions.filter(
    (session) =>
      failedSessions.has(session.id) && !freshSessions.has(session.id),
  )
  return {
    inventory: { ...next, sessions: [...next.sessions, ...retained] },
    staleSessions: retained.map((session) => session.id),
  }
}

export function safeHerdrRequestError(code?: string) {
  if (code === 'herdr_session_discovery_unavailable') {
    return 'Unable to discover running Herdr sessions. Retry the refresh.'
  }
  return code === 'herdr_fleet_unavailable' || code === 'herdr_unavailable'
    ? 'Live Herdr inventory is temporarily unavailable. Retry the refresh.'
    : 'Live Herdr inventory could not be refreshed. Check Herdr and retry.'
}

export function clearHerdrInventoryFailures(
  inventory: HerdrFleetInventory | null,
) {
  return inventory ? { ...inventory, failures: [] } : null
}

export function safeHerdrFailureReason(code: string) {
  if (code === 'session_metadata_ambiguous') {
    return 'Herdr reported conflicting session metadata.'
  }
  return code === 'session_identity_mismatch'
    ? 'Herdr returned a snapshot for a different session.'
    : 'Herdr snapshot was unavailable for this session.'
}

const SAFE_METADATA_CONFLICTS = new Set([
  'title',
  'working directory',
  'tab',
  'process status',
])

export function safeHerdrPaneMetadataReason(pane: HerdrLivePane) {
  const prefix = 'Conflicting or incomplete metadata: '
  if (!pane.metadata_reason?.startsWith(prefix) || !pane.metadata_reason.endsWith('.')) {
    return null
  }
  const fields = pane.metadata_reason.slice(prefix.length, -1).split(', ')
  return fields.length > 0 &&
    fields.length <= SAFE_METADATA_CONFLICTS.size &&
    fields.every((field) => SAFE_METADATA_CONFLICTS.has(field))
    ? `${prefix}${fields.join(', ')}.`
    : null
}

export function safeHerdrPaneReason(pane: HerdrLivePane) {
  if (SAFE_PANE_REASONS.has(pane.reason)) return pane.reason
  if (pane.observation === 'ambiguous') {
    return 'Live identity evidence conflicts.'
  }
  if (pane.observation === 'unobserved') {
    return 'Live identity evidence is incomplete.'
  }
  return pane.kind === 'agent'
    ? 'Live agent observed.'
    : 'Live runtime pane observed.'
}


function canonicalPane(left: HerdrLivePane, right: HerdrLivePane) {
  return JSON.stringify(left) <= JSON.stringify(right) ? left : right
}

export function dedupeHerdrPanes(panes: HerdrLivePane[]) {
  const unique = new Map<string, HerdrLivePane>()
  panes.forEach((pane) => {
    const current = unique.get(pane.identity_key)
    unique.set(pane.identity_key, current ? canonicalPane(current, pane) : pane)
  })
  return [...unique.values()]
}

export function countHerdrInventory(
  inventory: HerdrFleetInventory,
  groups: HerdrSessionGroup[],
  staleSessions: Set<string>,
  staleAll: boolean,
): HerdrInventoryCounts {
  const all = inventory.sessions.flatMap((session) =>
    dedupeHerdrPanes(session.panes).map((pane) => ({ pane, session: session.id })),
  )
  const visible = groups.flatMap((session) =>
    session.workspaces.flatMap((workspace) => workspace.panes),
  )
  const stale = ({ session }: { session: string }) =>
    staleAll || staleSessions.has(session)
  const livePanes = all.filter(({ pane }) => pane.has_live_pane)
  const visiblePanes = visible.filter((pane) => pane.has_live_pane)
  return {
    freshTotal: livePanes.filter((entry) => !stale(entry)).length,
    visibleTotal: visiblePanes.length,
    visibleAgents: visiblePanes.filter((pane) => pane.kind === 'agent').length,
    visibleRuntime: visiblePanes.filter((pane) => pane.kind === 'runtime').length,
    visibleUnattached: visible.filter((pane) => !pane.has_live_pane).length,
    staleTotal: livePanes.filter(stale).length,
    failedSessions: inventory.failures.length,
  }
}

export function herdrPaneClassificationLabel(pane: HerdrLivePane) {
  if (!pane.has_live_pane) return 'Unattached agent'
  return pane.kind === 'agent' ? 'Agent pane' : 'Runtime pane'
}

export function herdrPaneStatusLabel(pane: HerdrLivePane, stale: boolean) {
  if (stale) return 'Last observed'
  return pane.status === 'unknown' ? 'Status not reported' : pane.status
}

function uniqueText(values: Array<string | null>) {
  const seen = new Set<string>()
  return values.filter((value): value is string => {
    if (!value) return false
    const key = value.toLocaleLowerCase()
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
}

export function herdrPaneAccessibleName(
  pane: HerdrLivePane,
  sessionLabel: string,
  workspaceLabel: string,
) {
  const label = herdrPaneLabel(pane)
  const classification = herdrPaneClassificationLabel(pane)
  return uniqueText([
    label,
    label.toLocaleLowerCase().includes(classification.toLocaleLowerCase())
      ? null
      : classification,
    sessionLabel,
    workspaceLabel,
  ]).join(', ')
}

export function herdrPaneAccessibleDescription(
  pane: HerdrLivePane,
  stale: boolean,
) {
  return uniqueText([
    herdrPaneStatusLabel(pane, false),
    stale ? 'Stale snapshot; last observed.' : null,
    safeHerdrPaneReason(pane),
    safeHerdrPaneMetadataReason(pane),
  ]).join(' ')
}

function countNoun(count: number, singular: string, plural = `${singular}s`) {
  return `${count} ${count === 1 ? singular : plural}`
}

export function herdrInventoryStatus(
  counts: HerdrInventoryCounts,
  visibleFailures = counts.failedSessions,
) {
  const resultCount =
    counts.visibleTotal + counts.visibleUnattached + visibleFailures
  const totalPanes = counts.freshTotal + counts.staleTotal
  return [
    `Showing ${countNoun(resultCount, 'result')}: ${counts.visibleTotal} of ${countNoun(totalPanes, 'Herdr pane')}`,
    counts.visibleUnattached
      ? countNoun(counts.visibleUnattached, 'unattached agent')
      : null,
    `${countNoun(counts.failedSessions, 'session failure')}.`,
  ]
    .filter(Boolean)
    .join('; ')
}

export function herdrRequestFailureStatus(
  attemptedSessions: number,
  failedSessions: number,
) {
  return `${failedSessions} of ${countNoun(
    attemptedSessions,
    'Herdr session snapshot',
  )} failed.`
}

export function filterHerdrFailures(
  inventory: HerdrFleetInventory,
  query: string,
) {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle) return inventory.failures
  return inventory.failures.filter((failure) =>
    `${failure.session} ${safeHerdrFailureReason(failure.code)}`
      .toLocaleLowerCase()
      .includes(needle),
  )
}

export function filterHerdrInventory(
  inventory: HerdrFleetInventory,
  query: string,
  staleSessions = new Set<string>(),
  staleAll = false,
): HerdrSessionGroup[] {
  const needle = query.trim().toLocaleLowerCase()
  const nameCounts = new Map<string, number>()
  inventory.sessions.forEach((session) => {
    nameCounts.set(session.name, (nameCounts.get(session.name) ?? 0) + 1)
  })
  return inventory.sessions.flatMap((session) => {
    const sessionStale = staleAll || staleSessions.has(session.id)
    const sessionLabel =
      (nameCounts.get(session.name) ?? 0) > 1
        ? `${session.name} · ${session.id}`
        : session.name
    const sessionFailureText = inventory.failures
      .filter((failure) => failure.session === session.id)
      .map((failure) => safeHerdrFailureReason(failure.code))
      .join(' ')
    const sessionMatches = Boolean(
      needle &&
        [
          session.id,
          session.name,
          sessionLabel,
          session.is_default ? 'default session default' : null,
          session.metadata_ambiguous ? 'ambiguous session metadata' : null,
          session.metadata_ambiguous ? 'Conflicting session metadata.' : null,
          sessionFailureText,
        ]
          .filter(Boolean)
          .join(' ')
          .toLocaleLowerCase()
          .includes(needle),
    )
    const workspaces = new Map<string, HerdrWorkspaceGroup>()
    dedupeHerdrPanes(session.panes).forEach((pane) => {
      const classification = herdrPaneClassificationLabel(pane)
      const status = herdrPaneStatusLabel(pane, sessionStale)
      const searchable = [
        session.id,
        session.name,
        sessionLabel,
        session.is_default ? 'default session default' : null,
        session.metadata_ambiguous ? 'ambiguous session metadata' : null,
        session.metadata_ambiguous ? 'Conflicting session metadata.' : null,
        sessionStale
          ? 'stale Last observed Last observed details Refresh for current information'
          : null,
        pane.workspace_id,
        pane.workspace_label ?? 'Ambiguous location',
        pane.tab_id,
        pane.pane_id,
        pane.terminal_id,
        pane.label,
        pane.name,
        pane.provider,
        pane.display_provider,
        classification,
        pane.kind === 'agent' ? 'agent evidence' : 'runtime',
        pane.observation,
        status,
        herdrPaneStatusLabel(pane, false),
        safeHerdrPaneReason(pane),
        safeHerdrPaneMetadataReason(pane),
      ]
        .filter(Boolean)
        .join(' ')
        .toLocaleLowerCase()
      if (needle && !sessionMatches && !searchable.includes(needle)) return
      const workspaceId = pane.workspace_id ?? `ambiguous:${pane.identity_key}`
      const group = workspaces.get(workspaceId) ?? {
        id: workspaceId,
        label: pane.workspace_label ?? 'Ambiguous location',
        panes: [],
        paneCount: 0,
        unattachedCount: 0,
      }
      group.panes.push(pane)
      if (pane.has_live_pane) group.paneCount += 1
      else group.unattachedCount += 1
      workspaces.set(workspaceId, group)
    })
    const groups = [...workspaces.values()]
    return groups.length
      ? [{
          id: session.id,
          name: session.name,
          label: sessionLabel,
          isDefault: session.is_default,
          metadataReason: session.metadata_ambiguous
            ? 'Conflicting session metadata.'
            : null,
          panes: groups.reduce((count, group) => count + group.paneCount, 0),
          unattached: groups.reduce(
            (count, group) => count + group.unattachedCount,
            0,
          ),
          workspaces: groups,
        }]
      : []
  })
}

export function herdrPaneLabel(pane: HerdrLivePane) {
  const runtimeIdentity = pane.terminal_id ?? pane.pane_id
  if (!pane.has_live_pane) {
    const evidenceLabel = pane.name ?? pane.label
    return evidenceLabel ? `Unattached agent · ${evidenceLabel}` : 'Unattached agent'
  }
  return (
    pane.name ??
    pane.label ??
    (pane.kind === 'agent'
      ? runtimeIdentity
        ? `Unnamed agent · ${runtimeIdentity}`
        : 'Unnamed agent'
      : runtimeIdentity ?? 'Unidentified runtime pane')
  )
}

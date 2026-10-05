export interface WorkerLabelSource {
  assignmentRole?: string | null
  // The user-chosen worker name (`display_name`). It wins over every other
  // source; blank or absent falls through to the default chain below.
  displayName?: string | null
  observedDisplayProvider?: string | null
  observedName?: string | null
  observedProvider?: string | null
  profileName?: string | null
  projectName?: string | null
  projectOrchestratorName?: string | null
  tabLabel?: string | null
  workerId: string
  workspaceLabel?: string | null
}

function clean(value: string | null | undefined) {
  return value?.trim() || null
}

function displayRole(value: string) {
  return value === value.toLocaleLowerCase('en-US')
    ? `${value.charAt(0).toLocaleUpperCase('en-US')}${value.slice(1)}`
    : value
}

function appendContext(label: string, context: string | null) {
  if (!context) return label
  return label.toLocaleLowerCase('en-US').includes(
    context.toLocaleLowerCase('en-US'),
  )
    ? label
    : `${label} · ${context}`
}

function observedLabel(source: WorkerLabelSource) {
  const primary =
    clean(source.observedName) ??
    clean(source.observedDisplayProvider) ??
    clean(source.observedProvider)
  if (!primary) return null
  const tabLabel = clean(source.tabLabel)
  const workspaceLabel = clean(source.workspaceLabel)
  return appendContext(
    appendContext(primary, tabLabel),
    workspaceLabel === tabLabel ? null : workspaceLabel,
  )
}

function workerIdPrefix(workerId: string, peerWorkerIds: string[]) {
  let length = Math.min(8, workerId.length)
  while (
    length < workerId.length &&
    peerWorkerIds.some(
      (candidate) =>
        candidate !== workerId &&
        candidate.slice(0, length) === workerId.slice(0, length),
    )
  ) {
    length += 1
  }
  return workerId.slice(0, length)
}

/**
 * The one label rule: a user-chosen name, when set, wins over the default
 * label. Blank names fall through.
 */
export function labelWithDisplayName(
  displayName: string | null | undefined,
  defaultLabel: string,
): string {
  return clean(displayName) ?? defaultLabel
}

/**
 * The default label as secondary text, only when a user-chosen name replaced
 * it, so "BAR CDK" can still show that it is a Generalist.
 */
export function secondaryDefaultLabel(
  displayName: string | null | undefined,
  defaultLabel: string | null | undefined,
): string | null {
  const name = clean(displayName)
  if (!name || !defaultLabel || defaultLabel === name) return null
  return defaultLabel
}

/** The label the worker would have without a user-chosen name. */
export function workerDefaultLabel(
  source: WorkerLabelSource,
  peerWorkerIds: string[] = [],
): string {
  const role = clean(source.assignmentRole)
  const project = clean(source.projectName)
  if (role && project) return `${displayRole(role)} · ${project}`

  const profile = clean(source.profileName)
  if (profile) return profile

  const orchestratorProject = clean(source.projectOrchestratorName)
  if (orchestratorProject) return `${orchestratorProject} orchestrator`

  const observed = observedLabel(source)
  if (observed) return observed

  return `Worker ${workerIdPrefix(source.workerId, peerWorkerIds)}`
}

export function workerDisplayLabel(
  source: WorkerLabelSource,
  peerWorkerIds: string[] = [],
): string {
  return labelWithDisplayName(
    source.displayName,
    workerDefaultLabel(source, peerWorkerIds),
  )
}

/** "BAR CDK · Generalist" where one line has room for both. */
export function workerLabelWithDefault(
  source: WorkerLabelSource,
  peerWorkerIds: string[] = [],
): string {
  const fallback = workerDefaultLabel(source, peerWorkerIds)
  const label = labelWithDisplayName(source.displayName, fallback)
  const secondary = secondaryDefaultLabel(source.displayName, fallback)
  return secondary ? `${label} · ${secondary}` : label
}

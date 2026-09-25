export interface WorkerLabelSource {
  assignmentRole?: string | null
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

export function workerDisplayLabel(
  source: WorkerLabelSource,
  peerWorkerIds: string[] = [],
) {
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

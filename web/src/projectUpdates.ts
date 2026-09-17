import {
  resolveRuntimeCapabilities,
  runtimeCapabilityStatus,
} from './runtimeCapabilities'
import type {
  Assignment,
  ObservedStatus,
  Project,
  RuntimeInventory,
  StatusReport,
  WorkerRuntimeBinding,
  WorkflowStatus,
  YardOrchestratorRoute,
} from './types'

export type ProjectUpdateState =
  | 'in_progress'
  | 'needs_action'
  | 'offline'
  | 'ready'

export interface ProjectUpdate {
  blockers: string[]
  last: string
  next: string
  project: Project
  report: StatusReport | null
  state: ProjectUpdateState
  updatedAt: number
}

export type ProjectStatusReports = Record<string, StatusReport | undefined>

const ACTIVE_LIFECYCLES = new Set([
  'allocating',
  'active',
  'handing_off',
])
const WORKFLOW_STATES = new Set<WorkflowStatus>([
  'working',
  'needs_attention',
  'idle',
])
const REPORT_STATE: Record<WorkflowStatus, ProjectUpdateState> = {
  idle: 'ready',
  needs_attention: 'needs_action',
  working: 'in_progress',
}
const STATUS_REPORT_KEYS = new Set([
  'version',
  'command_id',
  'state',
  'last',
  'next',
  'blockers',
])
const MAX_STATUS_REPORT_LINE_BYTES = 16_384
const UTF8_ENCODER = new TextEncoder()

function observedStatus(
  runtime: WorkerRuntimeBinding | null,
  inventory: RuntimeInventory | null,
): ObservedStatus {
  const capabilities = resolveRuntimeCapabilities(true, runtime, inventory)
  return runtimeCapabilityStatus(runtime, capabilities)
}

function newest<T>(
  values: T[],
  timestamp: (value: T) => number,
): T | undefined {
  return [...values].sort(
    (left, right) => timestamp(right) - timestamp(left),
  )[0]
}

export function parseStatusReport(value: unknown): StatusReport | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const candidate = value as Record<string, unknown>
  const keys = Object.keys(candidate)
  const commandId = candidate.command_id
  const blockers = candidate.blockers
  if (
    keys.length !== STATUS_REPORT_KEYS.size ||
    !keys.every((key) => STATUS_REPORT_KEYS.has(key)) ||
    candidate.version !== 1 ||
    typeof commandId !== 'string' ||
    commandId.length === 0 ||
    commandId.trim() !== commandId ||
    UTF8_ENCODER.encode(commandId).byteLength > 120 ||
    typeof candidate.state !== 'string' ||
    !WORKFLOW_STATES.has(candidate.state as WorkflowStatus) ||
    typeof candidate.last !== 'string' ||
    UTF8_ENCODER.encode(candidate.last).byteLength > 4096 ||
    typeof candidate.next !== 'string' ||
    UTF8_ENCODER.encode(candidate.next).byteLength > 4096 ||
    !Array.isArray(blockers) ||
    blockers.length > 32 ||
    !blockers.every(
      (blocker) =>
        typeof blocker === 'string' &&
        blocker.length > 0 &&
        blocker.trim() === blocker &&
        UTF8_ENCODER.encode(blocker).byteLength <= 2048,
    )
  ) {
    return null
  }

  const report: StatusReport = {
    version: 1,
    command_id: commandId,
    state: candidate.state as WorkflowStatus,
    last: candidate.last,
    next: candidate.next,
    blockers: [...blockers] as string[],
  }
  if (
    UTF8_ENCODER.encode(JSON.stringify(report)).byteLength >
    MAX_STATUS_REPORT_LINE_BYTES
  ) {
    return null
  }
  return report
}

export function statusReportMatchesLatestRoute(
  report: StatusReport,
  projectId: string,
  routes: YardOrchestratorRoute[],
): boolean {
  const knownRoutes = routes.filter(
    (route) => route.command_id === report.command_id,
  )
  if (knownRoutes.length === 0) return true
  const deliveredRoutes = knownRoutes.filter(
    (route) =>
      route.status === 'submitted' &&
      route.runtime_status !== null &&
      route.submitted_at_unix_ms !== null,
  )
  if (
    deliveredRoutes.length === 0 ||
    deliveredRoutes.some((route) => route.target_project_id !== projectId)
  ) {
    return false
  }

  const latestProjectRoute = newest(
    routes.filter(
      (route) =>
        route.target_project_id === projectId &&
        route.status === 'submitted' &&
        route.runtime_status !== null &&
        route.submitted_at_unix_ms !== null,
    ),
    (route) => route.updated_at_unix_ms,
  )
  return latestProjectRoute?.command_id === report.command_id
}

export function projectUpdates(
  projects: Project[],
  assignments: Assignment[],
  inventory: RuntimeInventory | null,
  routes: YardOrchestratorRoute[],
  statusReports: ProjectStatusReports = {},
): ProjectUpdate[] {
  return projects
    .map((project): ProjectUpdate => {
      const report = statusReports[project.id] ?? null
      const projectAssignments = assignments.filter(
        (assignment) => assignment.project_id === project.id,
      )
      const active = projectAssignments.filter((assignment) =>
        ACTIVE_LIFECYCLES.has(assignment.lifecycle),
      )
      const latestActive = newest(
        active,
        (assignment) => assignment.updated_at_unix_ms,
      )
      const latestCompletion = newest(
        projectAssignments.filter(
          (assignment) => assignment.completion_receipt !== null,
        ),
        (assignment) =>
          assignment.completion_receipt?.created_at_unix_ms ??
          assignment.updated_at_unix_ms,
      )
      const latestRoute = newest(
        routes.filter((route) => route.target_project_id === project.id),
        (route) => route.updated_at_unix_ms,
      )
      const orchestratorRuntime = project.orchestrator.runtime
      const orchestratorStatus = observedStatus(orchestratorRuntime, inventory)
      const attentionAssignment = newest(
        active.filter((assignment) => {
          const status = observedStatus(assignment.worker.runtime, inventory)
          return status === 'blocked' || status === 'done'
        }),
        (assignment) => assignment.updated_at_unix_ms,
      )
      const unresolvedBlocker =
        latestCompletion?.completion_receipt?.unresolved_blockers[0]
      const completionBlockers =
        latestCompletion?.completion_receipt?.unresolved_blockers ?? []
      const routeNeedsAction =
        latestRoute?.status === 'failed' ||
        latestRoute?.status === 'ambiguous'
      const orchestratorNeedsAction = orchestratorStatus === 'blocked'
      const orchestratorOffline =
        orchestratorRuntime === null ||
        orchestratorRuntime.process_state !== 'running' ||
        orchestratorRuntime.observation_state === 'ambiguous' ||
        orchestratorStatus === 'unknown'
      const routeIsLatest =
        latestRoute !== undefined &&
        latestRoute.updated_at_unix_ms >
          (latestCompletion?.completion_receipt?.created_at_unix_ms ?? 0) &&
        latestRoute.updated_at_unix_ms >
          (latestActive?.updated_at_unix_ms ?? 0)

      const durableLast = latestCompletion?.completion_receipt
        ? latestCompletion.completion_receipt.summary
        : latestActive
          ? `Started ${latestActive.objective}`
          : latestRoute
            ? `Directed: ${latestRoute.text}`
            : 'No completed work recorded.'
      let durableNext = 'No active work queued.'
      if (routeNeedsAction) {
        durableNext =
          latestRoute.error_message?.trim()
            ? latestRoute.error_message
            : `Retry the last instruction: ${latestRoute.text}`
      } else if (attentionAssignment) {
        const assignmentStatus = observedStatus(
          attentionAssignment.worker.runtime,
          inventory,
        )
        durableNext = `Review ${attentionAssignment.profile_name} (${assignmentStatus}): ${attentionAssignment.objective}`
      } else if (unresolvedBlocker) {
        durableNext = `Resolve completion blocker: ${unresolvedBlocker}`
      } else if (orchestratorNeedsAction) {
        durableNext =
          'Open the project orchestrator and resolve its blocked runtime.'
      } else if (latestActive) {
        durableNext = latestActive.objective
      } else if (orchestratorOffline) {
        durableNext = 'Reconnect the project orchestrator.'
      } else if (routeIsLatest) {
        durableNext = 'Awaiting the project orchestrator.'
      }
      const durableNeedsAction =
        attentionAssignment !== undefined ||
        completionBlockers.length > 0 ||
        routeNeedsAction ||
        orchestratorNeedsAction

      let state: ProjectUpdateState
      let last: string
      let next: string
      let blockers: string[]
      if (report && !durableNeedsAction) {
        state = REPORT_STATE[report.state]
        last = report.last
        next = report.next
        blockers = report.blockers
      } else {
        state = 'ready'
        if (durableNeedsAction) {
          state = 'needs_action'
        } else if (orchestratorOffline) {
          state = 'offline'
        } else if (
          active.length > 0 ||
          orchestratorStatus === 'working' ||
          (routeIsLatest && latestRoute.status === 'submitted')
        ) {
          state = 'in_progress'
        }

        last = report?.last || durableLast
        next = durableNext
        blockers = [
          ...new Set([...completionBlockers, ...(report?.blockers ?? [])]),
        ]
      }

      return {
        blockers,
        last,
        next,
        project,
        report,
        state,
        updatedAt: Math.max(
          project.updated_at_unix_ms,
          latestActive?.updated_at_unix_ms ?? 0,
          latestCompletion?.completion_receipt?.created_at_unix_ms ?? 0,
          latestRoute?.updated_at_unix_ms ?? 0,
        ),
      }
    })
    .sort((left, right) => {
      const priority: Record<ProjectUpdateState, number> = {
        needs_action: 0,
        in_progress: 1,
        offline: 2,
        ready: 3,
      }
      return (
        priority[left.state] - priority[right.state] ||
        right.updatedAt - left.updatedAt ||
        left.project.name.localeCompare(right.project.name)
      )
    })
}

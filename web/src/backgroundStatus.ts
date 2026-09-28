import { YardApiError } from './api'
import type {
  CoordinationNodeDispositionPreview,
  CoordinationSnapshot,
  ProjectArchivePreconditions,
  ProjectBackgroundStatus,
  ProjectDispositionPreview,
  ProjectRestoreUnavailableReason,
  RestoredProject,
} from './types'

function count(value: number, noun: string) {
  return `${value} ${noun}${value === 1 ? '' : 's'}`
}

// Every assignment the archive ends: the active workers, then Yard's own
// summary workers, which the preview lists separately.
export function projectDispositionEndedAssignments(
  preview: ProjectDispositionPreview | null,
) {
  // No preview: deleting a project that was already archived ends nothing.
  if (preview === null) return []
  return [
    ...preview.active_assignments,
    ...(preview.summary_worker_assignments ?? []),
  ]
}

// The archive preconditions are exactly what the preview showed: its
// versions, and every listed active assignment to record as cancelled. The
// server refuses with a fresh preview if anything changed since.
export function projectArchivePreconditions(
  preview: ProjectDispositionPreview,
): ProjectArchivePreconditions {
  return {
    expected_project_version: preview.project_version,
    expected_orchestrator_worker_id: preview.orchestrator_worker_id,
    expected_orchestrator_worker_version: preview.orchestrator_worker_version,
    expected_orchestrator_runtime_version: preview.orchestrator_runtime_version,
    active_work: 'cancel',
    expected_active_assignments: projectDispositionEndedAssignments(preview).map(
      (assignment) => ({
        assignment_id: assignment.assignment_id,
        expected_assignment_version: assignment.assignment_version,
      }),
    ),
  }
}

// The confirm label names the workers the command ends, so nothing is ended
// behind a generic "Archive project" button.
export function projectDispositionConfirmLabel(
  mode: 'archive' | 'delete',
  preview: ProjectDispositionPreview | null,
) {
  const active = preview ? projectDispositionEndedAssignments(preview).length : 0
  if (active === 0) {
    return mode === 'archive' ? 'Archive project' : 'Delete project'
  }
  return `${mode === 'archive' ? 'Archive' : 'Delete'} and end ${count(active, 'active worker')}`
}

// The server cannot end a worker that is still starting (its allocation is
// unresolved), so an archive or delete listing one is always refused. The
// dialog says so and waits instead of offering a confirmation that fails.
export function projectDispositionBlocker(
  preview: ProjectDispositionPreview | null,
) {
  const starting =
    preview?.active_assignments.filter(
      (assignment) => assignment.lifecycle === 'allocating',
    ).length ?? 0
  if (starting === 0) return null
  return `${starting === 1 ? 'A worker is' : `${starting} workers are`} still starting. Try again once ${starting === 1 ? 'it has' : 'they have'} started or failed.`
}

// 409s that carry a fresh preview: the active workers changed (or an old
// request did not list them), so the user must review the list again.
export const PROJECT_PREVIEW_REFRESH_CODES = new Set([
  'project_archive_preview_stale',
  'project_has_active_work',
])

// 409s that commit nothing and mean the versions the preview pinned are
// out of date; the dialog re-reads the preview and asks again.
export const PROJECT_VERSION_REFRESH_CODES = new Set([
  'project_version_conflict',
  'project_archive_conflict',
])

// The disposition has already committed, so unfinished background work is
// reported as status text and never as an error.
export function projectDispositionNotice(
  disposition: 'archived' | 'deleted',
  result: {
    cleanup_pending: boolean
    background?: ProjectBackgroundStatus
    cancelled_assignment_ids?: string[]
  },
) {
  const parts = [
    disposition === 'archived' ? 'Project archived.' : 'Project deleted from Yard.',
  ]
  const ended = result.cancelled_assignment_ids?.length ?? 0
  if (ended > 0) {
    parts.push(
      `Ended ${count(ended, 'active worker')}; ${ended === 1 ? 'its assignment is' : 'their assignments are'} recorded as cancelled.`,
    )
  }
  if (result.cleanup_pending) {
    parts.push(
      disposition === 'archived'
        ? 'Verified orchestrator cleanup is queued.'
        : 'Runtime cleanup continues in the background.',
    )
  }
  const pending = result.background?.snapshots_pending ?? 0
  const abandoned = result.background?.snapshots_abandoned ?? 0
  if (pending > 0) {
    parts.push(
      `Snapshot pending: ${count(pending, 'knowledge snapshot')} can still collect this project.`,
    )
  }
  if (abandoned > 0) {
    parts.push(
      `Snapshot expired: ${count(abandoned, 'knowledge snapshot')} stopped waiting for this project.`,
    )
  }
  return parts.join(' ')
}

export function snapshotProgressLabel(
  progress: CoordinationSnapshot['progress'],
) {
  const collected = `${progress.completed}/${progress.total} collected`
  return progress.abandoned > 0
    ? `${collected} · ${progress.abandoned} expired`
    : collected
}

// Everything a workstream archive or delete does, itemized from the preview
// so the confirmation shows it before anything can fail late.
export function workstreamDispositionImpact(
  preview: CoordinationNodeDispositionPreview,
) {
  const lines: string[] = []
  const worker = preview.worker
  const workerName = worker?.profile_name
    ? `Dedicated worker ${worker.profile_name}`
    : 'The dedicated worker'
  if (worker?.will_end) {
    lines.push(
      worker.runtime_present
        ? `${workerName} will be ended — its Herdr tab stays open until you close it.`
        : `${workerName} will be ended (it has no open Herdr tab).`,
    )
  } else if (worker) {
    lines.push(`${workerName} has already ended.`)
  } else {
    lines.push('No dedicated worker is running.')
  }
  const projects = preview.attached_projects.length
  lines.push(
    projects === 0
      ? 'No projects are attached.'
      : `${count(projects, 'attached project')} ${projects === 1 ? 'is' : 'are'} not affected: ${preview.attached_projects
          .map((project) => project.name)
          .join(', ')}.`,
  )
  // Every automation scoped to the workstream leaves the map with it; the
  // active ones are paused first, so none of them runs again.
  const automations = preview.automations.length
  if (automations > 0) {
    lines.push(
      `${count(automations, 'automation')} will be paused and leave the map with it.`,
    )
  }
  return lines
}

// In-flight work that makes archive and delete wait, or null.
export function workstreamDispositionBlocker(
  preview: CoordinationNodeDispositionPreview,
) {
  if (preview.blockers.length === 0) return null
  const prompts = preview.blockers.filter(
    (blocker) => blocker.kind === 'prompt',
  ).length
  const routes = preview.blockers.length - prompts
  const work = [
    prompts > 0 ? count(prompts, 'pending prompt') : null,
    routes > 0 ? count(routes, 'pending route') : null,
  ]
    .filter(Boolean)
    .join(' and ')
  return `Wait for ${work} to finish, then reopen this dialog.`
}

export function workstreamDispositionNotice(
  disposition: 'archived' | 'deleted',
  result: {
    worker_id: string | null
    cleanup_pending: boolean
    paused_automation_ids: string[]
  },
) {
  const parts = [
    disposition === 'archived'
      ? 'Workstream archived.'
      : 'Workstream deleted from Yard.',
  ]
  if (result.worker_id) {
    parts.push(
      result.cleanup_pending
        ? 'Its worker was ended; cleanup is pending until you close its Herdr tab.'
        : 'Its worker was ended.',
    )
  }
  if (result.paused_automation_ids.length > 0) {
    parts.push(
      `Paused ${count(result.paused_automation_ids.length, 'automation')} and removed ${result.paused_automation_ids.length === 1 ? 'it' : 'them'} from the map.`,
    )
  }
  return parts.join(' ')
}

// How long the Undo for an archive is offered after it commits (design D7).
export const ARCHIVE_UNDO_WINDOW_MS = 10_000

// Restore puts the project back but never reopens what the archive ended.
export function projectRestoreNotice(
  name: string,
  result: Pick<RestoredProject, 'cancelled_assignment_ids' | 'orchestrator_runtime'>,
) {
  const parts = [`Restored “${name}”.`]
  if (result.orchestrator_runtime === 'unbound') {
    parts.push(
      'Its orchestrator’s Herdr tab was closed, so it is restored without a runtime.',
    )
  }
  const cancelled = result.cancelled_assignment_ids.length
  if (cancelled > 0) {
    parts.push(
      `${count(cancelled, 'cancelled assignment')} ${cancelled === 1 ? 'stays' : 'stay'} cancelled; ${cancelled === 1 ? 'its worker stays' : 'their workers stay'} ended.`,
    )
  }
  return parts.join(' ')
}

const RESTORE_UNAVAILABLE_MESSAGES: Record<
  ProjectRestoreUnavailableReason,
  string
> = {
  herdr_unreachable:
    'Herdr is unreachable, so Yard cannot tell whether the orchestrator is still running. Retry when Herdr is back.',
  workspace_reserved:
    'Another project or pending Herdr work now holds this project’s workspace.',
  runtime_reserved:
    'The orchestrator’s Herdr tab is now bound to another worker.',
  archive_changed:
    'This project was restored or archived again in another tab.',
  project_deleted: 'This project was deleted; deletion cannot be undone.',
  orchestrator_unavailable:
    'The project’s orchestrator is no longer available to restore.',
}

// A readable reason for a refused Restore; Herdr being down is the one
// refusal worth retrying with the same command.
// Only a refusal because Herdr is down (or a request that never got an
// answer) can succeed if the same restore command is sent again; every other
// refusal describes state the retry would meet unchanged (design D7).
export function projectRestoreRetryable(error: unknown) {
  if (!(error instanceof YardApiError)) return true
  // No Yard error body: a proxy or gateway answered instead of Yard.
  if (/^http_5\d\d$/.test(error.code)) return true
  return (
    error.code === 'project_restore_unavailable' &&
    error.reason === 'herdr_unreachable'
  )
}

export function projectRestoreErrorMessage(error: unknown) {
  if (error instanceof YardApiError) {
    if (error.code === 'project_restore_unavailable' && error.reason) {
      return (
        RESTORE_UNAVAILABLE_MESSAGES[
          error.reason as ProjectRestoreUnavailableReason
        ] ?? error.message
      )
    }
    if (error.code === 'project_not_archived') {
      return 'This project is no longer archived.'
    }
  }
  return error instanceof Error ? error.message : 'Project restore failed'
}

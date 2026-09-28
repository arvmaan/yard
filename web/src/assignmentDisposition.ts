import { YardApiError, disposeAssignment } from './api'
import type {
  Assignment,
  DispositionOutcome,
  DisposeAssignmentInput,
  DisposedAssignment,
} from './types'

/** Pre-commit window in which a quick Complete can be undone. */
export const QUICK_COMPLETE_UNDO_MS = 5_000

export function dispositionInput(
  assignment: Assignment,
  outcome: DispositionOutcome,
  endSession: boolean,
  commandId: string,
): DisposeAssignmentInput {
  const input: DisposeAssignmentInput = {
    command_id: commandId,
    actor: 'local-user',
    attempt_id: assignment.attempt.id,
    expected_assignment_version: assignment.version,
    expected_attempt_version: assignment.attempt.version,
    outcome,
    end_session: endSession,
  }
  if (endSession) {
    input.expected_worker_version = assignment.worker.version
    if (assignment.worker.runtime) {
      input.expected_runtime_version = assignment.worker.runtime.version
    }
  }
  return input
}

/**
 * The assignment to send for a disposition the user chose earlier (a quick
 * Complete after its Undo window, or a sheet opened a while ago). The
 * worker and runtime versions only guard `end_session` and change whenever
 * Herdr reports new status, so they are taken from the latest copy. The
 * assignment and attempt versions are what the user decided on, so if
 * those changed the original copy is sent and the server rejects it.
 */
export function withLatestSessionVersions(
  chosen: Assignment,
  latest: Assignment | undefined,
): Assignment {
  if (
    !latest ||
    latest.id !== chosen.id ||
    latest.version !== chosen.version ||
    latest.attempt.id !== chosen.attempt.id ||
    latest.attempt.version !== chosen.attempt.version ||
    latest.worker.id !== chosen.worker.id
  ) {
    return chosen
  }
  return latest
}

/**
 * Identity of one disposition request. Two requests with the same key have
 * identical bodies, so they may safely share a command ID.
 */
export function dispositionKey(
  assignment: Assignment,
  outcome: DispositionOutcome,
  endSession: boolean,
) {
  return [
    assignment.id,
    assignment.attempt.id,
    assignment.version,
    assignment.attempt.version,
    outcome,
    endSession ? 'end' : 'keep',
    endSession ? assignment.worker.version : '',
    endSession ? (assignment.worker.runtime?.version ?? '') : '',
  ].join(':')
}

/**
 * One command ID per request identity, created when the action starts and
 * reused on every retry until the request settles.
 */
export class DispositionCommandIds {
  private readonly ids = new Map<string, string>()
  private readonly create: () => string

  constructor(create: () => string = () => crypto.randomUUID()) {
    this.create = create
  }

  commandIdFor(key: string) {
    let commandId = this.ids.get(key)
    if (!commandId) {
      commandId = this.create()
      this.ids.set(key, commandId)
    }
    return commandId
  }

  settle(key: string) {
    this.ids.delete(key)
  }
}

const ENDED_LIFECYCLES = new Set([
  'completed',
  'cancelled',
  'handed_off',
  'failed',
])

/**
 * Whether a reloaded assignment shows that the requested outcome committed.
 * Only the same outcome counts as success; any other ended state was
 * another action's result.
 */
export function reconcileDisposition(
  reloaded: Assignment | undefined,
  outcome: DispositionOutcome,
): 'committed' | 'other_outcome' | 'open' {
  if (!reloaded) return 'open'
  if (
    outcome === 'completed' &&
    reloaded.lifecycle === 'completed' &&
    reloaded.completion_receipt !== null
  ) {
    return 'committed'
  }
  if (outcome === 'cancelled' && reloaded.lifecycle === 'cancelled') {
    return 'committed'
  }
  return ENDED_LIFECYCLES.has(reloaded.lifecycle) ? 'other_outcome' : 'open'
}

export function otherOutcomeMessage(assignment: Assignment) {
  switch (assignment.lifecycle) {
    case 'completed':
      return 'Already completed by another action.'
    case 'cancelled':
      return 'Already cancelled by another action.'
    case 'handed_off':
      return 'Already handed off to another project.'
    default:
      return 'This assignment already ended.'
  }
}

export type DispositionResult =
  | {
      kind: 'committed'
      assignment: Assignment
      disposed: DisposedAssignment | null
    }
  | { kind: 'other_outcome'; assignment: Assignment; message: string }
  | { kind: 'failed'; code: string | null; message: string }

/**
 * Send one disposition. On any failure the latest assignment is reloaded:
 * the request counts as committed only when the recorded outcome matches the
 * requested one. Otherwise the command ID is kept, so a retry sends the same
 * command and never double-applies.
 */
export async function submitDisposition({
  assignment,
  commands,
  endSession,
  outcome,
  reload,
}: {
  assignment: Assignment
  commands: DispositionCommandIds
  endSession: boolean
  outcome: DispositionOutcome
  reload: () => Promise<Assignment | undefined>
}): Promise<DispositionResult> {
  const key = dispositionKey(assignment, outcome, endSession)
  const commandId = commands.commandIdFor(key)
  try {
    const disposed = await disposeAssignment(
      assignment.project_id,
      assignment.id,
      dispositionInput(assignment, outcome, endSession, commandId),
    )
    commands.settle(key)
    return { kind: 'committed', assignment: disposed.assignment, disposed }
  } catch (caught) {
    const code = caught instanceof YardApiError ? caught.code : null
    const message =
      caught instanceof Error ? caught.message : 'Yard could not end this work'
    const reloaded = await reload().catch(() => undefined)
    const state = reconcileDisposition(reloaded, outcome)
    if (reloaded && state === 'committed') {
      commands.settle(key)
      return { kind: 'committed', assignment: reloaded, disposed: null }
    }
    if (reloaded && state === 'other_outcome') {
      commands.settle(key)
      return {
        kind: 'other_outcome',
        assignment: reloaded,
        message: otherOutcomeMessage(reloaded),
      }
    }
    return { kind: 'failed', code, message }
  }
}

/**
 * The notice shown after a committed disposition. The disposition has
 * already committed, so unfinished background work is reported as status
 * text and never as an error.
 */
export function dispositionNotice(
  outcome: DispositionOutcome,
  endedSession: boolean,
  background: { cleanup_pending: boolean; transcript_pending: boolean } | null =
    null,
) {
  const parts = [
    outcome === 'cancelled'
      ? 'Yard stopped tracking this worker. The agent keeps running until its Herdr tab is closed.'
      : endedSession
        ? 'Completed. Yard ended the session; the agent keeps running until its Herdr tab is closed.'
        : 'Completed. The worker stays available for new work.',
  ]
  if (background?.cleanup_pending) {
    parts.push('Runtime cleanup continues in the background.')
  }
  if (background?.transcript_pending) {
    parts.push('Yard is still saving the transcript.')
  }
  return parts.join(' ')
}

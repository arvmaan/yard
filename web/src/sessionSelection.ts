import type { CoordinationNode, Project, RuntimeSession } from './types'

export const HERDR_SESSION_STORAGE_KEY = 'yard:herdr-session:v1'

export interface SessionRoles {
  projects: number
  workstreams: number
}

export type SessionRoleCounts = ReadonlyMap<string, SessionRoles>

type ProjectLike = Pick<Project, 'runtime'>
type NodeLike = Pick<CoordinationNode, 'kind' | 'worker'>

export function countSessionRoles(
  projects: readonly ProjectLike[],
  coordinationNodes: readonly NodeLike[],
): Map<string, SessionRoles> {
  const counts = new Map<string, SessionRoles>()
  const entry = (session: string) => {
    let roles = counts.get(session)
    if (!roles) {
      roles = { projects: 0, workstreams: 0 }
      counts.set(session, roles)
    }
    return roles
  }
  projects.forEach((project) => {
    const session = project.runtime?.session
    if (session) entry(session).projects += 1
  })
  coordinationNodes.forEach((node) => {
    const session = node.worker?.runtime?.session
    if (node.kind === 'workstream' && session) entry(session).workstreams += 1
  })
  return counts
}

/**
 * Automatic choice: the running session holding the most Yard project
 * runtimes, then Herdr's default session, then the first running session.
 */
export function pickDefaultSession(
  sessions: readonly RuntimeSession[],
  counts: SessionRoleCounts,
): string {
  const running = sessions.filter((session) => session.running)
  let best: RuntimeSession | undefined
  let bestProjects = 0
  running.forEach((session) => {
    const projects = counts.get(session.name)?.projects ?? 0
    if (projects > bestProjects) {
      best = session
      bestProjects = projects
    }
  })
  return (
    best?.name ??
    running.find((session) => session.is_default)?.name ??
    running[0]?.name ??
    ''
  )
}

export interface ResolveSessionInput {
  sessions: readonly RuntimeSession[]
  counts: SessionRoleCounts
  /** The currently selected session ('' when none yet). */
  current: string
  /** A session the user explicitly chose, if any. */
  explicit: string | null
  /**
   * True once project data is known and an automatic choice has been made;
   * a settled running selection is kept instead of being recomputed.
   */
  settled: boolean
}

export function resolveSelectedSession({
  sessions,
  counts,
  current,
  explicit,
  settled,
}: ResolveSessionInput): string {
  const isRunning = (name: string | null) =>
    Boolean(name) &&
    sessions.some((session) => session.name === name && session.running)
  if (explicit && isRunning(explicit)) return explicit
  if (settled && isRunning(current)) return current
  return pickDefaultSession(sessions, counts)
}

function plural(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? '' : 's'}`
}

export function sessionRoleHint(
  session: RuntimeSession,
  counts: SessionRoleCounts,
): string {
  const roles = counts.get(session.name)
  const parts: string[] = []
  if (roles?.projects) parts.push(plural(roles.projects, 'project'))
  if (roles?.workstreams) parts.push(plural(roles.workstreams, 'workstream'))
  if (session.is_default) parts.push('Herdr default')
  return parts.join(', ')
}

export function sessionOptionLabel(
  session: RuntimeSession,
  counts: SessionRoleCounts,
): string {
  const hint = sessionRoleHint(session, counts)
  return `${session.name}${hint ? ` — ${hint}` : ''}${
    session.running ? '' : ' (stopped)'
  }`
}

export function readStoredSession(): string | null {
  try {
    return window.localStorage.getItem(HERDR_SESSION_STORAGE_KEY) || null
  } catch {
    return null
  }
}

export function writeStoredSession(session: string) {
  try {
    if (session) {
      window.localStorage.setItem(HERDR_SESSION_STORAGE_KEY, session)
    } else {
      window.localStorage.removeItem(HERDR_SESSION_STORAGE_KEY)
    }
  } catch {
    // Storage unavailable; the choice still applies for this page.
  }
}

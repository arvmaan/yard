import type {
  Project,
  RuntimeInventory,
  WorkerCandidate,
} from './types'

export interface ProjectTransferSnapshot {
  candidates: WorkerCandidate[]
  inventory: RuntimeInventory
  project: Project
}

export interface ProjectTransferContextEntry {
  error: string | null
  generation: number
  loading: boolean
  snapshot: ProjectTransferSnapshot | null
}

export function emptyProjectTransferContext(): ProjectTransferContextEntry {
  return {
    error: null,
    generation: 0,
    loading: false,
    snapshot: null,
  }
}

export function beginProjectTransferRefresh(
  current: ProjectTransferContextEntry,
  generation: number,
): ProjectTransferContextEntry {
  return {
    ...current,
    generation,
    loading: true,
  }
}

export function resolveProjectTransferRefresh(
  current: ProjectTransferContextEntry,
  generation: number,
  snapshot: ProjectTransferSnapshot,
): {
  entry: ProjectTransferContextEntry
  snapshot: ProjectTransferSnapshot | null
} {
  if (current.generation !== generation) {
    return { entry: current, snapshot: null }
  }

  // Equal observations are safe: inventory may reuse a current snapshot, and
  // the project/worker reads that follow still form one coherent context.
  if (
    current.snapshot &&
    current.snapshot.inventory.observed_at_unix_ms >
      snapshot.inventory.observed_at_unix_ms
  ) {
    return {
      entry: {
        ...current,
        error:
          'Runtime inventory is older than the current transfer snapshot. Wait for a current or newer runtime observation.',
        loading: false,
      },
      snapshot: null,
    }
  }
  return {
    entry: {
      error: null,
      generation,
      loading: false,
      snapshot,
    },
    snapshot,
  }
}

export function failProjectTransferRefresh(
  current: ProjectTransferContextEntry,
  generation: number,
  error: string | null,
): ProjectTransferContextEntry {
  if (current.generation !== generation) return current
  return {
    ...current,
    error,
    loading: false,
    snapshot: error === null ? null : current.snapshot,
  }
}

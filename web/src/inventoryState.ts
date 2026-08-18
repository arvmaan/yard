import type { RuntimeInventory } from './types'

function hasSameJsonValue(
  left: unknown,
  right: unknown,
  ignoredKeys: ReadonlySet<string> = new Set(),
): boolean {
  if (Object.is(left, right)) return true
  if (
    left === null ||
    right === null ||
    typeof left !== 'object' ||
    typeof right !== 'object'
  ) {
    return false
  }
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value, index) =>
        hasSameJsonValue(value, right[index], ignoredKeys),
      )
    )
  }

  const leftRecord = left as Record<string, unknown>
  const rightRecord = right as Record<string, unknown>
  const leftKeys = Object.keys(leftRecord).filter(
    (key) => !ignoredKeys.has(key),
  )
  const rightKeys = Object.keys(rightRecord).filter(
    (key) => !ignoredKeys.has(key),
  )
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every(
      (key) =>
        Object.hasOwn(rightRecord, key) &&
        hasSameJsonValue(
          leftRecord[key],
          rightRecord[key],
          ignoredKeys,
        ),
    )
  )
}

export function reconcileJsonSnapshot<T>(current: T, next: T): T {
  return hasSameJsonValue(current, next) ? current : next
}

const RUNTIME_OBSERVATION_KEYS = new Set(['last_observed_at_unix_ms'])

export function reconcileRuntimeProjectionSnapshot<T>(
  current: T,
  next: T,
): T {
  return hasSameJsonValue(current, next, RUNTIME_OBSERVATION_KEYS)
    ? current
    : next
}

export function reconcileInventorySnapshot(
  current: RuntimeInventory | null,
  next: RuntimeInventory,
): RuntimeInventory {
  if (!current) return next

  const unchanged =
    current.adapter === next.adapter &&
    current.session === next.session &&
    current.runtime_version === next.runtime_version &&
    current.protocol === next.protocol &&
    hasSameJsonValue(current.focus, next.focus) &&
    hasSameJsonValue(current.workspaces, next.workspaces) &&
    hasSameJsonValue(current.tabs, next.tabs) &&
    hasSameJsonValue(current.panes, next.panes) &&
    hasSameJsonValue(current.workers, next.workers) &&
    hasSameJsonValue(current.child_agents, next.child_agents)

  return unchanged ? current : next
}

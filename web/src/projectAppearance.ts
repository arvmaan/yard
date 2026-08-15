export const PROJECT_ACCENTS = [
  { label: 'Harbor', value: '#19766b' },
  { label: 'Cobalt', value: '#3978b8' },
  { label: 'Signal', value: '#d6a324' },
  { label: 'Vermilion', value: '#d94a37' },
  { label: 'Orchid', value: '#8a5e96' },
  { label: 'Moss', value: '#58725f' },
] as const

const PROJECT_ACCENT_STORAGE_KEY = 'yard:project-accents:v1'

function stableHash(value: string) {
  let hash = 2166136261
  for (const character of value) {
    hash ^= character.charCodeAt(0)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

export function defaultProjectAccent(projectId: string) {
  return PROJECT_ACCENTS[stableHash(projectId) % PROJECT_ACCENTS.length].value
}

export function readProjectAccents(): Record<string, string> {
  try {
    const stored = window.localStorage.getItem(PROJECT_ACCENT_STORAGE_KEY)
    if (!stored) return {}
    const parsed = JSON.parse(stored) as Record<string, string>
    const accepted = new Set<string>(
      PROJECT_ACCENTS.map(({ value }) => value),
    )
    return Object.fromEntries(
      Object.entries(parsed).filter(([, value]) => accepted.has(value)),
    )
  } catch {
    return {}
  }
}

export function writeProjectAccents(accents: Record<string, string>) {
  window.localStorage.setItem(
    PROJECT_ACCENT_STORAGE_KEY,
    JSON.stringify(accents),
  )
}

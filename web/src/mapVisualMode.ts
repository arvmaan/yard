export type MapVisualMode = 'depth' | 'flat'

export const MAP_VISUAL_MODE_KEY = 'yard:map-visual-mode:v1'

export function readMapVisualMode(): MapVisualMode {
  try {
    const stored = window.localStorage.getItem(MAP_VISUAL_MODE_KEY)
    return stored === 'flat' || stored === 'depth' ? stored : 'depth'
  } catch {
    return 'depth'
  }
}

export function writeMapVisualMode(mode: MapVisualMode) {
  window.localStorage.setItem(MAP_VISUAL_MODE_KEY, mode)
}

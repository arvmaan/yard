/**
 * Scene model for the projected 2.5D map.
 *
 * Everything in here is expressed in unprojected world coordinates. The
 * projection is applied once, at render time, by `ProjectedMap.tsx`. Keeping
 * the model unprojected means the scene builder in `RuntimeCanvas.tsx` and the
 * hit tests share exactly the same numbers the server persists.
 */

import type { WorldPoint, WorldRect } from './mapProjection'

export type ProjectedRouteState = 'active' | 'failed' | 'idle'

export type ProjectedRouteKind =
  | 'allocation'
  | 'child'
  | 'coordination'
  | 'relationship'

export type ProjectedAnchorKind =
  | 'assigned-worker'
  | 'automation'
  | 'child-agent'
  | 'coordination-node'
  | 'orchestrator'
  | 'worker'
  | 'yard-orchestrator'

export interface ProjectedBuilding {
  colorSeed: number
  completed: boolean
  footprint: WorldRect
  height: number
}

export interface ProjectedTerritory {
  accent: string
  allocationTarget: boolean
  buildings: ProjectedBuilding[]
  kind: 'project' | 'workspace'
  label: string
  nodeId: string
  rect: WorldRect
  runtime: string
  selected: boolean
  status: string
  tokenTotal?: number
}

/**
 * Compact token-count label for a building — real observed usage rounded to
 * the precision a small map label can actually show, not a precise counter.
 * Returns null below 1000 tokens: a bare one- or two-digit number reads as
 * noise on a skyline, not as a meaningful figure.
 */
export function formatTokenCount(total: number): string | null {
  if (total >= 1_000_000) {
    const millions = total / 1_000_000
    return `${millions >= 10 ? Math.round(millions) : Math.round(millions * 10) / 10}M`
  }
  if (total >= 1_000) {
    const thousands = total / 1_000
    return `${thousands >= 10 ? Math.round(thousands) : Math.round(thousands * 10) / 10}K`
  }
  return null
}

export interface ProjectedAnchor {
  accent: string
  kind: ProjectedAnchorKind
  nodeId: string
  point: WorldPoint
  selected: boolean
  status: string
}

export interface ProjectedRoute {
  id: string
  from: WorldPoint
  kind: ProjectedRouteKind
  state: ProjectedRouteState
  to: WorldPoint
}

export interface ProjectedScene {
  anchors: ProjectedAnchor[]
  routes: ProjectedRoute[]
  territories: ProjectedTerritory[]
}

export const EMPTY_SCENE: ProjectedScene = {
  anchors: [],
  routes: [],
  territories: [],
}

/** FNV-1a style hash. Stable across reloads for a given identity string. */
export function stableHash(value: string) {
  let hash = 2166136261
  for (const character of value) {
    hash ^= character.charCodeAt(0)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

const BUILDING_INSET = 0.09
const BUILDING_MAX_HEIGHT = 230
// Narrower than before (was 0.92): still close enough to read as a cluster
// standing shoulder to shoulder, but leaves each footprint's base visibly
// slimmer relative to the height it now reaches, for skyscraper proportions
// rather than blocky ones.
const BUILDING_FOOTPRINT_RATIO = 0.8

/**
 * Square-spiral cell offsets, center outward: index 0 is the center cell,
 * then each ring of 8*k cells surrounds the previous one. Paired with
 * height-descending placement below, this is what makes a skyline's tallest
 * buildings cluster at its core and shorter ones spread outward as more are
 * added — the opposite of a plain row-major grid, which has no center to
 * grow outward from.
 */
function spiralOffsets(count: number): { col: number; row: number }[] {
  const offsets: { col: number; row: number }[] = [{ col: 0, row: 0 }]
  let col = 0
  let row = 0
  let steps = 1
  let direction = 0
  const deltaCol = [1, 0, -1, 0]
  const deltaRow = [0, 1, 0, -1]
  while (offsets.length < count) {
    for (let leg = 0; leg < 2 && offsets.length < count; leg += 1) {
      for (let step = 0; step < steps && offsets.length < count; step += 1) {
        col += deltaCol[direction]
        row += deltaRow[direction]
        offsets.push({ col, row })
      }
      direction = (direction + 1) % 4
    }
    steps += 1
  }
  return offsets
}

/**
 * Deterministic building footprints for a project territory.
 *
 * Reuses the identity hash and the height curve the flat city silhouette used,
 * so a project's skyline is recognisably the same city before and after the
 * move to projected geometry — only the geometry it is expressed in changed.
 * Footprints are laid out on a world-space grid inside the territory, which is
 * what keeps every building standing on the ground plane its territory lies on.
 * Footprints stay narrow relative to height — skyscraper massing, not
 * warehouse massing. `Territory` in ProjectedMap.tsx paints these back-to-
 * front by world depth, which is what keeps a packed, spiraled cluster like
 * this one from having a tall building blot out a shorter neighbor's face.
 */
export function territoryBuildings(
  projectId: string,
  buildingCount: number,
  completedBuildingCount: number,
  rect: WorldRect,
): ProjectedBuilding[] {
  if (buildingCount <= 0) return []
  const insetX = rect.width * BUILDING_INSET
  const insetY = rect.height * BUILDING_INSET
  const fieldWidth = Math.max(1, rect.width - insetX * 2)
  const fieldHeight = Math.max(1, rect.height - insetY * 2)

  const buildings = Array.from({ length: buildingCount }, (_, index) => {
    const hash = stableHash(`${projectId}:building:${index}`)
    const completed =
      index >= Math.max(1, buildingCount - completedBuildingCount)
    const height = Math.min(
      BUILDING_MAX_HEIGHT,
      70 +
        (hash % 110) +
        Math.round(buildingCount * 2.4) +
        (completed ? 32 : 0),
    )
    const widthRatio = BUILDING_FOOTPRINT_RATIO - ((hash >>> 8) % 10) / 100
    const depthRatio = BUILDING_FOOTPRINT_RATIO - ((hash >>> 14) % 10) / 100
    // Seeds a small per-building tint variance in ProjectedMap.tsx, so a
    // skyline reads as individually-colored buildings in related tones of
    // the project's accent rather than one flat repeated color.
    const colorSeed = (hash >>> 20) % 100
    return { colorSeed, completed, depthRatio, height, widthRatio }
  })

  const tallestFirst = [...buildings].sort((a, b) => b.height - a.height)
  const positions = spiralOffsets(buildingCount)
  const cols = positions.map((point) => point.col)
  const rows = positions.map((point) => point.row)
  const minCol = Math.min(...cols)
  const maxCol = Math.max(...cols)
  const minRow = Math.min(...rows)
  const maxRow = Math.max(...rows)

  // Positions ordered front-to-back, not by spiral generation order: world
  // depth here is (col + row) — the same combination the projection's v-axis
  // uses — so the position with the highest col+row is the one closest to
  // the camera, with nothing else in the cluster in front of it. Pairing
  // that with the tallest building first (below) means the tallest building
  // always lands where nothing can paint over it. Placing it at the spiral's
  // geometric center instead — the first version of this — surrounded it
  // with touching neighbors on every side, and neighbors positioned in front
  // painted over most of its side face: pixel-sampled, roughly two-thirds of
  // the tallest building's own side color was replaced by a neighbor's top
  // face. A shorter building losing part of its face to something taller in
  // front of it reads as normal skyline occlusion; a tall building
  // half-erased by a short building in front of it reads as broken.
  const frontToBack = [...positions].sort(
    (a, b) => b.col + b.row - (a.col + a.row),
  )

  // A fixed cell span rather than one sized to fit exactly buildingCount
  // cells: the spiral needs equal room to grow in every direction from its
  // center, and constraining the cell size to the field's smaller dimension
  // is what keeps buildings touching as the count grows, instead of the
  // spiral's footprint just expanding to fill whatever space exists.
  const span = Math.max(maxCol - minCol + 1, maxRow - minRow + 1, 1)
  const cellSize = Math.min(fieldWidth, fieldHeight) / span
  const clusterWidth = (maxCol - minCol + 1) * cellSize
  const clusterHeight = (maxRow - minRow + 1) * cellSize
  const originX = rect.x + insetX + (fieldWidth - clusterWidth) / 2
  const originY = rect.y + insetY + (fieldHeight - clusterHeight) / 2

  return tallestFirst.map((building, rank) => {
    const { col, row } = frontToBack[rank]
    const footprintWidth = cellSize * building.widthRatio
    const footprintHeight = cellSize * building.depthRatio
    const cellX = originX + (col - minCol) * cellSize
    const cellY = originY + (row - minRow) * cellSize
    return {
      colorSeed: building.colorSeed,
      completed: building.completed,
      footprint: {
        x: cellX + (cellSize - footprintWidth) / 2,
        y: cellY + (cellSize - footprintHeight) / 2,
        width: footprintWidth,
        height: footprintHeight,
      },
      height: building.height,
    }
  })
}

/**
 * A route across the ground plane, expressed as world-space waypoints that run
 * along the plane's own axes. Projecting these gives segments that lie on the
 * same diagonals as the territories, rather than flat screen-space curves.
 */
export function groundRoutePoints(
  from: WorldPoint,
  to: WorldPoint,
): WorldPoint[] {
  const midpoint = { x: to.x, y: from.y }
  if (
    Math.abs(to.x - from.x) < 0.5 ||
    Math.abs(to.y - from.y) < 0.5
  ) {
    return [from, to]
  }
  return [from, midpoint, to]
}

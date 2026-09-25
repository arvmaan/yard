/**
 * Scene model for the projected 2.5D map.
 *
 * Everything in here is expressed in unprojected world coordinates. The
 * projection is applied once, at render time, by `ProjectedMap.tsx`. Keeping
 * the model unprojected means the scene builder in `RuntimeCanvas.tsx` and the
 * hit tests share exactly the same numbers the server persists.
 */

import type { WorldPoint, WorldRect } from './mapProjection'
import type {
  ArchitectureEcosystem,
  ArchitectureNodeKind,
  ArchitectureRepositoryStatus,
  RepositoryArchitecture,
} from './types'

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
  ecosystem: ArchitectureEcosystem
  footprint: WorldRect
  height: number
  id: string
  kind: ArchitectureNodeKind
  manifestPath: string
  name: string
}

export interface ProjectedDistrict {
  error: string | null
  id: string
  label: string
  rect: WorldRect
  status: ArchitectureRepositoryStatus
  truncated: boolean
}

export interface ProjectedTerritory {
  accent: string
  allocationTarget: boolean
  architectureScannedAt?: number
  architectureStale?: boolean
  architectureTruncated?: boolean
  buildings: ProjectedBuilding[]
  districts: ProjectedDistrict[]
  emptyLabel?: string
  kind: 'project' | 'workspace'
  label: string
  nodeId: string
  rect: WorldRect
  runtime: string
  selected: boolean
  status: string
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

/**
 * Project architecture laid out as one district per linked repository and one
 * building per detected manifest node. A building's geometry depends only on
 * its stable node identity and district rectangle, so adding another node does
 * not move existing buildings.
 */
export function architectureLayout(
  repositories: RepositoryArchitecture[],
  rect: WorldRect,
): { buildings: ProjectedBuilding[]; districts: ProjectedDistrict[] } {
  const sorted = [...repositories].sort((a, b) =>
    a.repository_id.localeCompare(b.repository_id),
  )
  if (sorted.length === 0) return { buildings: [], districts: [] }
  const inset = Math.max(12, Math.min(rect.width, rect.height) * 0.06)
  const gap = Math.max(8, Math.min(rect.width, rect.height) * 0.025)
  const columns = Math.ceil(Math.sqrt(sorted.length))
  const rows = Math.ceil(sorted.length / columns)
  const width = Math.max(1, (rect.width - inset * 2 - gap * (columns - 1)) / columns)
  const height = Math.max(1, (rect.height - inset * 2 - gap * (rows - 1)) / rows)
  const districts = sorted.map((repository, index): ProjectedDistrict => ({
    error: repository.errors[0] ?? null,
    id: repository.repository_id,
    label: repository.name,
    rect: {
      x: rect.x + inset + (index % columns) * (width + gap),
      y: rect.y + inset + Math.floor(index / columns) * (height + gap),
      width,
      height,
    },
    status: repository.status,
    truncated: repository.truncated,
  }))
  const buildings = sorted.flatMap((repository, index) => {
    const district = districts[index].rect
    const labelBand = Math.min(28, district.height * 0.2)
    const usableHeight = Math.max(1, district.height - labelBand)
    const footprintBase = Math.max(
      10,
      Math.min(30, Math.min(district.width, usableHeight) * 0.12),
    )
    return [...repository.nodes]
      .sort((a, b) => a.id.localeCompare(b.id))
      .map((node): ProjectedBuilding => {
        const hash = stableHash(node.id)
        const widthRatio = 0.72 + ((hash >>> 8) % 18) / 100
        const depthRatio = 0.72 + ((hash >>> 14) % 18) / 100
        const footprintWidth = footprintBase * widthRatio
        const footprintHeight = footprintBase * depthRatio
        const xRange = Math.max(0, district.width - footprintWidth)
        const yRange = Math.max(0, usableHeight - footprintHeight)
        return {
          colorSeed: (hash >>> 20) % 100,
          ecosystem: node.ecosystem,
          footprint: {
            x: district.x + ((hash & 0xffff) / 0xffff) * xRange,
            y:
              district.y +
              labelBand +
              (((hash >>> 16) & 0xffff) / 0xffff) * yRange,
            width: footprintWidth,
            height: footprintHeight,
          },
          height:
            44 +
            (hash % 76) +
            (node.kind === 'application' || node.kind === 'service' ? 24 : 0),
          id: node.id,
          kind: node.kind,
          manifestPath: node.manifest_path,
          name: node.name,
        }
      })
  })
  return { buildings, districts }
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

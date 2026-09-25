import { describe, expect, it } from 'vitest'
import { architectureLayout } from './mapScene'
import type { ArchitectureNode, RepositoryArchitecture } from './types'

const rect = { x: 20, y: 30, width: 900, height: 600 }

function node(id: string): ArchitectureNode {
  return {
    ecosystem: 'npm',
    id: `repo-1:npm:${id}/package.json`,
    kind: 'package',
    manifest_path: `${id}/package.json`,
    name: id,
  }
}

function repository(nodes: ArchitectureNode[]): RepositoryArchitecture {
  return {
    edges: [],
    errors: [],
    name: 'workspace',
    nodes,
    repository_id: 'repo-1',
    status: 'ready',
    truncated: false,
  }
}

describe('architecture layout', () => {
  it('keeps existing buildings stable across order changes and additions', () => {
    const original = architectureLayout(
      [repository([node('alpha'), node('beta')])],
      rect,
    )
    const expanded = architectureLayout(
      [repository([node('gamma'), node('beta'), node('alpha')])],
      rect,
    )
    const footprints = new Map(
      expanded.buildings.map((building) => [
        building.id,
        building.footprint,
      ]),
    )

    for (const building of original.buildings) {
      expect(footprints.get(building.id)).toEqual(building.footprint)
    }
  })

  it('keeps every building footprint inside its repository district', () => {
    const layout = architectureLayout(
      [repository(Array.from({ length: 50 }, (_, index) => node(`p${index}`)))],
      rect,
    )
    const district = layout.districts[0].rect

    for (const building of layout.buildings) {
      expect(building.footprint.x).toBeGreaterThanOrEqual(district.x)
      expect(building.footprint.y).toBeGreaterThanOrEqual(district.y)
      expect(
        building.footprint.x + building.footprint.width,
      ).toBeLessThanOrEqual(district.x + district.width)
      expect(
        building.footprint.y + building.footprint.height,
      ).toBeLessThanOrEqual(district.y + district.height)
    }
  })

  it('lays out at least 500 nodes within a generous bound', () => {
    const nodes = Array.from({ length: 500 }, (_, index) => node(`p${index}`))
    const started = performance.now()
    const layout = architectureLayout([repository(nodes)], rect)
    const elapsed = performance.now() - started

    expect(layout.buildings).toHaveLength(500)
    expect(elapsed).toBeLessThan(1_000)
  })
})

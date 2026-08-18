import { describe, expect, it } from 'vitest'
import {
  MAP_PROJECTION,
  PROJECTION_SCALE_X,
  PROJECTION_SCALE_Y,
  boundsOfPoints,
  createProjection,
  groundAxisAngles,
  mergeBounds,
  polygonPoints,
  projectBounds,
  projectDelta,
  projectPoint,
  projectPolygon,
  projectRect,
  rectCorners,
  unprojectDelta,
  unprojectPoint,
} from './mapProjection'

const WORLD_POINTS = [
  { x: 0, y: 0 },
  { x: 1, y: 0 },
  { x: 0, y: 1 },
  { x: 210, y: 88 },
  { x: -412.5, y: 973.25 },
  { x: 12345.678, y: -8765.432 },
]

describe('projection constants', () => {
  it('exposes the planned starting scales', () => {
    expect(PROJECTION_SCALE_X).toBe(0.82)
    expect(PROJECTION_SCALE_Y).toBe(0.44)
    expect(MAP_PROJECTION.scaleX).toBe(PROJECTION_SCALE_X)
    expect(MAP_PROJECTION.scaleY).toBe(PROJECTION_SCALE_Y)
  })

  it('keeps a fixed origin across repeated calls', () => {
    // The origin must never be derived from live scene bounds, or the whole
    // world would shift under the pointer during a drag.
    const first = projectPoint({ x: 100, y: 40 })
    for (let index = 0; index < 32; index += 1) {
      projectPoint({ x: index * 37, y: index * -11 })
    }
    expect(projectPoint({ x: 100, y: 40 })).toEqual(first)
    expect(MAP_PROJECTION.originX).toBe(0)
    expect(MAP_PROJECTION.originY).toBe(0)
  })

  it('rejects a projection whose inverse would divide by zero', () => {
    expect(() =>
      createProjection({ originX: 0, originY: 0, scaleX: 0, scaleY: 0.44 }),
    ).toThrow(/positive scales/)
    expect(() =>
      createProjection({ originX: 0, originY: 0, scaleX: 0.82, scaleY: -1 }),
    ).toThrow(/positive scales/)
    expect(() =>
      createProjection({
        originX: Number.NaN,
        originY: 0,
        scaleX: 0.82,
        scaleY: 0.44,
      }),
    ).toThrow(/finite origin/)
  })
})

describe('projectPoint / unprojectPoint', () => {
  it('matches the stated forward transform', () => {
    expect(projectPoint({ x: 100, y: 40 })).toEqual({
      x: 0.82 * 60,
      y: 0.44 * 140,
    })
  })

  it('round trips every sampled world point', () => {
    for (const point of WORLD_POINTS) {
      const roundTripped = unprojectPoint(projectPoint(point))
      expect(roundTripped.x).toBeCloseTo(point.x, 9)
      expect(roundTripped.y).toBeCloseTo(point.y, 9)
    }
  })

  it('round trips through a shifted, differently scaled projection', () => {
    const projection = createProjection({
      originX: -137.5,
      originY: 620.25,
      scaleX: 1.31,
      scaleY: 0.29,
    })
    for (const point of WORLD_POINTS) {
      const roundTripped = unprojectPoint(
        projectPoint(point, projection),
        projection,
      )
      expect(roundTripped.x).toBeCloseTo(point.x, 9)
      expect(roundTripped.y).toBeCloseTo(point.y, 9)
    }
  })

  it('sends the world axes to distinct diagonal screen directions', () => {
    const origin = projectPoint({ x: 0, y: 0 })
    const alongX = projectPoint({ x: 1, y: 0 })
    const alongY = projectPoint({ x: 0, y: 1 })
    expect(alongX.x - origin.x).toBeGreaterThan(0)
    expect(alongX.y - origin.y).toBeGreaterThan(0)
    expect(alongY.x - origin.x).toBeLessThan(0)
    expect(alongY.y - origin.y).toBeGreaterThan(0)
  })
})

describe('projectDelta / unprojectDelta', () => {
  it('is origin independent', () => {
    const shifted = createProjection({
      originX: 913.5,
      originY: -240.25,
      scaleX: PROJECTION_SCALE_X,
      scaleY: PROJECTION_SCALE_Y,
    })
    expect(projectDelta({ x: 31, y: -17 }, shifted)).toEqual(
      projectDelta({ x: 31, y: -17 }),
    )
    expect(unprojectDelta({ x: 31, y: -17 }, shifted)).toEqual(
      unprojectDelta({ x: 31, y: -17 }),
    )
  })

  it('agrees with the difference of two projected points', () => {
    const from = { x: 210, y: 88 }
    const to = { x: 361.5, y: -14.25 }
    const projectedDifference = {
      x: projectPoint(to).x - projectPoint(from).x,
      y: projectPoint(to).y - projectPoint(from).y,
    }
    const delta = projectDelta({ x: to.x - from.x, y: to.y - from.y })
    expect(delta.x).toBeCloseTo(projectedDifference.x, 9)
    expect(delta.y).toBeCloseTo(projectedDifference.y, 9)
  })

  it('round trips deltas in both directions', () => {
    for (const delta of WORLD_POINTS) {
      const forward = unprojectDelta(projectDelta(delta))
      expect(forward.x).toBeCloseTo(delta.x, 9)
      expect(forward.y).toBeCloseTo(delta.y, 9)
      const backward = projectDelta(unprojectDelta(delta))
      expect(backward.x).toBeCloseTo(delta.x, 9)
      expect(backward.y).toBeCloseTo(delta.y, 9)
    }
  })

  it('turns a purely horizontal screen drag into movement on both world axes', () => {
    // This is the interaction contract for projected dragging: pushing the
    // pointer sideways slides the node along both diagonal ground axes.
    const world = unprojectDelta({ x: 120, y: 0 })
    expect(world.x).toBeCloseTo(0.5 * (120 / 0.82), 9)
    expect(world.y).toBeCloseTo(-0.5 * (120 / 0.82), 9)
    expect(world.x).toBeGreaterThan(0)
    expect(world.y).toBeLessThan(0)
  })

  it('turns a purely vertical screen drag into movement on both world axes', () => {
    const world = unprojectDelta({ x: 0, y: 70 })
    expect(world.x).toBeCloseTo(0.5 * (70 / 0.44), 9)
    expect(world.y).toBeCloseTo(0.5 * (70 / 0.44), 9)
  })
})

describe('polygons and bounds', () => {
  const rect = { x: 100, y: 40, width: 200, height: 120 }

  it('emits rectangle corners north, east, south, west', () => {
    expect(rectCorners(rect)).toEqual([
      { x: 100, y: 40 },
      { x: 300, y: 40 },
      { x: 300, y: 160 },
      { x: 100, y: 160 },
    ])
  })

  it('projects a rectangle to a non-axis-aligned parallelogram', () => {
    const corners = projectRect(rect)
    const expected = [
      { x: 0.82 * 60, y: 0.44 * 140 },
      { x: 0.82 * 260, y: 0.44 * 340 },
      { x: 0.82 * 140, y: 0.44 * 460 },
      { x: 0.82 * -60, y: 0.44 * 260 },
    ]
    corners.forEach((corner, index) => {
      expect(corner.x).toBeCloseTo(expected[index].x, 9)
      expect(corner.y).toBeCloseTo(expected[index].y, 9)
    })
    // No edge of the projected shape is axis aligned.
    for (let index = 0; index < corners.length; index += 1) {
      const from = corners[index]
      const to = corners[(index + 1) % corners.length]
      expect(from.x).not.toBeCloseTo(to.x, 6)
      expect(from.y).not.toBeCloseTo(to.y, 6)
    }
  })

  it('keeps opposite edges parallel', () => {
    const [north, east, south, west] = projectRect(rect)
    expect(east.x - north.x).toBeCloseTo(south.x - west.x, 9)
    expect(east.y - north.y).toBeCloseTo(south.y - west.y, 9)
  })

  it('maps projectPolygon through projectPoint', () => {
    const points = [
      { x: 1, y: 2 },
      { x: -3, y: 4 },
    ]
    expect(projectPolygon(points)).toEqual(points.map((p) => projectPoint(p)))
  })

  it('matches a hand computed envelope', () => {
    // Corners project to u in {49.2, 213.2, 114.8, -49.2} and
    // v in {61.6, 149.6, 202.4, 114.4}.
    const bounds = projectBounds(rect)
    expect(bounds.x).toBeCloseTo(-49.2, 6)
    expect(bounds.y).toBeCloseTo(61.6, 6)
    expect(bounds.width).toBeCloseTo(213.2 - -49.2, 6)
    expect(bounds.height).toBeCloseTo(202.4 - 61.6, 6)
  })

  it('returns an empty envelope for no points', () => {
    expect(boundsOfPoints([])).toEqual({
      x: 0,
      y: 0,
      width: 0,
      height: 0,
    })
  })

  it('merges envelopes', () => {
    expect(
      mergeBounds([
        { x: 0, y: 0, width: 10, height: 10 },
        { x: -5, y: 20, width: 10, height: 5 },
      ]),
    ).toEqual({ x: -5, y: 0, width: 15, height: 25 })
  })

  it('renders SVG polygon points', () => {
    expect(polygonPoints(projectRect(rect))).toBe(
      '49.2,61.6 213.2,149.6 114.8,202.4 -49.2,114.4',
    )
  })
})

describe('groundAxisAngles', () => {
  it('derives the ground texture angles from the projection constants', () => {
    const angles = groundAxisAngles()
    expect(angles.x).toBeCloseTo(28.2174, 3)
    expect(angles.y).toBeCloseTo(180 - 28.2174, 3)
  })
})

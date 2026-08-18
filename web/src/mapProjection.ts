/**
 * Axonometric map projection.
 *
 * This module is the single source of truth for the 2.5D map's spatial model.
 * Persisted placements (`CanvasPlacement`) always stay in unprojected world
 * coordinates; projection is a client rendering and interaction concern only.
 *
 * Forward:
 *   u = originX + scaleX * (x - y)
 *   v = originY + scaleY * (x + y)
 *
 * Inverse:
 *   x = 0.5 * ((u - originX) / scaleX + (v - originY) / scaleY)
 *   y = 0.5 * ((v - originY) / scaleY - (u - originX) / scaleX)
 *
 * Deltas drop the origin terms because a difference cancels the origin.
 *
 * Everything here is pure arithmetic: no React, no DOM, no ReactFlow imports.
 */

export interface MapProjection {
  originX: number
  originY: number
  scaleX: number
  scaleY: number
}

/** A point in unprojected world space (what gets persisted). */
export interface WorldPoint {
  x: number
  y: number
}

/**
 * A point in projected flow space. Structurally identical to `WorldPoint` so it
 * composes directly with ReactFlow's `XYPosition`, but semantically distinct:
 * these are the coordinates ReactFlow's viewport transform is applied to.
 */
export interface FlowPoint {
  x: number
  y: number
}

export interface WorldRect {
  x: number
  y: number
  width: number
  height: number
}

export interface FlowRect {
  x: number
  y: number
  width: number
  height: number
}

/**
 * Starting constants carried forward from the projected-map plan. They have not
 * yet been tuned against the owner's visual reference; keep them here so a
 * tuning pass is a one-line change with unit-test coverage behind it.
 */
export const PROJECTION_SCALE_X = 0.82
export const PROJECTION_SCALE_Y = 0.44

/**
 * The projection origin must be a stable constant, never derived from live
 * scene bounds. Deriving it per frame would shift the whole world under the
 * pointer while dragging.
 */
export const PROJECTION_ORIGIN_X = 0
export const PROJECTION_ORIGIN_Y = 0

export const MAP_PROJECTION: MapProjection = createProjection({
  originX: PROJECTION_ORIGIN_X,
  originY: PROJECTION_ORIGIN_Y,
  scaleX: PROJECTION_SCALE_X,
  scaleY: PROJECTION_SCALE_Y,
})

/**
 * Validates a projection. The inverse divides by both scales, so a zero or
 * negative scale would silently produce `Infinity`/`NaN` placements. Failing
 * loudly here means a future visual-tuning change breaks the unit tests instead
 * of the persisted data.
 */
export function createProjection(projection: MapProjection): MapProjection {
  if (!(projection.scaleX > 0) || !(projection.scaleY > 0)) {
    throw new Error(
      `Map projection needs positive scales, received scaleX=${projection.scaleX} scaleY=${projection.scaleY}`,
    )
  }
  if (
    !Number.isFinite(projection.originX) ||
    !Number.isFinite(projection.originY)
  ) {
    throw new Error('Map projection needs a finite origin')
  }
  return projection
}

export function projectPoint(
  point: WorldPoint,
  projection: MapProjection = MAP_PROJECTION,
): FlowPoint {
  return {
    x: projection.originX + projection.scaleX * (point.x - point.y),
    y: projection.originY + projection.scaleY * (point.x + point.y),
  }
}

export function unprojectPoint(
  point: FlowPoint,
  projection: MapProjection = MAP_PROJECTION,
): WorldPoint {
  const u = (point.x - projection.originX) / projection.scaleX
  const v = (point.y - projection.originY) / projection.scaleY
  return {
    x: 0.5 * (u + v),
    y: 0.5 * (v - u),
  }
}

export function projectDelta(
  delta: WorldPoint,
  projection: MapProjection = MAP_PROJECTION,
): FlowPoint {
  return {
    x: projection.scaleX * (delta.x - delta.y),
    y: projection.scaleY * (delta.x + delta.y),
  }
}

export function unprojectDelta(
  delta: FlowPoint,
  projection: MapProjection = MAP_PROJECTION,
): WorldPoint {
  const u = delta.x / projection.scaleX
  const v = delta.y / projection.scaleY
  return {
    x: 0.5 * (u + v),
    y: 0.5 * (v - u),
  }
}

export function projectPolygon(
  points: WorldPoint[],
  projection: MapProjection = MAP_PROJECTION,
): FlowPoint[] {
  return points.map((point) => projectPoint(point, projection))
}

/**
 * World rectangle corners in a stable, drawable order:
 * north (x, y) -> east (x + width, y) -> south (x + width, y + height) ->
 * west (x, y + height). In projected space that reads clockwise starting at the
 * top corner of the parallelogram.
 */
export function rectCorners(rect: WorldRect): WorldPoint[] {
  return [
    { x: rect.x, y: rect.y },
    { x: rect.x + rect.width, y: rect.y },
    { x: rect.x + rect.width, y: rect.y + rect.height },
    { x: rect.x, y: rect.y + rect.height },
  ]
}

export function projectRect(
  rect: WorldRect,
  projection: MapProjection = MAP_PROJECTION,
): FlowPoint[] {
  return projectPolygon(rectCorners(rect), projection)
}

/**
 * Axis-aligned envelope of a projected world rectangle, in flow coordinates.
 * `fitView` frames flat node boxes, which no longer match what the user sees
 * once territories are parallelograms; this is what `fitBounds` needs instead.
 */
export function projectBounds(
  rect: WorldRect,
  projection: MapProjection = MAP_PROJECTION,
): FlowRect {
  return boundsOfPoints(projectRect(rect, projection))
}

export function boundsOfPoints(points: FlowPoint[]): FlowRect {
  if (points.length === 0) {
    return { x: 0, y: 0, width: 0, height: 0 }
  }
  let minX = Number.POSITIVE_INFINITY
  let minY = Number.POSITIVE_INFINITY
  let maxX = Number.NEGATIVE_INFINITY
  let maxY = Number.NEGATIVE_INFINITY
  for (const point of points) {
    if (point.x < minX) minX = point.x
    if (point.y < minY) minY = point.y
    if (point.x > maxX) maxX = point.x
    if (point.y > maxY) maxY = point.y
  }
  return { x: minX, y: minY, width: maxX - minX, height: maxY - minY }
}

export function mergeBounds(bounds: FlowRect[]): FlowRect {
  return boundsOfPoints(
    bounds.flatMap((rect) => [
      { x: rect.x, y: rect.y },
      { x: rect.x + rect.width, y: rect.y + rect.height },
    ]),
  )
}

/** `points` attribute for an SVG `<polygon>`. */
export function polygonPoints(points: FlowPoint[]) {
  return points
    .map((point) => `${round(point.x)},${round(point.y)}`)
    .join(' ')
}

/**
 * Screen-space angle, in degrees, of a unit step along the world x axis. Used so
 * the ground texture and the projected geometry are derived from one constant
 * set instead of drifting apart.
 */
export function groundAxisAngles(projection: MapProjection = MAP_PROJECTION) {
  const toDegrees = (radians: number) => (radians * 180) / Math.PI
  return {
    x: toDegrees(Math.atan2(projection.scaleY, projection.scaleX)),
    y: toDegrees(Math.atan2(projection.scaleY, -projection.scaleX)),
  }
}

function round(value: number) {
  return Math.round(value * 1000) / 1000
}

import {
  Fragment,
  useMemo,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import {
  MAP_PROJECTION,
  boundsOfPoints,
  groundAxisAngles,
  polygonPoints,
  projectPoint,
  projectRect,
  rectCorners,
  type FlowPoint,
  type WorldPoint,
  type WorldRect,
} from './mapProjection'
import type {
  ProjectedAnchor,
  ProjectedBuilding,
  ProjectedScene,
  ProjectedTerritory,
} from './mapScene'
import { formatTokenCount, groundRoutePoints } from './mapScene'

interface ProjectedMapProps {
  scene: ProjectedScene
  onGroundContextMenu: (event: ReactMouseEvent<SVGElement>) => void
  onTerritoryPointerDown: (
    nodeId: string,
    event: ReactPointerEvent<SVGElement>,
  ) => void
}

const GROUND_PADDING = 460
const GRID_STEP = 120
const GRID_MAJOR_EVERY = 5
const ANCHOR_RADIUS = 21

function pathFrom(points: FlowPoint[]) {
  return points
    .map(
      (point, index) =>
        `${index === 0 ? 'M' : 'L'}${round(point.x)} ${round(point.y)}`,
    )
    .join(' ')
}

function pointAlongRoute(points: FlowPoint[], progress: number) {
  const segments = points.slice(1).map((point, index) => {
    const from = points[index]
    return {
      from,
      length: Math.hypot(point.x - from.x, point.y - from.y),
      to: point,
    }
  })
  const totalLength = segments.reduce(
    (sum, segment) => sum + segment.length,
    0,
  )
  let remaining = totalLength * Math.min(1, Math.max(0, progress))
  for (const segment of segments) {
    if (remaining <= segment.length) {
      const ratio = segment.length === 0 ? 0 : remaining / segment.length
      return {
        x: segment.from.x + (segment.to.x - segment.from.x) * ratio,
        y: segment.from.y + (segment.to.y - segment.from.y) * ratio,
      }
    }
    remaining -= segment.length
  }
  return points.at(-1) ?? { x: 0, y: 0 }
}

function round(value: number) {
  return Math.round(value * 100) / 100
}

function raise(point: FlowPoint, height: number): FlowPoint {
  return { x: point.x, y: point.y - height }
}

/**
 * Ground grid drawn in world space and projected, so its lines are the world
 * axes themselves rather than a decorative diagonal texture. Every visible line
 * answers "which world coordinate is this?".
 */
function GroundGrid({ world }: { world: WorldRect }) {
  const lines = useMemo(() => {
    const startX = Math.floor(world.x / GRID_STEP) * GRID_STEP
    const endX = Math.ceil((world.x + world.width) / GRID_STEP) * GRID_STEP
    const startY = Math.floor(world.y / GRID_STEP) * GRID_STEP
    const endY = Math.ceil((world.y + world.height) / GRID_STEP) * GRID_STEP
    const result: { d: string; key: string; major: boolean }[] = []
    for (let x = startX; x <= endX; x += GRID_STEP) {
      result.push({
        d: pathFrom([
          projectPoint({ x, y: startY }),
          projectPoint({ x, y: endY }),
        ]),
        key: `x${x}`,
        major: Math.round(x / GRID_STEP) % GRID_MAJOR_EVERY === 0,
      })
    }
    for (let y = startY; y <= endY; y += GRID_STEP) {
      result.push({
        d: pathFrom([
          projectPoint({ x: startX, y }),
          projectPoint({ x: endX, y }),
        ]),
        key: `y${y}`,
        major: Math.round(y / GRID_STEP) % GRID_MAJOR_EVERY === 0,
      })
    }
    return result
  }, [world])

  return (
    <g className="projected-ground-grid">
      {lines.map((line) => (
        <path
          className={`projected-ground-grid__line ${line.major ? 'is-major' : ''}`}
          d={line.d}
          key={line.key}
        />
      ))}
    </g>
  )
}

const COMPLETED_PLATE_COUNT = 4
const COMPLETED_PLATE_GAP = 3

/**
 * One extruded slab: a flat top face plus a single visible side face.
 *
 * The reference draws structures with exactly two faces — never a second side
 * face — because the whole scene shares one axonometric angle in which only the
 * `+x` face is camera facing. The side face carries a diagonal hatch so the
 * scene reads as an engraved schematic rather than shaded 3D.
 */
function Slab({
  base,
  height,
  lift,
}: {
  base: FlowPoint[]
  height: number
  lift: number
}) {
  const ground = base.map((point) => raise(point, lift))
  const top = ground.map((point) => raise(point, height))
  // The south corner of a world rectangle is nearest the viewer, so both
  // faces meeting at it — east-to-south and south-to-west — are camera
  // facing; between them they ground the entire visible half of the
  // footprint's perimeter. Drawing only the east-south face (the original
  // version of this) left the south-west edge with no wall connecting its
  // top corner down to the ground at all, which read as that whole side of
  // the building floating over bare ground rather than standing on it.
  const [, east, south, west] = ground
  const [, topEast, topSouth, topWest] = top
  const sideEast = polygonPoints([east, south, topSouth, topEast])
  const sideWest = polygonPoints([south, west, topWest, topSouth])
  return (
    <>
      <polygon className="projected-building__face--side-b" points={sideWest} />
      <polygon className="projected-building__hatch" points={sideWest} />
      <polygon className="projected-building__face--side" points={sideEast} />
      <polygon className="projected-building__hatch" points={sideEast} />
      <polygon
        className="projected-building__face--top"
        points={polygonPoints(top)}
      />
    </>
  )
}

function Building({
  building,
  accent,
}: {
  accent: string
  building: ProjectedBuilding
}) {
  const base = projectRect(building.footprint)
  // Completed work accumulates: an assignment with a completion receipt is
  // drawn as a stack of thin layered plates, the way the reference marks its
  // most accumulated component, while in-progress work stays a single block.
  const plates = building.completed
    ? Array.from({ length: COMPLETED_PLATE_COUNT }, (_, index) => {
        const plateHeight = Math.max(
          3,
          (building.height -
            COMPLETED_PLATE_GAP * (COMPLETED_PLATE_COUNT - 1)) /
            COMPLETED_PLATE_COUNT,
        )
        return {
          height: plateHeight,
          lift: index * (plateHeight + COMPLETED_PLATE_GAP),
        }
      })
    : [{ height: building.height, lift: 0 }]

  // A small per-building tint of the project's own accent — lighter or
  // darker by a hash-driven amount — so a skyline reads as individually
  // colored buildings in related purples rather than one flat repeated
  // color. Nested inside the color-mix calls each face already does for its
  // own top/side tone, so the variance carries through everywhere the base
  // accent would have been used.
  const lighten = building.colorSeed % 2 === 0
  const shiftAmount = 4 + (building.colorSeed % 10)
  const tintedAccent = `color-mix(in srgb, ${accent} ${100 - shiftAmount}%, ${
    lighten ? 'white' : 'black'
  })`

  return (
    <g
      className={`projected-building ${building.completed ? 'is-completed' : ''}`}
      style={{ '--project-accent': tintedAccent } as React.CSSProperties}
    >
      {plates.map((plate, index) => (
        <Slab
          base={base}
          height={plate.height}
          key={index}
          lift={plate.lift}
        />
      ))}
    </g>
  )
}

/**
 * Compact real-usage label on the skyline's tallest building — real observed
 * token counts (see RuntimeCanvas.tsx's projectTokenTotal, summed from each
 * assigned worker's own Herdr-reported usage), not a decorative number.
 */
function TokenLabel({
  building,
  formatted,
}: {
  building: ProjectedBuilding
  formatted: string
}) {
  const base = projectRect(building.footprint)
  const roof = base.map((point) => raise(point, building.height))
  const centre = {
    x: roof.reduce((sum, point) => sum + point.x, 0) / roof.length,
    y: roof.reduce((sum, point) => sum + point.y, 0) / roof.length,
  }
  return (
    <text
      className="projected-building__token-label"
      textAnchor="middle"
      x={round(centre.x)}
      y={round(centre.y - 10)}
    >
      {formatted}
    </text>
  )
}

function Territory({
  territory,
  onPointerDown,
}: {
  onPointerDown: (
    nodeId: string,
    event: ReactPointerEvent<SVGElement>,
  ) => void
  territory: ProjectedTerritory
}) {
  const outline = projectRect(territory.rect)
  return (
    <g
      className={`projected-territory projected-territory--${territory.kind} ${
        territory.selected ? 'is-selected' : ''
      } ${territory.allocationTarget ? 'is-allocation-target' : ''}`}
      data-node-id={territory.nodeId}
      data-runtime={territory.runtime}
      data-status={territory.status}
      style={{ '--project-accent': territory.accent } as React.CSSProperties}
    >
      <polygon
        aria-label={`${territory.label} territory`}
        className="territory-polygon"
        onPointerDown={(event) => onPointerDown(territory.nodeId, event)}
        points={polygonPoints(outline)}
      />
      <g className="projected-territory__buildings">
        {/* Painted back-to-front by world depth (x+y, the same combination
            the projection's v-axis uses), not grid index. Buildings are now
            tall enough to visually overlap their neighbors on screen even
            though their footprints don't overlap in world space; without
            this sort a "front" building's top face can paint over a
            "back" building's side face, making it look like it has none. */}
        {[...territory.buildings]
          .sort(
            (a, b) =>
              a.footprint.x + a.footprint.y - (b.footprint.x + b.footprint.y),
          )
          .map((building, index) => (
            <Building
              accent={territory.accent}
              building={building}
              key={index}
            />
          ))}
      </g>
      {(() => {
        if (territory.buildings.length === 0) return null
        const formatted = formatTokenCount(territory.tokenTotal ?? 0)
        if (!formatted) return null
        const tallest = territory.buildings.reduce((max, building) =>
          building.height > max.height ? building : max,
        )
        return <TokenLabel building={tallest} formatted={formatted} />
      })()}
      <polygon
        className="territory-polygon__outline"
        points={polygonPoints(outline)}
      />
    </g>
  )
}

/**
 * Ground anchor for a worker, orchestrator, child agent, automation, or
 * coordination node. The upright marker itself stays an HTML billboard standing
 * on this pad, which is the sprite-ready boundary: when real sprites arrive they
 * replace the billboard and keep this footprint.
 *
 * The pad itself is a soft ground-contact shadow, not a flat accent-tinted
 * rectangle — a colored polygon here read as an unfinished, unshaded flat
 * shape sitting apart from the buildings' engraved treatment. A billboard
 * sprite standing on it can carry its own art-baked shadow; this pad only
 * needs to ground it and carry the selection ring.
 */
function Anchor({ anchor }: { anchor: ProjectedAnchor }) {
  const center = projectPoint(anchor.point)
  return (
    <g
      className={`projected-anchor projected-anchor--${anchor.kind} ${
        anchor.selected ? 'is-selected' : ''
      }`}
      data-node-id={anchor.nodeId}
      data-status={anchor.status}
      style={{ '--project-accent': anchor.accent } as React.CSSProperties}
    >
      <ellipse
        className="projected-anchor__pad"
        cx={round(center.x)}
        cy={round(center.y)}
        rx={ANCHOR_RADIUS}
        ry={ANCHOR_RADIUS * 0.42}
      />
    </g>
  )
}

/**
 * The projected map surface.
 *
 * Mounted through ReactFlow's `<ViewportPortal>`, so it inherits the same
 * pan/zoom transform as the nodes and needs no transform of its own. Everything
 * it draws is derived from world coordinates through `mapProjection`, which is
 * what makes the ground, the territories, the skylines, and the routes one
 * spatial model instead of separate coincidental drawings.
 */
export function ProjectedMap({
  scene,
  onGroundContextMenu,
  onTerritoryPointerDown,
}: ProjectedMapProps) {
  const worldBounds = useMemo(() => {
    const points: WorldPoint[] = [
      ...scene.territories.flatMap((territory) =>
        rectCorners(territory.rect),
      ),
      ...scene.anchors.map((anchor) => anchor.point),
      ...scene.routes.flatMap((route) => [route.from, route.to]),
    ]
    const bounds = boundsOfPoints(points)
    return {
      x: bounds.x - GROUND_PADDING,
      y: bounds.y - GROUND_PADDING,
      width: bounds.width + GROUND_PADDING * 2,
      height: bounds.height + GROUND_PADDING * 2,
    }
  }, [scene])

  const viewBox = useMemo(() => {
    // The SVG viewport is the projected envelope of the world field plus the
    // tallest possible structure, so nothing is clipped at the top edge.
    const bounds = boundsOfPoints(projectRect(worldBounds))
    return {
      x: bounds.x,
      y: bounds.y - 160,
      width: Math.max(1, bounds.width),
      height: Math.max(1, bounds.height + 160),
    }
  }, [worldBounds])

  const groundAngles = groundAxisAngles(MAP_PROJECTION)

  return (
    <svg
      aria-hidden="true"
      className="projected-map"
      data-ground-angle={round(groundAngles.x)}
      height={viewBox.height}
      onContextMenu={onGroundContextMenu}
      style={{
        position: 'absolute',
        left: viewBox.x,
        top: viewBox.y,
        zIndex: -1,
      }}
      viewBox={`${viewBox.x} ${viewBox.y} ${viewBox.width} ${viewBox.height}`}
      width={viewBox.width}
    >
      <defs>
        {/*
          Engraved diagonal hatch for structure side faces. The hatch angle
          follows the projected world-y axis so the texture lies along the same
          plane as everything else in the scene.
        */}
        <pattern
          height="6"
          id="projected-side-hatch"
          patternTransform={`rotate(${round(groundAngles.y)})`}
          patternUnits="userSpaceOnUse"
          width="6"
        >
          <line
            className="projected-building__hatch-line"
            x1="0"
            x2="0"
            y1="0"
            y2="6"
          />
        </pattern>
      </defs>
      <polygon
        className="projected-ground"
        points={polygonPoints(projectRect(worldBounds))}
      />
      <GroundGrid world={worldBounds} />
      <g className="projected-routes">
        {scene.routes.map((route) => {
          // Tracks run along the projected ground axes. The infrastructure
          // stays neutral while the signal and optional train packet carry
          // communication state, so an idle connection remains readable
          // without implying that information is currently moving.
          const waypoints = groundRoutePoints(route.from, route.to).map(
            (point) => projectPoint(point),
          )
          const path = pathFrom(waypoints)
          const signal = pointAlongRoute(waypoints, 0.72)
          return (
            <g
              className={`information-path projected-route projected-route--${route.kind} projected-route--${route.state}`}
              data-route-id={route.id}
              data-state={route.state}
              key={route.id}
            >
              <path className="projected-route__ballast" d={path} />
              <path className="projected-route__sleepers" d={path} />
              <path className="projected-route__rails" d={path} />
              <path className="projected-route__gauge" d={path} />
              {route.state === 'active' ? (
                <path
                  className="projected-route__activity"
                  d={path}
                  pathLength="100"
                />
              ) : null}
              {waypoints.map((point, index) => (
                <circle
                  className="projected-route__junction"
                  cx={round(point.x)}
                  cy={round(point.y)}
                  key={index}
                  r={2.2}
                />
              ))}
              <g
                className="projected-route__signal"
                transform={`translate(${round(signal.x)} ${round(signal.y)})`}
              >
                <circle className="projected-route__signal-housing" r="5.2" />
                <circle className="projected-route__signal-light" r="2.8" />
              </g>
            </g>
          )
        })}
      </g>
      <g className="projected-territories">
        {scene.territories.map((territory) => (
          <Fragment key={territory.nodeId}>
            <Territory
              onPointerDown={onTerritoryPointerDown}
              territory={territory}
            />
          </Fragment>
        ))}
      </g>
      <g className="projected-anchors">
        {scene.anchors.map((anchor) => (
          <Anchor anchor={anchor} key={anchor.nodeId} />
        ))}
      </g>
    </svg>
  )
}

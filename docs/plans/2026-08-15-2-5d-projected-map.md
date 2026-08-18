---
title: Projected 2.5D Map Plan
date: 2026-08-15
status: ready-for-implementation
branch: 2.5d
---

# Projected 2.5D Map Plan

## Product Contract

The map is an operational view first and an RTS-inspired world second.
Projection should make project state, hierarchy, and communication easier to
scan. It must not add decorative motion or hide runtime truth.

In 2.5D mode:

- project territories lie on one projected ground plane;
- project progress appears as structures rising from that territory;
- relationships and communication travel across the same plane;
- map placement and dragging follow the plane's axes;
- labels and future worker sprites may billboard upright;
- controls, inspectors, chat, and terminals remain flat.

Plan mode remains the reliable flat fallback.

## Architecture Decision

Keep server and local-storage placement data in existing world coordinates.
Projection is a client rendering and interaction concern.

Use a dedicated projected scene in 2.5D mode. Mount it through ReactFlow's
`ViewportPortal` so it receives the existing pan and zoom transform. Hide the
default ReactFlow node and edge visuals in 2.5D mode while retaining ReactFlow
for viewport management and Plan mode.

Do not transform `.react-flow__viewport` directly. ReactFlow owns its inline
pan/zoom transform, and adding another transform there makes pointer and
viewport math ambiguous.

Suggested module boundary:

```text
web/src/mapProjection.ts
  projectPoint
  unprojectPoint
  projectDelta
  unprojectDelta
  projectPolygon

web/src/ProjectedMap.tsx
  projected ground
  projected edges
  territories
  structures
  billboard placeholders
  projected pointer interactions

web/src/RuntimeCanvas.tsx
  shared node/edge model
  mode selection
  ReactFlow viewport
  persistence callbacks
```

The exact file split may follow local patterns, but projection math must have
one source of truth.

## Projection Model

Use an affine axonometric transform with explicit scale and origin:

```text
u = originX + scaleX * (x - y)
v = originY + scaleY * (x + y)
```

The inverse is:

```text
x = 0.5 * ((u - originX) / scaleX + (v - originY) / scaleY)
y = 0.5 * ((v - originY) / scaleY - (u - originX) / scaleX)
```

Start near `scaleX = 0.82` and `scaleY = 0.44`, then tune against the visual
reference. Keep the constants centralized.

The origin or pivot must remain stable while dragging. Do not derive it from
live bounds on every pointer move, because that would shift the entire world
under the pointer.

## Render Layers

Render back to front:

1. A neutral projected ground field and low-contrast grid.
2. Project relationships and orchestration routes.
3. Project and unbound-workspace territories.
4. City foundations and buildings.
5. Child-agent and allocation links within territories.
6. Worker, child-agent, and orchestrator ground anchors.
7. Upright billboard placeholders and project labels.
8. Selection, allocation-target, focus, and failure overlays.

Use SVG for projected geometry. It provides precise polygons, paths, hit
regions, theme styling, and deterministic screenshots without adding a 3D
runtime dependency.

## Component Translation

### Projects

Project rectangles in world coordinates become clipped projected territory
polygons. The polygon border carries the project accent. Selection and
allocation-target states must be visible around the projected outline.

Project labels should remain upright and anchored near the territory's front
edge. Do not project label text into an unreadable skew.

### Progress Structures

Generate deterministic building footprints from the existing project ID,
assignment count, artifact count, and completion count. Project each footprint
onto the territory and render:

- a top face aligned to the ground axes;
- two visible side faces;
- vertical screen-space height;
- stronger or taller structures for completed work.

Buildings must stay inside the project territory and must not intercept
pointer events intended for project selection or dragging.

### Connections

Build edge routes in world space, then project every route point. This keeps
segments aligned to the same axes as the territories.

Preserve status semantics:

- idle: gray, static, 80 percent opacity;
- active communication: green, pulsing or flowing;
- failed or ambiguous communication: red;
- reduced motion: static status color with no animation.

### Workers And Orchestrators

Use simple upright billboard placeholders anchored to projected world points.
Do not spend time designing final units before sprites are supplied.

Child agents remain grouped as a visible tree connected to their parent
terminal or agent. Dead child agents remain hidden.

### Coordination And Knowledge Nodes

Workstream orchestrators and knowledge stores should use small projected
foundations plus upright symbols. Their project attachments route across the
ground plane like other coordination edges.

Automations remain satellites attached to their target orchestrator. Their
status and schedule text may remain in an upright compact label.

## Interaction Model

### Dragging

Use custom pointer capture for projected root-node dragging.

On pointer down:

1. record the pointer position;
2. record the node's unprojected world position;
3. record the current viewport zoom;
4. capture the pointer and freeze the projection origin.

On pointer move:

1. divide screen delta by ReactFlow zoom;
2. inverse-project that delta into world delta;
3. update the node's world position;
4. re-render all projected geometry from the updated world position.

On pointer up:

1. release pointer capture;
2. persist through the existing placement callback;
3. retain focus and selection.

For a projected-flow delta `(du, dv)`:

```text
dx = 0.5 * (du / scaleX + dv / scaleY)
dy = 0.5 * (dv / scaleY - du / scaleX)
```

Dragging horizontally on screen should therefore change both world axes. That
is expected and should be tested.

### Hit Testing

All pointer-to-world operations must use the inverse projection:

- project selection;
- allocation drag-over and drop;
- right-click node creation;
- relationship connection targets;
- resize handles;
- projected minimap or fit calculations where applicable.

Use polygon hit regions for territories. Do not leave invisible rectangular
ReactFlow nodes as the active mouse targets when they no longer match the
visible geometry.

### Pan, Zoom, Arrange, And Fit

ReactFlow continues to own viewport pan and zoom. The projected scene inside
`ViewportPortal` follows that transform.

`Arrange spaces` may continue to calculate world placements in a circle, but
the visible result must be projected. Fit calculations need projected bounds;
default ReactFlow node bounds are not sufficient once the visible scene
differs from flat node geometry.

## Implementation Sequence

1. Extract and unit-test forward and inverse projection helpers.
2. Remove the rejected embossed depth CSS while retaining the mode switch.
3. Add a projected ground and grid inside `ViewportPortal`.
4. Render project and workspace territory polygons from world geometry.
5. Render project structures with visible top and side faces.
6. Render relationship, coordination, allocation, and child-agent paths from
   projected route points.
7. Add billboard placeholders for workers and orchestrators.
8. Implement projected project dragging with pointer capture and inverse
   delta mapping.
9. Apply inverse mapping to allocation drop, context-menu creation, and
   connection hit testing.
10. Add projected selection, keyboard focus, resize, arrange, and fit
    behavior.
11. Verify dark/light themes, mobile framing, reduced motion, forced colors,
    and Plan-mode parity.

## Test Plan

Add focused unit coverage for:

- forward/inverse point round trips;
- forward/inverse delta round trips;
- stable projection origin;
- projected rectangle polygon order;
- projected bounds.

Replace the rejected Playwright geometry assertion with checks that:

- a project territory is a non-axis-aligned projected polygon;
- structures share the territory's projected axes;
- connection paths terminate at projected anchors;
- a screen-space drag persists the expected inverse-mapped world delta;
- reloading preserves the resulting world placement;
- toggling Plan/2.5D does not mutate persisted placement;
- allocation drop selects the visibly targeted projected territory;
- right-click creation stores the inverse-mapped world point;
- project selection does not hide or block child agents;
- selected and allocation-target outlines remain visible;
- active, idle, and failed connection styling remains distinct;
- keyboard selection and movement remain usable;
- light, dark, desktop, and mobile screenshots are nonblank and framed.

Run the complete gates before requesting owner review:

```bash
cargo test --workspace
cd web
npm run lint
npm run build
npm run test:e2e
```

Also run `git diff --check` and inspect Playwright screenshots at desktop and
mobile sizes.

## Acceptance Gate

The slice is ready for owner review only when:

- every visible map component shares the projected spatial model;
- project dragging visibly and persistently follows inverse projection;
- no project or edge is merely a flat card over an isometric background;
- Plan mode retains current behavior;
- workers and orchestrators have clean sprite-ready anchor boundaries;
- full automated verification is green;
- an owner live smoke confirms the map feels coherent and responsive.

Automated evidence is not a manual Yard receipt and must not be described as
owner acceptance.

## Out Of Scope

- final worker or orchestrator sprite art;
- WebGL, Three.js, or a general 3D engine;
- changing server placement schemas;
- decorative camera rotation;
- simulated activity that is not backed by runtime state;
- unrelated terminal, chat, or orchestration features.

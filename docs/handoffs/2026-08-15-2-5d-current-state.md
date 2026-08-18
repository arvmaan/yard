---
title: 2.5D Map Current-State Handoff
date: 2026-08-15
status: rejected-spike
branch: 2.5d
---

# 2.5D Map Current-State Handoff

## Objective

Move Yard from a flat node canvas toward an RTS-inspired operational map.
Projects should read as cities on one projected ground plane. Their buildings,
relationships, workers, orchestrators, and drag behavior must belong to that
same spatial model.

The visual reference is available locally at `~/IMG_7308.jpg`. It shows an
axonometric system map with:

- one coherent projected plane;
- nodes aligned to that plane;
- upright structures with visible top and side faces;
- relationships routed across the ground;
- flat operational chrome around the map.

Workers and orchestrators may remain restrained placeholders in this slice.
The owner plans to provide sprites for them later.

## What The Branch Contains

The current branch checkpoints the first 2.5D spike:

- a persisted Plan/2.5D map appearance toggle;
- 2.5D as the initial mode when no preference is stored;
- diagonal map-grid styling;
- embossed project, building, worker, orchestrator, child-agent,
  coordination-node, and automation styling;
- stable worker markers in 2.5D mode;
- Playwright coverage for mode persistence, desktop, dark theme, and mobile;
- the existing flat Plan mode as a fallback.

The implementation is concentrated in:

- `web/src/RuntimeCanvas.tsx`
- `web/src/App.css`
- `web/tests/project-control.spec.ts`

`CLAUDE.md` is unrelated and intentionally excluded from this branch commit.

## Verification Evidence

Before the visual rejection, this exact spike passed:

- 190 Rust tests;
- 67 Playwright tests;
- `npm run lint`;
- `npm run build`;
- `git diff --check`.

This evidence establishes that the spike preserved the existing application
behavior at that point. It does not establish visual acceptance.

## Why The Spike Was Rejected

The spike styles flat ReactFlow cards as raised objects but leaves ReactFlow's
world, edges, hit testing, and drag math planar. The result is decoration over
a flat canvas, not a projected world.

Specific failures:

1. Project territories remain axis-aligned HTML rectangles.
2. Buildings do not share one ground projection with their territories.
3. Edges remain flat ReactFlow routes over an isometric-looking background.
4. Dragging follows the original screen axes instead of inverse-mapping
   pointer movement through the projected plane.
5. Workers and orchestrators look embossed instead of standing as billboards
   or sprites on ground anchors.
6. The diagonal background grid implies perspective that the components do
   not obey.

The current CSS block beginning with the comment `2.5D is
presentation-only` records the rejected approach. It should be replaced, not
made more dramatic with additional skew, shadow, or extrusion.

## Review Findings On The Spike

An independent review also found:

- depth shadows can override yellow selected-state rings;
- forced-colors mode removes persistent selection cues;
- selected mobile projects retain desktop-sized extrusion;
- the generic depth edge filter overrides active communication glow.

These are useful regression warnings, but fixing them in isolation would not
address the rejected spatial model.

## Existing Contracts To Preserve

- Stored `CanvasPlacement` values remain unprojected world coordinates.
- Plan mode retains the current ReactFlow experience.
- Project, workspace, worker, child-agent, automation, and coordination-node
  selection continues to open the correct inspector.
- Project allocation drag/drop continues to hit the visible territory.
- Right-click node creation maps to the visible pointer location.
- Relationship status remains semantic: idle gray, active green, failed red.
- Terminal, chat, inspectors, map controls, minimap, and other operational
  chrome remain flat and theme-aware.
- Motion represents real activity and respects reduced-motion preferences.
- The map remains usable in light mode, dark mode, and mobile viewports.

## Resume Point

Start from the projected-map plan in
`docs/plans/2026-08-15-2-5d-projected-map.md`.

The next workflow should first remove the rejected depth-only CSS treatment,
then establish shared projection math and a projected scene. Do not start by
restyling individual cards again.

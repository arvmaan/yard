# UI Declutter Research: Top Chrome, Theme Toggle, Plan vs 2.5D

> **Superseded in part (2026-08-18):** The placement recommendations for
> global modes, resource access, theme, map view, runtime health, Project pulse,
> and Arrange spaces are replaced by
> [Proposed UI Settings and Chrome Architecture](./2026-08-18-ui-settings-and-chrome.md).
> The implementation inventory and 2D fallback rationale remain useful
> historical evidence.

Branch: `2.5d-projected-map-impl` @ `15884b1`. Read-only research; no code changed.

Files referenced: `web/src/App.tsx`, `web/src/App.css`, `web/src/RuntimeCanvas.tsx`,
`web/src/theme.ts`, `web/src/ProjectedMap.tsx`, `web/src/mapScene.ts`, `web/src/mapProjection.ts`.

---

## TL;DR recommendations

1. **Top chrome**: Collapse the three stacked control rows (command bar, resource shelf, canvas
   tools panel) into two. Merge the always-visible read-only metrics into the tab labels that
   already show counts, move session-select + refresh into a small overflow/settings menu, and
   pull "Project pulse" / "Arrange spaces" out of the tiny floating canvas panel into a proper
   toolbar next to (not stacked under) the resource shelf.
2. **Dark/light toggle**: It already exists and already works correctly (`App.tsx:4255-4272`,
   `web/src/theme.ts`). Nothing to build. The bug isn't the toggle, it's that it's buried as one
   of eight same-weight icon controls in a crowded bar — fix placement/grouping, not
   functionality.
3. **Plan vs 2.5D**: Don't remove Plan mode. The two modes share the same underlying (unprojected)
   world coordinates, so "doesn't translate over" isn't really about lost data — it's about three
   concrete, code-documented interaction/rendering gaps (minimap shows the wrong layout, the
   camera hard-jumps on every mode switch, and resize/connect handles don't track the projected
   shape). Fix those three, and reframe Plan explicitly as the secondary/precision-editing mode
   (2.5D is already the persisted default). Full removal would throw away ~2,700 lines / 13
   commits of purpose-built work for a problem that's fixable in far less.

---

## Topic 1 — Top bar / profiles bar / control bar declutter

### What's actually there today

The app renders three stacked rows of controls at the top of the screen, defined by a CSS grid
on `.app-shell` (`web/src/App.css:86-105`):

```
--command-bar-height: 48px
--resource-shelf-height: 96px
```

**Row 1 — `.command-bar`** (`App.tsx:4126-4310`), always visible. Left to right:

| Control | Element / lines | What it does | Usage frequency (my read) | Duplicate elsewhere? |
|---|---|---|---|---|
| Brand mark ("Y" / "Yard / Operations atlas") | `4127-4135` | Static branding, no action | n/a | — |
| Map / Chat / Terminal tabs | `4137-4171` (`workspace-mode-switcher`) | Switches the whole workspace between canvas, chat, terminal | **High** — this is the primary navigation control | No |
| "Create project" | `4173-4184` (`top-command`) | Opens project-creation flow | Medium — a core action, but not continuous | No obvious duplicate; right-click canvas menu only offers Automation/Workstream/Knowledge store, not a new project |
| Profiles / Workers / Workspaces tabs | `4186-4207` (`command-bar__resources`) | Opens/closes the resource shelf (row 2) and picks which list it shows | High | — |
| Metrics: "N projects", "N workers", "N attention" | `4209-4232` (`command-bar__metrics`) | **Read-only** counters, not buttons | Low-to-none as an *action* (nothing to click) | Yes — see below |
| Session select dropdown | `4234-4253` (`session-command`) | Picks the active Herdr runtime session | Low-frequency (set-and-forget for most sessions) | No |
| Theme toggle | `4255-4272` | Dark/light switch | Low-frequency but sticky/important once set | No |
| Connection state text + refresh button | `4274-4309` (`connection-state`) | Shows "Herdr observed / unavailable / Observing…" + manual refresh | Refresh: occasional. Status text: ambient/passive | No |

**Row 2 — `.resource-shelf`** (`App.tsx:4312-4553`, the "profiles bar"), shown only when a
resource tab is active. Its own collapse chevron (`4318-4326`) sits inside it. Content is
one of three panels depending on `railView`:

- **Profiles panel** (`4328-4403`): heading with a "New profile" icon button (`4335-4343`), a
  *second*, functionally identical "New profile" button in `.profile-list__mobile-actions`
  (`4346-4356`, CSS-only shown on narrow layouts — so it's not simultaneously visible, but it is
  duplicated markup/logic for the same action), then the list of `profile-row` items.
- **Workers panel** (`4404-4505`): heading + live count (`4411`), a filter segmented control
  (`4413-4424`) with **All / Available / Allocated / Attention**, then `worker-row` items.
- **Workspaces panel** (`4506-4549`): heading + count, `workspace-row` items.

**Row 3 — the floating `.canvas-tools-panel`** (`RuntimeCanvas.tsx:3581-3630`), the "bar
underneath". This is a `<Panel position="top-left">` from React Flow, so it visually sits right
below rows 1-2, reading as a third horizontal strip of buttons even though it's technically part
of the canvas, not the app shell grid:

- **Plan / 2.5D view switcher** (`3584-3607`) — see Topic 3.
- **"Project pulse"** (`3608-3616`) — opens a full-screen `ProjectPulseWorkspace` overlay
  (`App.tsx:4843+`), a dashboard-like view. Unrelated in kind to the other two buttons in this
  panel.
- **"Arrange spaces"** (`3617-3629`) — one-shot auto-layout command, disabled when there's
  nothing to arrange.

### Concrete duplication found

- **`command-bar__metrics` "N attention" badge duplicates the Workers panel's "Attention" filter**
  (`App.tsx:4224-4231` vs. the `FILTERS` array at `215-219` and the segmented control at
  `4413-4424`). The top-bar version is inert (a `<span>`, not a button) — you see the count but
  can't act on it, while the *real*, actionable version is one click away inside the Workers tab.
  This is close to the textbook "throwing things on top of other things": the same fact is shown
  twice, once passively and once functionally.
- **"N workers" in the metrics cluster** overlaps with the live count already shown in the
  Workers panel's `section-heading` (`4411`, `{visibleCandidates.length}`) once that panel is
  open. Not a hard duplicate (one is total workers, the other is the filtered/visible set) but
  conceptually redundant chrome that only needs to exist in one place.
- **Two "New profile" buttons** for the same action, gated by a media query rather than actually
  being one responsive control (`4335-4343` and `4346-4356`).
- The mobile breakpoint at `App.css:4684-4757` has to shrink the whole command bar to
  icon-only, hide the connection-state label, hide the top-command label, and still let the bar
  scroll horizontally (`overflow-x: auto`, `4703-4709`). That the bar *has* to horizontally
  scroll on a 760px-wide viewport is itself evidence there are too many permanently-resident
  controls in row 1 — the fix on mobile is currently "shrink everything," not "show less."

### Proposed consolidation

Cluster by function, not by where they happen to sit today:

1. **Primary navigation (always visible, unchanged)**: Map / Chat / Terminal tabs, Profiles /
   Workers / Workspaces tabs. These are the two real navigational primitives in the app; keep
   them as the anchor of row 1.
2. **Primary action (always visible, unchanged)**: "Create project."
3. **Fold metrics into the tabs, don't show them twice.** Put the live counts as small badges on
   the Profiles/Workers/Workspaces tab labels themselves (e.g. "Workers · 12") instead of a
   separate `command-bar__metrics` cluster. Drop the standalone "N projects" readout (it's not
   actionable from the top bar anywhere) or move it into the resource shelf. Kill the passive
   "N attention" span outright — make the Workers tab itself carry an attention indicator (dot or
   count in red) so there's exactly one place attention count lives, and it's the actionable one.
4. **Move low-frequency, "set once and forget" controls into a single overflow/settings
   affordance**: session select dropdown, and the connection-state text. These are the two
   controls least likely to be touched per session — session switching is rare, and connection
   status is ambient information better suited to a small persistent icon (green/red dot) than a
   full text label + icon cluster. Collapse "Herdr observed / unavailable / Observing Herdr" +
   refresh button into one small status icon that expands into the full session/refresh controls
   on click, rather than being permanently spelled out in the bar.
5. **Keep the theme toggle visible, but give it more visual isolation** — see Topic 2 below for
   why.
6. **Merge the resource shelf's own chrome down**: the collapse chevron, the "New profile"
   button, and the worker filter segmented control are all shelf-scoped controls that are fine
   where they are; the only real fix here is de-duplicating the two "New profile" buttons into
   one responsive control instead of two DOM copies gated by CSS.
7. **Break up the floating canvas-tools-panel by function, and stop stacking it directly under
   the shelf.** "Project pulse" (navigate to a dashboard) and "Arrange spaces" (one-shot layout
   command) have nothing to do with "Plan vs 2.5D" (a persistent view setting) other than
   happening to live in the same React Flow `<Panel>`. Move the Plan/2.5D switcher to live beside
   the theme toggle in row 1 (it's a view preference, exactly like light/dark, and arguably
   belongs in the same visual family), and leave only workspace-scoped actions (Project pulse,
   Arrange spaces) floating on the canvas — ideally as a single "..." overflow button on the
   canvas rather than a permanent 3-button row, since "Arrange spaces" in particular is not
   something used continuously.

Net effect: row 1 keeps navigation + the one primary action + theme/view prefs tucked into one
compact cluster; row 2 (resource shelf) stays as-is with the one duplicate button removed; row 3
disappears as a *visually separate bar* — it becomes a single small overflow control on the
canvas itself, not a third horizontal strip competing for the same header space.

---

## Topic 2 — Dark/light toggle

**This already exists and is fully wired up. It is not a "let's take a look at building this"
item — it's a "why is this hard to find" item.**

- Button: `App.tsx:4255-4272`, class `icon-button theme-toggle`, `aria-label`/`title` both say
  "Switch to {light/dark} mode", icon flips between `Moon`/`Sun` from lucide.
- State: `const [theme, setTheme] = useState<YardTheme>(readTheme)` at `App.tsx:1866`.
- Persistence + side effects: `useEffect(() => applyTheme(theme), [theme])` at `App.tsx:1985-1987`.
- `web/src/theme.ts`:
  - `readTheme()` (`8-14`) reads `localStorage['yard:theme']`, falling back to
    `prefers-color-scheme` media query if nothing is stored.
  - `applyTheme()` (`16-23`) sets `document.documentElement.dataset.theme`, sets
    `document.documentElement.style.colorScheme`, persists to localStorage, and dispatches a
    `yard:theme-change` CustomEvent.
  - `terminalTheme()` (`33-72`) derives a full xterm.js `ITheme` (background, foreground, ANSI
    colors, selection color, cursor color) from the current CSS custom properties, so the
    in-app terminal's colors follow the theme too.

I checked for bugs and found none — it round-trips correctly, respects OS preference on first
load, persists across reloads, and the terminal color derivation is a genuinely nice touch that's
easy to miss unless you go looking for it.

**Honest read on discoverability vs. quality**: this is a placement problem, not a quality
problem. In the current markup, the theme toggle is the 7th of 8 distinct controls in a single
horizontal strip (`App.css:107-117` — one flex row, `gap: 8px`, no visual grouping beyond that),
sandwiched between the session-select dropdown and the connection-state cluster, all styled at
similar visual weight (same `icon-button` sizing convention as the refresh button right next to
it). There's nothing that says "this one is different in kind from the others" — it looks like
just another utility icon in a row of utility icons, not a top-level app preference. Under the
Topic 1 consolidation (moving session-select and connection-state into a lower-priority
overflow), the theme toggle would end up with far less competing for attention right next to it,
which alone would likely fix the "lost it in the noise" complaint without touching `theme.ts` at
all. If further isolation is wanted, pairing it with the Plan/2.5D switcher (both are "how do I
want to look at this" settings, not workflow actions) as a small "view preferences" cluster would
make it read as a distinct, intentional control rather than one more icon in the busy row.

---

## Topic 3 — Plan mode vs 2.5D mode

### Where it lives

- Type: `type MapVisualMode = 'depth' | 'flat'` (`RuntimeCanvas.tsx:327`). UI labels: "flat" is
  presented as **Plan**, "depth" as **2.5D** (`RuntimeCanvas.tsx:3587-3606`).
- Persistence: `localStorage['yard:map-visual-mode:v1']`
  (`readMapVisualMode`/`writeMapVisualMode`, `RuntimeCanvas.tsx:414-425`). **Default is `'depth'`**
  if nothing is stored (`417`, `419`) — i.e. 2.5D is already the de facto default for new users;
  Plan is already the opt-in secondary mode at the data layer, just not in how the UI presents the
  choice (a flat two-button segmented control implies parity).
- Switcher UI: `RuntimeCanvas.tsx:3581-3607`, inside the floating `canvas-tools-panel` (see
  Topic 1).
- Rendering split: `ProjectedMap` (the SVG ground/buildings/routes) only mounts when
  `visualMode === 'depth'` (`RuntimeCanvas.tsx:3572-3580`). In flat mode, React Flow's normal
  node boxes and edges render directly with no projection.
- CSS: `App.css` has a dedicated **~674-line block** (lines `6114-6788`, the very end of the
  file) titled "2.5D projected map", containing 43 separate `[data-visual-mode="depth"]`
  selectors plus all of the building/billboard/sprite/route styling that's exclusively consumed
  by the depth-only `ProjectedMap`/marker components. That's roughly **10% of the entire 6,788-line
  App.css file** dedicated to one of two view modes.
- Branch cost: `git diff --stat main...2.5d-projected-map-impl` shows **13 commits, +2,742 lines**
  net across `web/src`, including three new modules built specifically for this
  (`ProjectedMap.tsx` 469 lines, `mapScene.ts` 265 lines, `mapProjection.ts` 235 lines), a
  dedicated projection math test file (`mapProjection.test.ts`, 256 lines), ~734 new lines in
  `App.css`, and ~820 net new lines in `RuntimeCanvas.tsx`. This is clearly the flagship feature
  of the branch, not a minor side mode.

### What "doesn't translate over" concretely means

The good news first: **the underlying data is shared, not forked.** `node.position` is stored in
unprojected world coordinates in both modes (see the comment at `RuntimeCanvas.tsx:2401-2413`),
and projection is applied only as a rendering-time CSS transform
(`projectedNodes` memo, `RuntimeCanvas.tsx:2546-2613`). So layout, selection state
(`CanvasSelection` in `App.tsx`), and all domain data (projects, workers, assignments) are
identical whether you're in Plan or 2.5D — switching modes does not lose or reset your project
layout.

What I found instead are three concrete, code-documented interaction/rendering gaps — these are
almost certainly what reads as "doesn't translate":

1. **The minimap shows a different layout than the mode you're in.** `<MiniMap>`
   (`RuntimeCanvas.tsx:3708-3729`) always renders from React Flow's raw (flat) `node.position`
   values — it is never projected. So in 2.5D, the main canvas shows an axonometric skyline while
   the minimap in the corner simultaneously shows a plain top-down flat layout of the same scene.
   That's a direct, always-visible contradiction between two views of the same state — very
   plausibly what "doesn't translate" feels like in practice, since it's visible every second
   you're in 2.5D mode.

2. **The camera hard-jumps every time you switch modes.** `RuntimeCanvas.tsx:3275-3336` and
   `2778-2814`: because "the world y axis runs left across the screen" once projected
   (direct code comment), the flat viewport position is meaningless in depth mode, so switching
   into 2.5D forces a `fitBounds()` re-frame, and `arrangeSpaces()` has to special-case `fitBounds`
   vs `fitView` depending on mode. This is handled deliberately and correctly, but the user
   experience is: click "2.5D," and the camera visibly recenters/rezooms on its own. That reads
   as "my view didn't carry over," even though the underlying layout did.

3. **Resize and relationship/coordination-connect handles don't track the projected shape.**
   Two comments in `RuntimeCanvas.tsx` call this out explicitly as intentional-but-incomplete:
   - `699-709`: "`NodeResizer` computes its eight handles from the node's own measured flat
     rectangle, and there is no supported way to move them onto a projected parallelogram's
     corners... Projected corner handles need per-corner delta maths beyond the translation-only
     inverse projection and are explicit future scope."
   - `721-729`: the relationship/coordination `<Handle>` drag targets "stay live and keep their
     flat position relative to the node box... connect-by-drag remains a flat interaction over a
     projected territory."
   - In practice: in 2.5D, a project territory renders as a skewed parallelogram, but its resize
     handles and connection points sit where the *invisible* flat rectangle's corners would be —
     not on the visible shape's corners. If you try to resize or connect by dragging to what looks
     like the territory's corner in 2.5D, you're dragging to the wrong spot on screen.

4. **Two parallel edge-rendering systems must stay in sync.** React Flow's normal edges
   (`.react-flow__edge`) are hidden via CSS in depth mode (`App.css:6738-6746`, "Flat edges are
   replaced by the projected routes... they simply stop being drawn") and replaced by
   `ProjectedRoute` SVG geometry from `mapScene.ts`. They represent the same relationships through
   two independent code paths — a real ongoing maintenance cost, not a one-time gap.

### Options

**(a) Keep both modes, fix the specific translation gaps.**
Concretely: reproject the minimap (cheapest fix — project each node's position before handing it
to `<MiniMap>`, or accept the flat minimap but visually badge it "plan view" so the mismatch reads
as intentional rather than broken), smooth or otherwise call out the camera re-frame on switch
(e.g. animate rather than snap, or preserve zoom level), and either finish the parallelogram-aware
resize/connect handles (the harder, "explicit future scope" fix already flagged in the code) or
lean into option below and stop treating that gap as a bug.

**(b) Make 2.5D the default/primary mode, demote Plan to secondary.**
This is already true at the persistence layer (`readMapVisualMode` defaults to `'depth'` for new
users) but not reflected in the UI, which presents "Plan | 2.5D" as a flat, symmetric toggle.
Reframing Plan explicitly as the secondary/precision-editing view — e.g. "switch to Plan to
resize or connect precisely, 2.5D is the default view otherwise" — would turn the resize/connect
handle limitation (item 3 above) from a bug into a documented reason Plan mode still exists,
rather than something that needs fixing.

**(c) Remove Plan mode entirely.**
Given the ~2,700 lines and 13 commits already invested in the projection system, and that Plan
mode is functionally the *more precise* mode for resizing territories and connecting
relationships (per the code's own comments), removing it outright would both waste that
investment and remove the one mode where two real interactions currently work correctly. Not
recommended without first fixing or explicitly accepting the resize/connect gap — you'd be
removing your escape hatch for the exact interactions 2.5D itself admits it can't do properly yet.

### Recommendation

**Go with (a), with a light touch of (b).** Fix the two cheap, highly-visible gaps first — the
minimap mismatch and the camera-jump-on-switch — since those are almost certainly the concrete
things that make the mode switch feel broken today, and both are small, contained fixes relative
to the rest of the system. Leave the resize/connect-handle limitation as-is for now, but make it
legible to the user by reframing Plan mode's purpose in the UI copy/labeling (soft version of
(b): "2.5D" as the default/primary view, "Plan" explicitly labeled as the precision-editing or
fallback view rather than a coequal peer). Do **not** pursue (c) — the sunk cost is real, the mode
is the branch's namesake feature, and Plan mode is currently doing real interaction work that
2.5D admits, in its own code comments, that it can't do yet. Revisit removal only if, after
fixing the minimap/camera issues and reframing Plan as secondary, usage data shows people still
never touch it.

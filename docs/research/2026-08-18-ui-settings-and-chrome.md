# Proposed UI Settings and Chrome Architecture

Date: 2026-08-18

Status: Proposed research; not an accepted product decision

Scope: Yard web chrome, settings placement, and responsive control hierarchy

Supersedes in part: `docs/research/2026-08-17-ui-declutter.md`

## Decision summary

Yard should use one quiet global command bar, one optional resource shelf, and
contextual controls inside the active workspace. The map, not the chrome,
should remain the dominant visual surface.

Keep these controls or states always visible:

- compact Yard identity;
- Create project;
- one Resources trigger;
- actionable attention when attention exists;
- Project pulse;
- current Herdr session and connection health;
- one Settings trigger;
- an `Auto N` telltale only while automatic token-consuming behavior is on.

Move these controls behind a one-click top-level surface:

- theme and 2D/2.5D in Settings;
- session selection and global refresh in the health popover;
- Profiles, Workers, Workspaces, counts, and filters in the resource shelf;
- Arrange spaces in a map actions menu;
- the three durable automatic-token controls in the first level of Settings.

Make these controls contextual:

- Chat and Terminal mode switching after an agent target is opened;
- agent/window navigation inside the agent workspace;
- terminal palette inside Terminal;
- profile creation inside the Profiles shelf;
- resource filters inside the Workers shelf;
- map zoom, fit, minimap, and map actions only on Map.

Remove or merge these visible elements:

- inert project and worker metric chips;
- the inert attention chip;
- the `Yard projects / count / session` canvas plaque;
- duplicate mobile and desktop Create profile markup;
- disabled Chat and Terminal tabs when no target exists;
- the separate floating row containing view preference, pulse, and arrange.

This is an information architecture change, not a rebrand. Keep Yard's current
palette, typography, compact geometry, icons, and map treatment.

## Product and visual stance

The subject is a local operational control plane for people supervising
multiple AI-agent workstreams. Its single interface job is to make current
state and the next safe action easy to find without covering the work.

Retain the existing visual system:

| Role | Existing treatment |
| --- | --- |
| Surfaces | Paper `#f7f9f8`, mineral `#e5e9e7`, canvas `#dce2df` |
| Text | Ink `#18201e`, muted `#68736f` |
| Operational accents | Teal `#19766b`, yellow `#e7b32b`, vermilion `#d94a37`, blue `#3978b8` |
| UI type | IBM Plex Sans |
| Identity and headings | Space Grotesk |
| Runtime data | IBM Plex Mono |
| Geometry | 2-6px radii, thin rules, compact 28-34px controls |

The map is Yard's signature element. Chrome should not compete with it. The one
bounded new signature is an instrument-like `Auto N` telltale that appears
only while automatic token-consuming coordination is enabled. This is a useful
risk: hidden automatic spend is more dangerous than one conditional status
label. It uses existing type, color, and dimensions rather than introducing a
new visual theme.

A generic settings page, card dashboard, decorative hero treatment, or new
palette would not be specific to Yard and is rejected.

## Evidence

The current implementation renders:

- a 48px command bar with brand, workspace tabs, Create project, three resource
  tabs, three metric chips, session select, theme, connection state, and
  refresh (`web/src/App.tsx:4123-4310`);
- a 96px resource shelf below the command bar
  (`web/src/App.tsx:4312-4553`, `web/src/App.css:362-479`);
- a floating map row with 2D/2.5D, Project pulse, and Arrange spaces
  (`web/src/RuntimeCanvas.tsx:3596-3645`);
- a second canvas plaque repeating project count and session
  (`web/src/App.tsx:4555-4560`);
- a minimap and zoom/fit controls on every map size
  (`web/src/RuntimeCanvas.tsx:3723-3745`);
- full-screen agent workspaces beneath the still-visible global bar
  (`web/src/AgentWorkspaceShell.tsx:96-221`).

At 760px and below, the command bar becomes horizontally scrollable,
connection state disappears, labels collapse to icons, and the open shelf
grows to 132px (`web/src/App.css:4684-4809`). That hides operational health and
moves remaining controls offscreen rather than establishing priority.
The global mode labels are removed with `display: none` while their icons are
`aria-hidden`, so those compact buttons can also lose their accessible names
(`web/src/App.css:4564-4576`).

Global inventory already polls automatically, but the manual refresh remains
an important recovery command (`web/src/App.tsx:2232-2304`). It should be
demoted, not deleted. Workspace resource rows have the opposite problem:
runtime status is represented through `data-status` styling without visible
or accessible status text (`web/src/App.tsx:4506-4544`).

The full-screen agent workspace is marked as a dialog but does not make the
background inert or trap focus (`web/src/AgentWorkspaceShell.tsx:96-221`).
Because proposed global chrome remains usable above that workspace, it should
be a named application region rather than a modal dialog.

Focused Playwright artifacts generated from the current implementation show:

- `web/test-results/project-control-renders-du-c5b9d-nbound-resources-on-desktop/desktop.png`:
  the global bar gives inert metrics, preferences, status, and actions similar
  visual weight;
- `web/test-results/project-control-renders-th-d7331--and-persists-the-view-mode/raised-map-desktop.png`:
  the open shelf, canvas plaque, and floating toolbar form three chrome bands;
- `web/test-results/project-control-renders-th-d7331--and-persists-the-view-mode/raised-map-dark.png`:
  the existing dark visual language is coherent and does not need redesign;
- `web/test-results/project-control-renders-th-d7331--and-persists-the-view-mode/raised-map-mobile.png`:
  horizontal header overflow, a 132px shelf, canvas plaque, toolbar, minimap,
  and zoom controls substantially reduce the visible map;
- `web/test-results/project-control-uses-full--58b7b-th-a-Herdr-window-navigator/agent-workspace-shell.png`:
  target navigation is useful in the workspace, while global disabled modes
  and map/resource actions remain visible above it;
- `web/test-results/project-control-uses-repor-21e2c-eeps-runtime-state-distinct/project-pulse-long-message.png`:
  Project pulse already works as a focused operational dialog and should stay
  directly reachable.

These generated images are evidence, not committed visual baselines.

## Superseded recommendations

The 2026-08-17 note identified the crowding correctly. This proposal changes
several placements after reviewing the integrated UI and mobile screenshots.

| 2026-08-17 recommendation | 2026-08-18 replacement |
| --- | --- |
| Keep Map/Chat/Terminal and Profiles/Workers/Workspaces permanently visible as two primary navigation groups. | Keep resource access persistent as one Resources trigger. Show Chat/Terminal only in a target workspace. Do not render unavailable modes as disabled global tabs. |
| Keep theme visible in the command bar. | Put theme at the first level of Settings. It is a preference, not an operational command. |
| Put 2D/2.5D beside theme in the command bar. | Put 2D/2.5D at the first level of Settings. Preserve 2D as the explicit fallback. |
| Move connection text into overflow/settings and leave a status dot. | Keep session plus health as an always-visible, labeled status trigger. Its one-click popover owns session selection and refresh. A color-only dot is insufficient. |
| Leave Project pulse and Arrange spaces together in canvas overflow. | Keep Project pulse in global chrome because it is portfolio status. Put Arrange spaces in a map-only actions menu. |
| Fold all counts into resource tabs. | Show counts inside the open shelf. Keep only actionable attention in persistent chrome. |

The recommendation to deduplicate Create profile remains valid. The
recommendation to retain 2D remains valid.

## Audited control inventory

Classification:

- **Always**: persistent operational state or primary command.
- **One click**: visible after opening one top-level panel or menu.
- **Context**: visible only where it can act on a selected target or surface.
- **Merge/remove**: redundant or passive chrome.

| Current control or state | Current location | Audit | Proposed location |
| --- | --- | --- | --- |
| Yard mark and name | Global bar | Useful orientation; tagline is low-value chrome | **Always**. Compact mark plus `Yard`; mark/name returns to Map |
| Operations atlas tagline | Global bar | Branding competes with controls | **Remove** from compact chrome |
| Map tab | Global mode tabs | Duplicates closing an agent workspace | **Context**. Back to Map in agent workspace; Yard identity also returns home |
| Chat tab | Global mode tabs; disabled without target | Unavailable control adds noise | **Context** in target workspace and selected-target actions |
| Terminal tab | Global mode tabs; disabled without target | Unavailable control adds noise | **Context** in target workspace and selected-target actions |
| Create project | Global bar | Core command | **Always**, text on desktop and folder-plus icon with tooltip on mobile |
| Profiles tab | Global bar | Useful, but not worth permanent width | **One click** in Resources shelf |
| Workers tab | Global bar | Useful, but not worth permanent width | **One click** in Resources shelf |
| Workspaces tab | Global bar | Useful, but not worth permanent width | **One click** in Resources shelf |
| Project count | Inert global chip and canvas plaque | Repeated, non-actionable | **Remove** from persistent chrome; show in Pulse and shelf context |
| Worker count | Inert global chip and Workers shelf | Repeated, non-actionable | **Remove** from persistent chrome; show on Workers tab in open shelf |
| Attention count | Inert global chip and Workers filter | Important but current chip cannot act | **Always when nonzero**. One click opens Workers filtered to Attention |
| Session select | Global bar | Low frequency; consumes width | **One click** in health popover |
| Theme toggle | Global bar | Valid local preference, not a frequent command | **One click** in Settings, Light/Dark segmented control |
| Herdr loading/observed/unavailable | Global bar; hidden on mobile | Ambient operational state must not disappear | **Always** as session plus health trigger; warnings retain text at every size |
| Global refresh | Inside connection cluster | Occasional recovery action | **One click** in health popover, next to last-updated state |
| Shelf collapse | Shelf corner | Necessary only while shelf is open | **Context** in shelf header |
| Shelf heading/eyebrow | Shelf | Repeats selected resource tab | **Merge** into shelf tabs and concise scoped toolbar |
| Create profile, desktop copy | Profiles heading | Correct scoped action | **Context** in Profiles shelf |
| Create profile, mobile copy | Profiles list | Duplicate markup and behavior | **Remove**; use one responsive control |
| Worker filters | Workers shelf | Correct scoped controls | **Context** in Workers shelf; preserve All/Available/Allocated/Attention |
| Resource rows | Resource shelf | Core browse/drag affordance; workspace status is currently color-only | **Context** in open shelf; cards remain valid repeated items and expose status as text |
| Canvas project/session plaque | Map top-left | Repeats global/shelf state and displaces toolbar | **Remove** |
| 2D button | Floating map toolbar | Required fallback, but a preference | **One click** in Settings; never remove or hide behind an advanced submenu |
| 2.5D button | Floating map toolbar | Current default and preference | **One click** in Settings |
| Project pulse | Floating map toolbar | High-value portfolio status | **Always** in global chrome; show attention count when relevant |
| Arrange spaces | Floating map toolbar | Occasional destructive-to-layout command | **One click** in map actions menu; disabled reason shown when unavailable |
| Minimap | Map bottom-left | Useful on larger maps; costly on narrow map | **Context** on desktop; hide at mobile breakpoint |
| Zoom in/out and fit | Map bottom-right | Standard direct manipulation | **Context** on Map; retain familiar icons and labels |
| Right-click create menu | Map pointer context | Efficient expert path | **Context** on Map; also provide keyboard-reachable Map actions path |
| Open chat | Selected target inspector | Correct target-scoped action | **Context**, one click from selected target |
| Open terminal | Selected target inspector | Correct target-scoped action | **Context**, one click from selected target |
| Open in Ghostty | Selected target inspector | Separate explicit command | **Context**. Placement stays with terminal actions; renderer choice is out of scope |
| Agent/window navigator | Agent workspace sidebar | Useful target switching | **Context**. Persistent desktop rail; one-click drawer on mobile |
| Agent target status and identity | Agent workspace toolbar | Necessary ambient context | **Context**, always visible while workspace is open |
| Agent workspace close | Agent workspace toolbar | Duplicates global Map tab today | **Context** as one Back to Map icon with tooltip and focus restoration |
| Chat order template | Chat context column | Useful optional accelerator | **Context** inside Chat; mobile opens from a Context button rather than disappearing |
| Chat refresh | Chat transcript | Correct scope | **Context** in transcript header; do not merge with global refresh |
| Chat composer/send | Chat workspace | Primary chat action | **Context**, fixed to active chat surface |
| Group chat retry/send-new | Group delivery state | Correct only after partial failure | **Context** after relevant delivery outcomes |
| Terminal connection status | Terminal status bar | Necessary and correctly scoped | **Context**, always visible in Terminal |
| Terminal color theme | Terminal status bar | Local terminal preference | **Context** in Terminal; do not duplicate in global Settings |

## Proposed component hierarchy

```text
AppShell
|-- GlobalCommandBar
|   |-- YardHome
|   |-- CreateProjectAction
|   |-- ResourcesTrigger
|   |-- AttentionTrigger (only when nonzero)
|   |-- ProjectPulseTrigger
|   |-- RuntimeHealthTrigger
|   |-- AutomaticCoordinationTelltale (only when enabled)
|   `-- SettingsTrigger
|-- RuntimeHealthPopover
|   |-- CurrentSessionSummary
|   |-- SessionSelect
|   `-- RefreshStateAction
|-- SettingsDialog
|   |-- AppearanceSettings
|   |   |-- ThemeControl
|   |   `-- MapViewControl
|   `-- AutomaticCoordinationSettings
|       |-- SuperintendentProjectSummaryToggle
|       |-- ProjectWorkerSummaryToggle
|       `-- ScheduledSummaryToggle
|-- ResourceShelf
|   |-- ResourceTabs
|   |-- ScopedShelfTools
|   `-- ResourceList
|-- WorkspaceViewport
|   |-- MapWorkspace
|   |   |-- RuntimeCanvas
|   |   |-- MapNavigationControls
|   |   `-- MapActionsMenu
|   `-- AgentWorkspace
|       |-- TargetNavigator
|       |-- AgentWorkspaceHeader
|       |   |-- BackToMap
|       |   |-- TargetIdentityAndStatus
|       |   `-- SurfaceSwitcher
|       `-- ChatSurface | TerminalSurface
|-- Inspector
`-- ProjectPulseDialog
```

Settings is one level deep. Do not add category cards, a settings landing page,
or nested submenus.

## Desktop wireframe

```text
+--------------------------------------------------------------------------------+
| [Y Yard] [Create project] [Resources] [! 1] [Project pulse]     [alpha observed]|
|                                                        [Auto 2] [Settings]       |
+--------------------------------------------------------------------------------+
| Resources: [Profiles 1] [Workers 9] [Workspaces 3] [Attention]        [^ close]|
| [resource row] [resource row] [resource row] ...                               |
+--------------------------------------------------------------------------------+
|                                                                                |
|                              YARD MAP                                          |
|                                                                                |
| [minimap]                                                  [+][-][fit] [more]  |
+--------------------------------------------------------------------------------+
```

The resource shelf is a full-width band, not a floating card. When closed, the
map starts immediately below the 48px command bar.

## Mobile wireframe

```text
+------------------------------------------------+
| [Y] [+] [Resources !1] [Pulse] [alpha *] [gear]|
+------------------------------------------------+
|                                                |
|                   YARD MAP                     |
|                                      [+][-][fit]|
|                                                |
+------------------------------------------------+
| Resources                    [Profiles Workers]|
| [Workspaces] [Attention]                    [x]|
| resource row                                    |
| resource row                                    |
+------------------------------------------------+
```

Resources opens as a bottom tray capped at `min(44vh, 360px)`. It is not open
by default on narrow screens. The minimap is hidden. The map remains visible
above the tray, and the command bar never scrolls horizontally.

## Exact one-click paths

Here, "one click" means one persistent or contextual trigger reveals the
information or control surface. Choosing a value or confirming a command is a
subsequent deliberate action.

| Need | Exact path |
| --- | --- |
| View or change theme | `Settings` -> Appearance / Theme is visible |
| View or change 2D/2.5D | `Settings` -> Appearance / Map view is visible |
| Return to 2D fallback | `Settings` -> `2D` |
| Inspect automatic token behavior | `Settings` -> Automatic coordination group is visible |
| See why `Auto N` is present | `Auto N` -> Settings opens focused at Automatic coordination |
| See current session and health details | `alpha observed` -> Runtime health popover |
| Select another running session | `alpha observed` -> Herdr session select |
| Refresh Yard/runtime state | `alpha observed` -> Refresh state |
| Open resources | `Resources` -> resource shelf/tray |
| Act on attention | `! N` -> Workers shelf filtered to Attention |
| Open portfolio status | `Project pulse` -> Project pulse dialog |
| Arrange map spaces | `Map actions` -> Arrange spaces |
| Open target chat | selected agent inspector -> `Open chat` |
| Open target terminal | selected agent inspector -> `Open terminal` |
| Switch Chat to Terminal | agent workspace -> `Terminal` surface tab |
| Return to Map | agent workspace -> Back to Map |
| Switch active target on desktop | target row in agent navigator |
| Switch active target on mobile | target identity -> target drawer |
| Change terminal palette | Terminal status bar -> Terminal color theme select |
| Refresh a chat transcript | Chat transcript header -> Refresh agent activity |

Profiles, Workers, and Workspaces do not each need permanent global buttons.
Opening Resources is one click; its three tabs are immediately visible.
Attention retains a dedicated one-click route because it is time-sensitive.

## Settings model

### Appearance

Use compact controls, not text buttons:

- **Theme**: segmented Light / Dark control.
- **Map view**: segmented 2D / 2.5D control.

The accessible name of Settings should include current preferences, for
example `Settings, dark theme, 2.5D map`, so preference state is not opaque to
screen-reader users while the dialog is closed.

2D remains a supported fallback. It must not be renamed to an expert or legacy
mode, moved under Advanced, or removed when 2.5D is selected.

### Automatic coordination

This group is conceptually integrated here but must not ship as UI-only state.
It depends on a durable settings API and backend enforcement owned by the
token-coordination lane.

All three switches default off and are independently controlled:

| Proposed label | Meaning |
| --- | --- |
| Request project summaries automatically | Allow the Superintendent to request summaries from project orchestrators |
| Request worker summaries automatically | Allow project orchestrators to request summaries from workers |
| Run scheduled summaries automatically | Allow scheduled summary dispatch; this does not disable unrelated automations |

Group copy: `Automatic requests may use provider tokens. Manual requests and
Run now remain available when these are off.`

Do not add a master switch. It would obscure which layer is spending tokens.
Each row shows `Off` or `On`, and saving failure leaves the previous durable
value visible with an inline error.

When one or more are enabled, global chrome shows `Auto N`. Its accessible
label enumerates the enabled behaviors. When all are off, no telltale is
shown, and the Settings trigger's accessible description states `Automatic
coordination off`.

## Persistence boundaries

| State | Boundary | Reason |
| --- | --- | --- |
| App theme | Browser-local preference | Personal appearance; existing `yard:theme` |
| 2D/2.5D | Browser-local preference | Personal map rendering; existing `yard:map-visual-mode:v1` |
| Terminal palette | Browser-local preference | Personal terminal appearance; existing `yard:terminal-palette` |
| Project accent override | Existing browser-local preference, unchanged in this slice | Current behavior; broader ownership decision is out of scope |
| Resource shelf open/closed | Ephemeral UI state | Avoid stale desktop/mobile layout carryover |
| Resource tab and worker filter | Ephemeral UI state | Current task context, not a durable product setting |
| Active Herdr session | Ephemeral UI state | Availability changes between runs; initialize from running sessions |
| Open menu/dialog/workspace | Ephemeral UI state | Interaction state |
| Three automatic coordination switches | Durable Yard setting | Changes token-consuming backend behavior and must survive clients/restarts |
| Project/node placement | Existing durable domain state | Shared operational model, not a UI preference |

Never mirror the automatic coordination switches to localStorage as an
authoritative value. A loading or unavailable settings API must render them
unavailable, not assume they are off while backend behavior may be on.

## Responsive behavior

### Wide desktop: 1100px and above

- One 48px non-scrolling global bar.
- Labels remain on Create project, Resources, Project pulse, and runtime health.
- Resource shelf is a 96px horizontal band when open.
- Agent target navigator remains a 252px rail.
- Minimap and zoom/fit remain available on Map.
- Settings is a compact anchored dialog with one scroll area.

### Compact desktop/tablet: 761-1099px

- Yard tagline is absent.
- Counts remain inside the shelf.
- Healthy runtime label may shorten to `alpha observed`; unavailable and
  loading states keep explicit text.
- Resource shelf remains a band and may scroll its resource list, never the
  command bar.
- Agent target navigator may narrow, but target labels remain available.

### Mobile: 320-760px

- One 46px fixed command bar with no horizontal scrolling.
- Yard uses its mark only. Create project and Project pulse use familiar icons
  with accessible names and tooltips where hover is available.
- Current session plus health stays visible. A healthy state may use a compact
  glyph with the session name; unavailable/loading always includes text.
- Resources opens as a bottom tray and starts closed.
- Settings opens as a full-width bottom sheet with a visible close button.
- Agent target navigation becomes a one-click drawer. It must not remain a
  58px icon rail with unlabeled targets.
- Chat context becomes a one-click panel rather than disappearing.
- Minimap is hidden; zoom and fit remain.
- Project pulse remains a full-viewport modal with safe-area padding.

No control relies on hover for mobile comprehension.

## Keyboard, focus, and labels

- Global actions use buttons, not tab roles. Tab order follows visual order:
  Yard home, Create, Resources, Attention when present, Project pulse, runtime
  health, Auto telltale when present, Settings.
- Resource categories and Chat/Terminal are real tablists only if they
  implement roving `tabIndex`, Left/Right, Home/End, and correct
  `aria-selected`/`aria-controls`. Otherwise use ordinary buttons.
- Settings opens with focus on its heading or first control, traps focus while
  modal, closes on Escape, and restores focus to Settings. Reuse the current
  modal dialog pattern.
- The runtime health popover moves focus to its session summary/select, closes
  on Escape or outside click, and restores focus to the health trigger.
- Runtime health changes use a restrained polite live announcement only when
  state changes. The one-second inventory poll must not repeatedly announce an
  unchanged healthy state.
- Map actions uses menu semantics: Arrow keys move, Enter/Space invokes, Escape
  closes and returns focus.
- Project pulse retains its tested close-button initial focus, focus trap,
  Escape behavior, inert background, and trigger restoration.
- Opening Chat or Terminal records the invoking inspector control. Back to Map
  restores focus to that control when it still exists, otherwise to the
  selected map node.
- The full-screen agent workspace uses a named region, not `role="dialog"`,
  while global chrome remains interactive. If implementation instead makes it
  modal, it must inert the background, trap focus, and provide complete dialog
  semantics.
- Every icon-only button has a stable accessible name. Unfamiliar icons use a
  shared tooltip that appears on hover and keyboard focus; `title` may remain a
  fallback but is not the only tooltip mechanism. Required labels include
  Create project, Project pulse, Runtime health, Settings, Back to Map, Target
  navigator, Map actions, Arrange spaces, Zoom in, Zoom out, and Fit map.
- Focus-visible treatment applies to all global, shelf, settings, map, and
  workspace controls with the existing 2px blue outline. Current top segmented
  controls should not depend on browser-default focus alone.
- Status color is always paired with text, icon shape, or accessible text.

## Reduced motion

- Settings and resource tray transitions become instantaneous under
  `prefers-reduced-motion: reduce`.
- Loading keeps text such as `Observing Herdr`; a frozen spinner is not the
  only loading cue.
- Arrange spaces continues to use zero-duration camera fitting when reduced
  motion is requested.
- Switching 2D/2.5D must not introduce animated camera travel in reduced
  motion.
- Existing worker, route, and status animations continue to stop under the
  reduced-motion media query.
- No automatic coordination setting is communicated through animation alone.

## Phased vertical slices

### Slice 1: Global health and preference settings

- Add `GlobalCommandBar`, `RuntimeHealthPopover`, and one-level
  `SettingsDialog`.
- Move theme and map view into Settings.
- Move session selection and global refresh into runtime health.
- Keep current session/health visible at every breakpoint.
- Remove inert metric chips and the canvas plaque.
- Add desktop/mobile focus and no-overflow coverage.

This slice does not include automatic coordination toggles.

### Slice 2: Resource shelf and actionable attention

- Replace three global resource tabs with Resources and conditional Attention.
- Put resource tabs/counts and scoped actions in one shelf toolbar.
- Deduplicate Create profile.
- Add the mobile bottom tray and remove command-bar horizontal scrolling.
- Preserve drag/allocation workflows and keyboard selection.

### Slice 3: Contextual map and agent workspaces

- Move Project pulse to global chrome.
- Move Arrange spaces into Map actions.
- Move Chat/Terminal switching into the target workspace.
- Replace global Map duplication with Back to Map and focus restoration.
- Convert mobile target navigation from an unlabeled rail to a drawer.
- Hide mobile minimap while retaining zoom/fit.

This slice does not select or replace a terminal renderer.

### Slice 4: Durable automatic coordination

- Land only after the token-coordination lane defines a durable read/write API
  and backend enforcement.
- Render the three independent, default-off switches in Settings.
- Keep manual prompts/routes and explicit Run now available while all are off.
- Add `Auto N` only after durable values load.
- Test each switch in isolation and test API failure/reconciliation.

Do not ship disabled-looking controls backed only by client state.

## Acceptance criteria

- At 1440x900 and 1280x800, the global bar is at most 48px high and never
  scrolls horizontally.
- At 390x844 and 320px wide, the global bar is at most 46px high, has no
  horizontal overflow, and leaves session health visible.
- The default mobile view starts with Resources closed and no minimap.
- Settings opens in one action and shows Theme, Map view, and Automatic
  coordination without another navigation step.
- Theme remains browser-local, persists across reload, and updates all current
  app surfaces.
- 2D and 2.5D remain selectable, persist locally, and 2D remains a tested
  fallback.
- Terminal palette remains reachable in Terminal and persists locally without
  duplicating the app Theme control.
- Runtime health never becomes a color-only dot. Loading and failure states
  remain explicit on mobile.
- Runtime health changes announce once through a polite live region and
  unchanged polling does not repeat the announcement.
- Runtime health opens session selection and Refresh state in one top-level
  popover.
- Attention is absent at zero and opens Workers/Attention in one action when
  nonzero.
- Project pulse opens in one action from Map, Chat, or Terminal and retains
  focus trapping, Escape, and focus restoration.
- Arrange spaces is available only on Map through Map actions and explains its
  disabled state.
- Selecting a target exposes Chat/Terminal actions; switching surfaces and
  returning to Map each take one action.
- Compact mobile controls retain explicit accessible names even when their
  visible text is removed.
- Workspace resource rows expose runtime status through text or an accessible
  label, not `data-status` color alone.
- Agent workspace semantics match behavior: a nonmodal region while global
  chrome remains interactive, or a fully contained modal dialog.
- Resource tabs, workspace surface tabs, menus, and dialogs implement the
  specified keyboard model.
- All icon-only controls have accessible names and visible focus.
- Unfamiliar icon-only controls expose tooltips on hover and keyboard focus.
- Reduced-motion mode removes nonessential transitions and retains textual
  status.
- No project/worker count is repeated in both persistent chrome and a second
  map plaque.
- Only one Create profile control exists in the DOM at a time.
- All automatic coordination settings default off for new and existing
  stores, are durable, and are enforced by the backend.
- Enabling each automatic setting affects only its named coordination layer.
- Scheduled summaries off does not disable unrelated automations or explicit
  Run now.
- Manual prompts and routes remain available with all automatic settings off.
- `Auto N` matches loaded durable state and never appears from optimistic
  client state alone.
- Focused Playwright screenshots cover light desktop, dark desktop, 390px
  mobile, Settings, Resources, Project pulse, Chat, and Terminal.

## Non-goals

- No production or test implementation in this research lane.
- No railroad rendering or performance work.
- No token-setting schema, scheduler, dispatch, or backend implementation.
- No terminal renderer selection or Ghostty evaluation beyond navigation
  placement.
- No portable or harness-neutral profile design.
- No map projection, camera, resize-handle, or route-rendering redesign.
- No change to Project pulse content architecture.
- No rebrand, new palette, new type system, marketing page, or game-style
  decorative chrome.
- No generic dashboard cards or nested settings cards.

## Open questions

- Should Resources start closed on wide desktop for all users, or only remember
  an ephemeral state for the current page lifetime?
- What durable scope owns automatic coordination: Yard-wide only, or a
  Yard-wide default with per-project overrides later? This proposal exposes
  only the three requested Yard-wide controls.
- What exact schedules qualify as `scheduled summaries` without affecting
  unrelated scheduled automations?
- Should the healthy runtime trigger show `alpha observed` or `alpha online`?
  Keep the domain term only if operators understand it.
- Should Project pulse absorb the conditional Attention trigger on very narrow
  screens, or should both remain separate? Usability testing should decide;
  the one-click attention path must survive either choice.

# Terminal-First Agent Experience

Status: **PROPOSED**

Date: 2026-08-18

Snapshot investigated: `yard/terminal-redesign` at `7fff717`

This report supersedes the terminal/chat recommendations in
[`2026-08-17-chat-redesign.md`](./2026-08-17-chat-redesign.md). All
forward-looking UX, component, migration, and test recommendations below are
proposed. No production implementation is included.

## Decision

**Proposed:** retain xterm.js and make the real terminal the default work
surface when exactly one worker or orchestrator is targeted. Offer `Terminal`
and `Focus` presentation presets over the same xterm instance, WebSocket, PTY
lease, and continuous terminal buffer. Focus is a visually simpler terminal,
not a chat transcript and not a conversion of terminal snapshots into
messages.

**Proposed:** keep chat first-class only for a selection of multiple workers or
orchestrators. Its timeline should contain only facts that Yard actually owns:
structured commands, per-recipient delivery receipts, durable assignment and
artifact events, and other typed Yard events. Show each agent's live output in
a real terminal pane or tab backed by the existing terminal transport. Do not
infer message boundaries, senders, or replies from terminal snapshots or byte
deltas.

**Proposed:** preserve manual prompts, order templates, optimistic concurrency,
idempotent command IDs, and retries in a reusable structured order composer.
The composer is distinct from terminal input: it is the durable, acknowledged
Yard command path.

**Proposed:** do not adopt `ghostty-web` now. Its PTY/WebSocket shape is
compatible, but it does not fix Yard's scroll issue and currently introduces
forced-bottom scrolling, accessibility loss, runtime-theme gaps, WASM/CSP
work, and browser-support uncertainty.

## Scope And Ownership

This lane covers terminal and chat entry points, terminal presentation, direct
interaction, structured steering, and multi-target steering. It does not
redesign unrelated top-bar settings or any railroad, performance, token
coordination, or portable worker-profile behavior.

The ownership boundary remains:

| Owner | Responsibilities |
| --- | --- |
| Herdr | Processes, panes, terminal identity, terminal bytes, terminal size, and live terminal lifecycle |
| Yard | Durable assignments, artifacts, commands, command receipts, typed coordination events, and the mapping from a durable target to its Herdr runtime |
| Browser terminal engine | VT emulation, rendering, selection, scrollback, keyboard/IME handling, and terminal-local accessibility |

Yard must not create a second terminal transcript and call it chat. It also
must not take ownership of Herdr's process or pane lifecycle to implement this
experience.

## Snapshot Reconciliation

The brief refers to commit `47bbf10` plus uncommitted
`TerminalSession`/`AgentChatWorkspace`/`App.css` changes. That was the state
before Herdr materialized the worktree. The current facts are:

- `47bbf10`, `Add named terminal color palettes, independent of the app
  theme`, is the first parent of `HEAD` and remains the tip of local
  `2.5d-integration`.
- `HEAD` is clean at `7fff717`, a synthetic Herdr merge named
  `On 2.5d-integration: yard-herdr-fanout-2026-08-18`. Its parents are
  `47bbf10` and the synthetic index commit `5581144`.
- The merge result contains the formerly local terminal/chat fixes. They are
  therefore not uncommitted in this worktree now, but they are also not an
  ordinary scoped product commit. `git branch -r --contains` finds no remote
  branch containing either `47bbf10` or `7fff717`.
- The `App.css` delta in the materialized checkpoint is not the terminal wheel
  fix. Terminal scrolling is implemented by xterm and the terminal-local CSS
  in `index.css`; `App.css` supplies surrounding terminal/chat sizing and
  overflow constraints.

This distinction explains how two people can inspect "the current branch" and
see different behavior. A checkout at `2.5d-integration`, `main`, or a remote
branch is not equivalent to this materialized worktree.

## Current State

### Named palettes

Commit `47bbf10` added the feature in four files:

- Palette IDs, official color values, `yard:terminal-palette` persistence,
  change events, theme resolution, and terminal-chrome variables are in
  [`web/src/theme.ts:15-233`](../../web/src/theme.ts#L15).
- The options are Auto, Nord, Dracula, Solarized Dark, Solarized Light, and
  Gruvbox Dark
  ([`web/src/theme.ts:26-36`](../../web/src/theme.ts#L26)).
- `TerminalSession` reads the preference, applies a palette without
  remounting the terminal, and renders the picker in the terminal status bar
  ([`web/src/TerminalSession.tsx:90-96`](../../web/src/TerminalSession.tsx#L90),
  [`343-408`](../../web/src/TerminalSession.tsx#L343)).
- Terminal-local scrollbar and status-bar chrome are in
  [`web/src/index.css:65-152`](../../web/src/index.css#L65).
- Persistence, independence from the app theme, invalid-value fallback, and
  palette contrast are covered in
  [`web/tests/project-control.spec.ts:4590-4718`](../../web/tests/project-control.spec.ts#L4590).

The default is `auto`, which intentionally resembles the app theme. The
picker is not global; it is visible only after a real terminal has opened.

### Terminal transport and scrolling

Yard currently uses `@xterm/xterm@^6.0.0` and
`@xterm/addon-fit@^0.11.0`; no search or clipboard addon is installed
([`web/package.json:20-33`](../../web/package.json#L20)).

`TerminalSession`:

- Creates xterm with `screenReaderMode`, a 4.5 minimum contrast ratio, and
  10,000 rows of client scrollback
  ([`web/src/TerminalSession.tsx:128-142`](../../web/src/TerminalSession.tsx#L128)).
- Uses `FitAddon` and `ResizeObserver`, then sends `terminal.resize`
  ([`web/src/TerminalSession.tsx:144-196`](../../web/src/TerminalSession.tsx#L144)).
- Sends xterm `onData` as `terminal.input`
  ([`web/src/TerminalSession.tsx:198-202`](../../web/src/TerminalSession.tsx#L198)).
- Opens a target-specific WebSocket, writes ordered base64-decoded
  `terminal.frame` bytes, and sends `terminal.release` on unmount
  ([`web/src/TerminalSession.tsx:203-340`](../../web/src/TerminalSession.tsx#L203)).
- Builds same-origin `ws:`/`wss:` URLs for assignment, project orchestrator,
  Yard orchestrator, and coordination-node targets
  ([`web/src/api.ts:788-855`](../../web/src/api.ts#L788)).

At `47bbf10`, `TerminalSession` installed a capture-phase `wheel` listener that
always called `terminal.scrollLines()` and stopped propagation. The
materialized `7fff717` result removes that override and delegates wheel
semantics to xterm. This matters because xterm distinguishes the normal
scrollback buffer from alternate-screen applications. In the alternate
screen, applications such as `vim`, `less`, or `tmux` may enable mouse
tracking, so wheel activity can correctly become PTY input rather than browser
scrollback.

The xterm scroll element gets contained overscroll and terminal-colored
scrollbars in
[`web/src/index.css:58-74`](../../web/src/index.css#L58). The enclosing modal
and workspace set stable grid dimensions and `overflow: hidden` in
[`web/src/App.css:2826-2915`](../../web/src/App.css#L2826) and
[`3527-3578`](../../web/src/App.css#L3527); these constraints are necessary
for the child xterm viewport to own scrolling, but they do not implement
scrolling themselves.

The focused regression test now verifies:

- Wheel scrolling reaches normal-buffer history.
- New frames do not move a user who is reviewing history back to the bottom.
- Alternate-screen wheel input reaches the PTY.
- Resize sends new dimensions, input reaches the socket, and close releases
  the lease.

See
[`web/tests/project-control.spec.ts:4430-4542`](../../web/tests/project-control.spec.ts#L4430).

This is a meaningful fix, not proof that all terminal scrolling is solved.
Alternate-screen behavior belongs to the running TUI, the current tests are
desktop-mouse oriented, and closing or changing modes disposes xterm and its
client scrollback.

### Single-target chat

The old blank-line/code-fence segmentation and 4,000-character splitting no
longer exist. `activitySegments()` now returns at most one 8,000-character
terminal excerpt
([`web/src/AgentChatWorkspace.tsx:123-132`](../../web/src/AgentChatWorkspace.tsx#L123)).
The chat also tracks whether the reader is within 48 pixels of the bottom and
only follows new snapshots in that state
([`web/src/AgentChatWorkspace.tsx:381-397`](../../web/src/AgentChatWorkspace.tsx#L381)).
Tests cover both one-snapshot rendering and preserving manual scroll position
([`web/tests/project-control.spec.ts:4210-4319`](../../web/tests/project-control.spec.ts#L4210)).

The underlying model is still wrong for chat:

- It polls a recent terminal-output snapshot over REST.
- It replaces every prior `activity:` item with the newest snapshot.
- It labels the snapshot "Agent output" and renders it as one agent bubble.

See
[`web/src/AgentChatWorkspace.tsx:252-304`](../../web/src/AgentChatWorkspace.tsx#L252).
Removing false internal boundaries fixed over-chunking, but the outer bubble
is still not a message emitted by the agent.

### Multi-target chat

`AgentGroupChat` supports assignment and project-orchestrator targets only
([`web/src/AgentGroupChat.tsx:45-58`](../../web/src/AgentGroupChat.tsx#L45)).
Its valuable behavior is real: one structured order becomes independent,
versioned commands with per-target pending, delivered, failed, and retry
states
([`web/src/AgentGroupChat.tsx:108-149`](../../web/src/AgentGroupChat.tsx#L108),
[`270-365`](../../web/src/AgentGroupChat.tsx#L270)).

Its output model is not real chat. It polls and replaces a 2,200-character
terminal snapshot for each target
([`web/src/AgentGroupChat.tsx:192-237`](../../web/src/AgentGroupChat.tsx#L192)).
It also forces the thread to the bottom whenever messages change
([`web/src/AgentGroupChat.tsx:262-268`](../../web/src/AgentGroupChat.tsx#L262)).

### Entry points and discoverability

The app starts in Map mode
([`web/src/App.tsx:1904-1906`](../../web/src/App.tsx#L1904)). Chat and Terminal
remain separate top-level tabs
([`web/src/App.tsx:4137-4170`](../../web/src/App.tsx#L4137)), separate inspector
launchers
([`web/src/WorkerInterventions.tsx:203-278`](../../web/src/WorkerInterventions.tsx#L203)),
and separately mounted surfaces
([`web/src/AgentWorkspaceShell.tsx:192-219`](../../web/src/AgentWorkspaceShell.tsx#L192)).

As a result, neither "single target means terminal" nor "chat is for multiple
targets" is represented in today's navigation.

## Why A User May Still Not See Or Use The Fixes

1. **Wrong revision.** The palette commit and materialized scroll/chat fixes
   are not contained by a remote branch. Running `main`, local
   `2.5d-integration`, or another checkout omits some or all of the behavior.
2. **Stale Vite server.** The prior handoff records Playwright reusing a
   long-running server on fixed port 5173 from another checkout. The browser
   can therefore show valid but stale code while tests appear to target this
   worktree.
3. **Hidden entry point.** Yard opens on Map. The palette picker appears only
   in the 28-pixel status bar after Terminal opens; Chat and external Ghostty
   do not show or use it.
4. **Visually quiet default.** `auto` deliberately matches the app theme, so a
   user sees no obvious palette change until choosing a named option.
5. **Terminal ownership.** The server validates that a lease still maps to the
   active durable runtime
   ([`crates/yard-server/src/terminal_service.rs:382-411`](../../crates/yard-server/src/terminal_service.rs#L382)).
   An unavailable or stale lease yields a closed terminal, and the current
   client intentionally does not reconnect
   ([`web/tests/project-control.spec.ts:4720-4748`](../../web/tests/project-control.spec.ts#L4720)).
6. **Deployment origin.** Interactive terminal WebSockets currently require a
   loopback browser origin
   ([`crates/yard-server/src/http/terminal.rs:223-248`](../../crates/yard-server/src/http/terminal.rs#L223)).
7. **Alternate screen.** A full-screen TUI can own wheel input. That is not
   normal shell scrollback and cannot be repaired with an unconditional DOM
   wheel handler.
8. **Session disposal.** Closing Terminal or switching away unmounts
   `TerminalSession`, releases the lease, and loses browser-local selection,
   search state, and scrollback.
9. **Incomplete ergonomics.** Search is absent. Selection, clipboard,
   touch-scroll, mobile virtual keyboard, and mobile clipboard behavior do not
   have focused acceptance coverage.
10. **Group-chat regression remains.** Single-target chat preserves reader
    position; group chat still jumps to the bottom on each message change.

## Superseding The 2026-08-17 Proposal

| Earlier claim or recommendation | Current evidence | Superseding decision |
| --- | --- | --- |
| Single-agent output is split at blank lines/code fences and hard-chunked at 4,000 characters. | The materialized code returns one snapshot, with a regression test. | The specific chunking claim is stale. The snapshot is still not a message. |
| "xterm.js already handles correct scrollback." | Normal-buffer history and follow preservation now pass a focused test; alternate-screen, mobile, selection, search, disposal, and ownership have separate semantics. | Treat scroll behavior as an explicit capability contract, not a blanket engine guarantee. |
| Append terminal deltas as attributed multi-agent chat replies. | Bytes and deltas have no durable reply boundary or author event. | Never convert snapshots or deltas into messages. Use live terminal panes for bytes and a Yard event timeline for typed durable facts. |
| Build iMessage-style agent reply bubbles and infer recipients from `@mention` text. | Yard has no typed agent-reply event. Recipient dispatch is already structured data. | Do not present terminal output as reply bubbles. Use explicit recipient controls; reserve bubbles/timeline rows for actual Yard events. |
| Terminal reconnects while remaining usable. | The client closes on ownership loss and the regression test expects no reconnect. | Design closed/reopen states explicitly; do not promise transparent reconnect before lease semantics support it. |
| The structured composer is worth preserving. | Command IDs, expected versions, templates, receipts, and safe retry paths are real. | Preserve and extract it as a reusable Yard-owned `OrderComposer`. |
| Terminal snapshots are not messages. | Both single and group chat still poll and relabel snapshots. | This remains the central architectural finding. |

The 2026-08-17 handoff statement that neither terminal scrolling nor chat
chunking had a fix is also superseded by the materialized checkpoint.

## Proposed Experience

### Target routing

| Selection | Default surface | Available secondary surface |
| --- | --- | --- |
| No target | Map | None |
| One worker or orchestrator | Real terminal in `Terminal` presentation | `Focus` presentation over the same terminal |
| Multiple workers/orchestrators | Group steering workspace | Per-target real terminal pane/tab |

Opening a single target from the map or inspector should enter the terminal
directly. The user should not have to choose between two competing
single-target transcripts.

### Terminal and Focus presentations

`Terminal` and `Focus` are presentation presets, not transport modes:

- Both render the same live xterm instance and preserve its PTY lease,
  scrollback, selection, search state, and viewport position when toggled.
- Terminal exposes connection state, terminal identity, palette, search, and
  the structured-order dock.
- Focus reduces nonessential chrome and can collapse the order dock. It still
  renders ANSI, cursor motion, alternate-screen applications, and one
  continuous terminal buffer.
- Focus must not trim output, create cards, insert speaker labels, or infer
  message boundaries.

### Structured orders

Extract the current manual prompt/template behavior into a Yard-owned
`OrderComposer`:

- Free-form manual order text remains available.
- Existing order templates remain available.
- One target is implicit in single-target mode.
- Multi-target mode uses explicit recipient chips or checkboxes. Do not parse
  `@mentions` out of prose to determine command routing.
- Command IDs, expected durable versions, delivery receipts, retry with the
  same ID when safe, and "send as new command" when required remain intact.
- Delivery feedback belongs next to the composer and in the durable event
  timeline, not in terminal output.

Terminal keystrokes and structured orders remain intentionally different:
keystrokes are ephemeral PTY input; orders are acknowledged Yard commands.

### Multi-target steering

The group workspace should have three coordinated areas:

1. **Recipient roster:** explicit target selection, status, and terminal-pane
   activation.
2. **Durable timeline:** user orders, per-recipient receipts, assignment
   lifecycle events, artifacts, and other typed Yard events. Every row must
   have a durable ID and source event.
3. **Live output deck:** one real terminal pane per active target, presented as
   tabs on constrained screens and optionally side by side on wide screens.

The first slice need not invent or persist agent reply messages. If Yard later
gains a typed agent-message event at write time, that event can enter the
timeline without changing terminal transport.

## Proposed Component Architecture

```text
AgentWorkSurface
  TargetSetRouter
    SingleTargetWorkspace
      TerminalSessionController
        TerminalViewport (one xterm instance)
        TerminalToolbar
        TerminalPresentationControl (Terminal | Focus)
      OrderComposer
    MultiTargetWorkspace
      RecipientSelector
      DurableEventTimeline
      TerminalPaneDeck
        TerminalSessionController per opened target
          TerminalViewport
      OrderComposer
```

`TerminalSessionController` is a proposed extraction of the lifecycle already
inside `TerminalSession`: xterm construction, target WebSocket, ordered frame
writes, input, fit/resize, connection state, and lease release. It does not
create processes or panes. A presentation toggle changes layout around its
existing `TerminalViewport`; it must not remount the controller.

`DurableEventTimeline` consumes Yard records only. `TerminalPaneDeck` consumes
Herdr terminal bytes through Yard's existing relay only. Keeping these inputs
separate prevents a display redesign from silently becoming a new messaging
protocol.

## Terminal Capability Contract

### Scrolling and scrollback

**Proposed requirements:**

- In the normal buffer, wheel, trackpad, Page Up/Down, keyboard shortcuts, and
  touch gestures can inspect up to the configured 10,000-row scrollback.
- New output preserves the viewport while the user is above the bottom and
  exposes a "new output" affordance that returns to live output.
- At the bottom, new output follows normally.
- Alternate-screen mouse reporting remains application-controlled. Yard may
  label this state, but must not intercept wheel events unconditionally.
- The first implementation keeps the existing fixed 10,000-row limit.
  User-configurable memory/scrollback policy is a separate decision.

### Selection, copy, and paste

**Proposed requirements:**

- Pointer and keyboard selection work in normal scrollback and remain stable
  when new output arrives.
- Standard platform copy shortcuts and an accessible Copy command copy the
  exact terminal selection.
- Standard paste, context-menu paste, IME composition, and bracketed-paste
  semantics reach the PTY once, without duplicate input.
- Clipboard permission failure leaves selection intact and exposes a
  nonblocking error.
- Mobile long-press selection and paste are explicitly tested on supported
  Safari/iOS and Chrome/Android versions.

### Search

**Proposed:** add the official xterm search addon and a terminal-local Find
control. `Ctrl/Cmd+F` should search the actual xterm normal buffer, show
previous/next match controls and count, and never search a snapshot copy.
Search is disabled or clearly scoped while an alternate-screen application
owns the display.

### Resize

**Proposed requirements:**

- Continue using `ResizeObserver` and `FitAddon`.
- Coalesce duplicate dimensions and send one `terminal.resize` for each
  effective row/column change.
- Desktop pane resize, browser resize, mobile rotation, virtual-keyboard
  appearance, and composer expansion must not create zero-sized terminals or
  resize loops.
- Presentation toggles refit the same terminal instance and do not release the
  lease.

### Accessibility

**Proposed requirements:**

- Retain xterm `screenReaderMode` and `minimumContrastRatio: 4.5`.
- Keep the terminal label, connection status live region, visible keyboard
  focus, non-color status indicators, and a complete keyboard path through
  toolbar, terminal, search, and composer.
- Announce new output without reading an unbounded stream or forcing focus.
- Every named palette must retain at least 4.5:1 text contrast and 3:1
  meaningful chrome contrast under the existing test policy.
- A terminal-engine replacement cannot ship if it removes an accessible
  representation of output.

### Mobile

**Proposed requirements:**

- Use a dedicated full-height single-column terminal surface, rather than
  relying only on the current 176-pixel viewport override
  ([`web/src/index.css:154-158`](../../web/src/index.css#L154)).
- Collapse secondary chrome and the order composer by default while keeping
  connection state, Find, palette, and return-to-live controls reachable.
- Keep xterm's hidden input available to the virtual keyboard without covering
  the active terminal row.
- Test touch scrollback, alternate-screen wheel/touch behavior where exposed,
  selection handles, copy/paste, orientation changes, safe areas, and no page
  overflow.

### Theme and palette choice

**Proposed:** keep palette choice terminal-local rather than adding it to the
top bar. Retain Auto and the five named palettes, persistence, invalid-value
fallback, and live application without remounting. Move the existing compact
picker into `TerminalToolbar` when that toolbar is introduced, and make it
available in both Terminal and Focus presentations.

## Product Decision Matrix

| Option | One-target fidelity | Multi-target steering | Invents message boundaries | Reuses current transport | Scope | Decision |
| --- | --- | --- | --- | --- | --- | --- |
| Current separate Terminal and snapshot Chat | High in Terminal, low in Chat | Partial | Yes | Partly | Already built | Reject as destination |
| Terminal only for every selection | High | Poor | No | Yes | Small | Reject; loses valuable group steering |
| Shared terminal surface plus Yard event timeline | High | High | No | Yes | Incremental | **Proposed** |
| New typed agent-message protocol first | Potentially high | High | No, if emitted at source | No | Large | Defer; not needed for first value |

## xterm.js Versus ghostty-web

Claims in this section were checked against primary public sources on
2026-08-18. Ghostty-web `main` was inspected at `1858a59`; its latest stable
release remains `0.4.0` from 2025-12-09. Yard uses xterm.js `6.0.0`, released
2025-12-22; the xterm npm beta channel was active through 2026-08-10.

| Criterion | xterm.js 6.0 | ghostty-web 0.4/main | Yard decision |
| --- | --- | --- | --- |
| Architecture | Browser terminal emulator with established DOM/canvas rendering and official addons | Patched Ghostty VT core compiled to roughly 400 KB WASM, with TypeScript canvas, input, selection, and compatibility layers | Ghostty may improve VT/Unicode behavior, but adds a WASM artifact and another renderer/input stack |
| Maturity | Project history dates to 2014; used by VS Code and many browser terminals; stable plus continuously published betas | First npm release 2025-11-13; one stable 0.4 release; later work is on `next`/main | xterm has materially lower product risk |
| Browser support | Explicit latest Chrome, Edge, Firefox, and Safari support | No published browser matrix; 0.4 claims iOS, while Android and Safari/Firefox fixes landed later | Ghostty requires Yard-owned real-browser qualification |
| Accessibility | Screen-reader mode and minimum contrast are public options | Canvas output has no accessible representation according to open issue #187 | Blocking Ghostty regression |
| Yard options | Supports every option Yard currently sets | Current interface omits `screenReaderMode`, `minimumContrastRatio`, `lineHeight`, `letterSpacing`, and `cursorInactiveStyle` | Migration is not import-only |
| Themes | Rich `ITheme`; Yard already applies runtime changes without remount | Initial theme works; runtime mutation is explicitly incomplete in open issue #125 | Would regress the named palette UX |
| Search | Official `@xterm/addon-search` | No documented or exported search API | xterm directly supports the proposed slice |
| Selection/copy/paste | Mature APIs and official clipboard addon; still requires Yard browser tests | Selection, IME, and paste exist, but open/recent issues cover Codex paste and browser behavior | No adoption advantage today |
| Resize | Current Yard `FitAddon` integration is exercised | Ships its own compatible `FitAddon`, but its sizing/rendering must be requalified | Basic fit is feasible, not free |
| WebSocket/PTY fit | Yard already maps `onData`, `write`, and resize to its JSON relay | README demo uses the same basic `onData`/`write` WebSocket pattern | Transport is not a reason to migrate |
| Scroll issue | Current Yard regression proves history stays visible after a new write | Current `terminal.ts` calls `scrollToBottom()` after every write; open issue #127 describes exactly this failure | Ghostty would not fix Yard's issue and could reintroduce it |
| Scrollback | Yard config is row-based and tested | Open issue #140 reports a line/byte unit mismatch; #139 reports viewport corruption | Blocking until fixed and released |
| Packaging/CSP | Existing JavaScript dependency | Requires async `init()`, WASM loading/hosting, asset and CSP handling; open issue #188 covers the current CSP path | Moderate-to-high integration cost |
| License | MIT | ghostty-web and Ghostty are MIT | Both acceptable, subject to bundled notices |

**Proposed engine decision:** keep xterm.js. Introduce an internal engine seam
only if it naturally falls out of `TerminalSessionController`; do not build a
generic abstraction in advance. Reconsider a feature-flagged Ghostty spike
after a published release provides accessible output, preserved
scroll-on-write, correct row-based scrollback, runtime themes, a supported
WASM path/CSP story, search or an equivalent API, and documented real-browser
coverage.

### Primary public sources

- Ghostty-web
  [README](https://github.com/coder/ghostty-web/blob/1858a5947767a3e1c9e98dbf53b2ff87fedb2aab/README.md),
  [interfaces](https://github.com/coder/ghostty-web/blob/1858a5947767a3e1c9e98dbf53b2ff87fedb2aab/lib/interfaces.ts),
  [terminal implementation](https://github.com/coder/ghostty-web/blob/1858a5947767a3e1c9e98dbf53b2ff87fedb2aab/lib/terminal.ts),
  [FitAddon](https://github.com/coder/ghostty-web/blob/1858a5947767a3e1c9e98dbf53b2ff87fedb2aab/lib/addons/fit.ts),
  [changelog](https://github.com/coder/ghostty-web/blob/1858a5947767a3e1c9e98dbf53b2ff87fedb2aab/CHANGELOG.md),
  [v0.4.0](https://github.com/coder/ghostty-web/releases/tag/v0.4.0),
  [npm metadata](https://registry.npmjs.org/ghostty-web), and
  [license](https://github.com/coder/ghostty-web/blob/main/LICENSE).
- Ghostty-web open issues inspected:
  [forced scroll to bottom #127](https://github.com/coder/ghostty-web/issues/127),
  [runtime themes #125](https://github.com/coder/ghostty-web/issues/125),
  [scrollback viewport corruption #139](https://github.com/coder/ghostty-web/issues/139),
  [scrollback units #140](https://github.com/coder/ghostty-web/issues/140),
  [PTY/mouse input gaps #145](https://github.com/coder/ghostty-web/issues/145),
  [Codex paste #148](https://github.com/coder/ghostty-web/issues/148),
  [screen-reader output #187](https://github.com/coder/ghostty-web/issues/187),
  and [WASM/CSP loading #188](https://github.com/coder/ghostty-web/issues/188).
- Ghostty-web browser work:
  [iOS #76](https://github.com/coder/ghostty-web/pull/76),
  [Safari/Firefox copy #94](https://github.com/coder/ghostty-web/pull/94), and
  [Android #110](https://github.com/coder/ghostty-web/pull/110).
- xterm.js
  [6.0 README and browser matrix](https://github.com/xtermjs/xterm.js/blob/6.0.0/README.md),
  [6.0 public API](https://github.com/xtermjs/xterm.js/blob/6.0.0/typings/xterm.d.ts),
  [search addon](https://github.com/xtermjs/xterm.js/tree/6.0.0/addons/addon-search),
  [clipboard addon](https://github.com/xtermjs/xterm.js/tree/6.0.0/addons/addon-clipboard),
  [6.0 release](https://github.com/xtermjs/xterm.js/releases/tag/6.0.0),
  [npm metadata](https://registry.npmjs.org/@xterm%2fxterm), and
  [license](https://github.com/xtermjs/xterm.js/blob/6.0.0/LICENSE).
- Ghostty
  [license](https://github.com/ghostty-org/ghostty/blob/main/LICENSE).

## Proposed Migration Slices

Each slice is independently reviewable and shippable. Later slices do not
justify widening an earlier one.

### Slice 0: Publish and verify the current baseline

- Extract the terminal/chat changes from synthetic `7fff717` into scoped
  commits on the intended integration branch.
- Run the focused Playwright cases against a private port so another checkout
  cannot be reused.
- Verify the exact built revision in the UI or test fixture.

Exit: the named palettes, native xterm scrolling, history preservation, and
one-snapshot chat fix are on a reviewable branch with reproducible evidence.

### Slice 1: Single-target terminal-first routing

- Route every single-target "work/open" action directly to Terminal.
- Add Terminal/Focus presentation control around one persistent
  `TerminalSessionController`.
- Preserve terminal state while changing presentation.
- Keep existing structured Chat available until composer extraction reaches
  parity; do not add new chat behavior.

Exit: one target opens a real terminal by default and presentation changes do
not reconnect or lose scrollback.

### Slice 2: Terminal ergonomics

- Add terminal-local Find with the official xterm search addon.
- Add explicit copy, paste, return-to-live, connection, and palette controls.
- Complete selection, clipboard, resize, accessibility, and mobile acceptance
  coverage.

Exit: the terminal capability contract is covered in supported desktop and
mobile browsers.

### Slice 3: Structured order dock

- Extract `OrderComposer` without changing existing command APIs.
- Dock it to the single-target terminal and make it collapsible in Focus.
- Preserve templates, delivery states, retries, and version conflicts.
- Remove the single-target terminal-snapshot transcript after behavior and
  accessibility parity.

Exit: Terminal and Focus provide both ephemeral PTY input and durable Yard
orders without a fake chat transcript.

### Slice 4: Multi-target steering

- Keep explicit recipient selection and current per-target command semantics.
- Build `DurableEventTimeline` from real Yard records.
- Add a terminal pane/tab deck using the same terminal controller.
- Preserve reader scroll position in the event timeline.
- Extend target kinds only when the corresponding structured dispatch and
  terminal mapping already exist.

Exit: multiple targets have a first-class steering workspace with no
snapshot-derived messages.

### Slice 5: Optional terminal-engine spike

Start only after Ghostty's blockers are resolved in a published release. Put
one engine behind a development-only flag and run the complete capability
suite. Do not migrate production users based on visual preference or VT
benchmarks alone.

## Proposed Acceptance Tests

1. Selecting one worker and choosing Open enters a connected real terminal,
   not Chat.
2. Switching Terminal to Focus preserves the same socket, lease, frame
   sequence, selection, viewport position, search match, and scrollback.
3. Focus renders one continuous ANSI terminal and creates no output bubbles or
   snapshot requests.
4. Manual orders and every current template dispatch through the existing
   structured command API with command ID and expected versions intact.
5. Normal-buffer wheel, trackpad, keyboard, and touch input can move through
   history; new output does not move a reader who is above the bottom.
6. Return to live moves to the bottom and resumes follow behavior.
7. Alternate-screen mouse mode forwards the expected input to the PTY and does
   not fabricate browser scrollback.
8. Selection survives incoming output. Copy returns exactly the selection,
   and a clipboard failure does not clear it.
9. Typed text, IME composition, normal paste, and bracketed paste each reach
   the PTY once.
10. Find locates, advances, reverses, and closes without changing terminal
    bytes or selection unexpectedly.
11. Pane resize, browser resize, mobile rotation, virtual keyboard, composer
    expansion, and presentation changes send only effective dimensions and
    never produce zero rows or columns.
12. A screen reader can identify the target, connection status, terminal
    output, toolbar, search controls, and composer; all actions are keyboard
    reachable with visible focus.
13. At 390x844 and supported iOS/Android viewports, the page has no incoherent
    overlap or page-level overflow; touch scroll, selection, copy/paste, and
    virtual-keyboard input work.
14. Auto and all five named palettes apply live, persist after reload, remain
    independent of the app theme, meet contrast assertions, and survive
    Terminal/Focus changes.
15. Closing the surface sends one release. Ownership loss shows a terminal
    closed state and an explicit reopen action; it does not silently reconnect.
16. Multiple targets open group steering, not the single-target Focus surface.
    One order produces independent durable receipts for only the explicitly
    selected recipients.
17. Refreshing or receiving durable group events preserves event-timeline
    position when the reader is above the bottom.
18. Agent terminal bytes appear only in terminal panes. A test should fail if
    a terminal snapshot or delta is rendered as an attributed timeline
    message.

## Explicit Non-Goals

- A new agent-message protocol or inferred reply model.
- Parsing blank lines, prompts, ANSI, snapshots, byte deltas, or `@mentions`
  to manufacture message structure.
- Durable terminal transcript storage in Yard.
- Changes to Herdr process, pane, terminal identity, or ownership semantics.
- Deep chat threading, reactions, avatars, presence, or social-chat polish.
- A generic terminal-engine framework before a second engine is viable.
- Replacing xterm.js in the initial migration.
- Configurable scrollback/memory policy in the first slice.
- Unrelated top-bar settings architecture.
- Railroad/map performance, token coordination settings, portable worker
  profiles, or other adjacent workstreams.

## Risks And Unresolved Questions

- **Publication risk:** the fixes are in a local synthetic checkpoint, so a
  design implementation can accidentally branch from the wrong behavior.
- **Lease continuity:** preserving one terminal across presentation and app
  navigation may require keeping the workspace mounted. The desired lifetime
  must remain consistent with Herdr's exclusive control and Yard lease
  validation.
- **Multi-terminal cost:** opening one live terminal per selected target can
  consume browser and server resources. The pane deck should open on demand
  and cap concurrent live panes before considering virtualization.
- **Event availability:** the exact durable event types suitable for the group
  timeline need inventory. Missing event types should produce a smaller
  timeline, not a fallback to terminal parsing.
- **Target coverage:** current group chat excludes Yard orchestrator and
  coordination-node targets. Expansion should follow real command and terminal
  capabilities, not UI symmetry alone.
- **Mobile support policy:** Yard needs an explicit supported mobile browser
  matrix before acceptance can be final.
- **Clipboard policy:** browser permission and secure-context constraints need
  product copy and test fixtures.
- **Remote access:** loopback-only WebSocket origin validation means remote
  browser use is outside current behavior; this design does not alter that
  security decision.
- **Closed-session recovery:** current behavior does not reconnect. A Reopen
  action can request a new validated lease, but transparent reconnection
  requires a separate ownership decision.
- **Ghostty timing:** open issues and unreleased fixes can change quickly.
  Re-evaluate against a tagged release, never only `main`, if a spike is later
  approved.

## Next Decision

Approve or reject the proposed product model before implementation:

1. One target defaults to a persistent real terminal with Terminal/Focus
   presentations.
2. Multiple targets get a Yard-event steering timeline plus real terminal
   panes.
3. Structured orders remain a separate durable command path.
4. xterm.js remains the production engine.

If approved, Slice 0 is the next action because implementation work should not
start from an unpublished synthetic checkpoint.

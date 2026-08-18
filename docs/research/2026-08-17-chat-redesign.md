# Chat Redesign Proposal — Yard Agent Chat

Branch investigated: `2.5d-projected-map-impl` @ `15884b1`. Read-only research; no code changed.

## 0. Recommendation in one paragraph

Kill "Chat" as a UI mode for single agents. Yard already has a real, live, unchunked
transcript for a single agent — `TerminalSession.tsx`, a WebSocket-backed xterm.js pane
that's already wired into the app as the "Terminal" tab. The fake chat view
(`AgentChatWorkspace.tsx`) is a second, worse transcript for the same agent, built by
polling terminal output and regex-splitting it into pseudo-bubbles — which is *why* the
chunking bug exists at all. Retire the bubble transcript for 1:1 use, keep the
order-composer (template picker, idempotent command dispatch, delivery receipt), and dock
it to the bottom of the Terminal view. For multiple agents, keep and redesign
`AgentGroupChat.tsx` into an actual iMessage-style thread: colored per-agent avatars,
left/right bubble alignment, append-only messages instead of snapshot-replace, and an
`@mention` composer for directing a message at one agent instead of always broadcasting.

---

## 1. Current architecture (as it exists today, not as it's described in comments)

### 1.1 Single agent: `AgentChatWorkspace.tsx` is not a chat, it's a lossy terminal reader

`AgentChatWorkspace` handles exactly one target at a time (`AgentChatTarget` — assignment,
project orchestrator, yard orchestrator/"Superintendent", or coordination node —
`web/src/AgentChatWorkspace.tsx:60-64`). There is no persisted conversation on the
backend. What happens on open/refresh (`loadActivity`, `AgentChatWorkspace.tsx:305-393`):

1. It calls one of `fetchAssignmentTerminalOutput` / `fetchOrchestratorTerminalOutput` /
   `fetchYardOrchestratorTerminalOutput` / `fetchCoordinationNodeTerminalOutput`
   (`web/src/api.ts:340,687,715,767`), which return a `TerminalOutput`-shaped object:
   `{ text, revision, truncated, ... }` (`web/src/types.ts:954-984`) — i.e. a flat dump of
   recent terminal bytes, not discrete messages.
2. That raw text is trimmed to the last 8000 chars (`activityExcerpt`,
   `AgentChatWorkspace.tsx:125-129`), then heuristically carved into "messages"
   (`activitySegments`, `:152-186`) by blank-line/code-fence boundaries, and any resulting
   block over 4000 chars is further hard-split by `chunkActivitySegment` (`:131-150`) —
   this is the literal chunking bug someone else is fixing, but it's a symptom of treating
   a terminal buffer as a set of chat turns.
3. Every refresh **replaces** all `id.startsWith('activity:')` messages with a freshly
   resegmented batch (`:352-357`). There is no stable per-message identity across
   refreshes — `id` is synthesized as `activity:${targetKey}:${revision}:${index}`, so a
   message's bubble boundaries can shift between polls.
4. Sending a prompt (`sendPrompt`, `:440-643`) does **not** create a "reply" — it builds a
   structured, idempotent command (`command_id: crypto.randomUUID()`,
   `expected_*_version` optimistic-concurrency fields — see `SendAssignmentPromptInput`
   etc., `web/src/types.ts:591-614`), POSTs it via `sendAssignmentPrompt` /
   `sendOrchestratorPrompt` / `sendYardOrchestratorPrompt` / `sendYardOrchestratorRoute` /
   `sendCoordinationNodePrompt` / `sendCoordinationNodeRoute`, and locally synthesizes a
   `kind: 'user'` bubble plus a `kind: 'system'` "Delivered"/"Routed" receipt bubble. The
   agent's actual reply is never captured as a reply — it just shows up later as
   undifferentiated terminal excerpt on the next poll, arbitrarily chunked.

So today's "chat" is: a REST command-dispatch form, bolted onto a lossy, re-segmented
tail of a terminal buffer that's dressed up with chat-message CSS
(`.chat-message`, `App.css:3132-3189`, uniform card styling, no left/right distinction, no
avatars — `data-kind` only toggles color, per `App.css:3141-3186`).

**Critically: a real, correct, unchunked transcript already exists for the same target.**
`web/src/TerminalSession.tsx` opens a WebSocket (`assignmentTerminalWebSocketUrl` /
`orchestratorTerminalWebSocketUrl` / `yardOrchestratorTerminalWebSocketUrl` /
`coordinationNodeTerminalWebSocketUrl`, `web/src/api.ts:788-849`) that streams live
`terminal.frame` byte deltas into an `xterm.js` `Terminal` instance
(`TerminalSession.tsx:242-291`), and pipes typed keystrokes straight back over the socket
as `terminal.input` (`:187-191`). This is already mounted as the "Terminal" tab
(`App.tsx:4161-4170`, `SquareTerminal` icon) alongside "Map" and "Chat"
(`App.tsx:4142-4160`) in a per-agent tri-mode switcher
(`agentWorkspaceMode`, wired at `App.tsx:2767-2777`). Both `AgentChatWorkspace` and
`TerminalSession` are shown for the *same* `activeAgentWorkspaceTarget` — they are two
transcripts of the same agent, one real-time and correct, one polled and lossy.

This means the "chat" REST path (`sendAssignmentPrompt` et al.) is not redundant with
terminal keystrokes — it's a deliberately different, safer channel: idempotent, versioned,
retryable without double-delivery, and prefillable from `ORDER_TEMPLATES`
(`web/src/orderTemplates.ts`, referenced at `AgentChatWorkspace.tsx:34-36,779-798`). That
composer is worth keeping. Its *transcript rendering* is not.

### 1.2 Multiple agents: `AgentGroupChat.tsx` is close to the right shape already

`AgentGroupChat` takes a list of `AgentGroupTarget` (assignment or orchestrator only — it
does not support yard-orchestrator or coordination-node targets, unlike the single-agent
component) and is structurally closer to "group chat" than `AgentChatWorkspace` is:

- One composer submit fans out to every selected target as independent structured
  commands (`commandForTarget` + `dispatch`, `:108-137,270-323`).
- Each recipient gets **independent per-target delivery state**
  (`TargetDelivery: 'pending' | 'delivered' | 'failed'`, `:72-76`, rendered per-agent at
  `:504-561`) with per-target retry, split into "retryable" vs "needs a fresh command"
  buckets (`:338-365`) — this is functionally identical to iMessage's per-recipient
  delivery receipts, just styled as a status list instead of under-bubble captions.
- The roster is rendered as labeled chips with status (`.group-chat-roster`,
  `:393-399,470-483`).

But the transcript is still the same flaw as §1.1: `loadSnapshots` (`:192-237`) polls each
target's terminal output independently, truncates to 2200 chars
(`outputExcerpt`, `:98-102`, no chunking here, but only because it's a harder cutoff, not
because the model is sound), and **replaces** the whole `snapshot:`-prefixed message set
every refresh (`:230-236`). Messages aren't attributed per-turn (i.e. "this reply is what
Agent B said in response to the order sent at 3:04") — they're just "here is a fresh
excerpt of Agent B's pane," relabeled each poll. There's no color/avatar per agent, no
bubble alignment — every message, including the user's own broadcast, renders as the same
`.chat-message` card with a `data-kind` color swap (`App.css:3132-3189`), distinguished
only by the text label in the header.

### 1.3 Shared conclusion

Single-agent and multi-agent chat are **not** one generalized component stretched thin —
they're two separately-implemented, near-duplicate versions of the same flawed pattern
(poll terminal output → resegment into fake messages → merge with locally-synthesized
user/system messages). That duplication is itself a problem (the chunking bug, e.g., only
needed fixing in one of the two files, but the same class of bug is latent in the other's
`outputExcerpt`). Neither file currently produces a real "message" as a discrete unit the
agent actually emitted — both fabricate message boundaries from a rolling text buffer.

---

## 2. What bb (get-bb/bb) does — specific findings worth adapting

Fetched `README.md`, `docs/system-overview.md`, and `docs/VISION.md` from
`github.com/get-bb/bb`. Three findings map directly onto this problem:

1. **"Thread: the unit of work... produces an append-only stream of events (messages,
   tool calls, file changes, etc.)"** (`docs/system-overview.md`, Data model). bb doesn't
   store "terminal text" and re-derive messages from it — it stores typed, append-only
   events from the start, where a chat message is one event type among several (tool
   calls and file changes are separate event types, not interleaved text a client has to
   parse back apart). This is the direct fix for Yard's root problem: **a message needs to
   be a real unit at write time, not inferred at read time.** Adapt this as: whatever
   emits agent output should tag "this is a discrete reply" as its own event/frame, not
   leave the client to guess message boundaries from blank lines and code fences.

2. **Threads are "standard (does work directly) or manager (coordinates other
   threads)... own child threads for delegation."** This maps directly onto Yard's
   existing worker/orchestrator/yard-orchestrator/coordination-node hierarchy — bb
   validates that a coordinator-style entity (Yard's "Superintendent"/coordination node)
   is a normal, first-class thread type, not a special case requiring its own chat
   component. Worth noting since it argues *against* Yard's current split where
   `AgentGroupChat` silently drops yard-orchestrator/coordination-node support (§1.2) —
   bb's model treats delegation as uniform, so Yard's redesign should too.

3. Screenshot alt text and layout description: **"a code review thread, dispatch panel,
   and task board"** — bb keeps the conversation transcript ("thread") and the
   instruction-sending surface ("dispatch panel") as visually distinct, separately-scoped
   panels rather than one interleaved feed. This directly supports this proposal's
   single-agent recommendation (§3.1): docked composer, separate from — not mixed into —
   the transcript.

The README also states bb sends only lifecycle telemetry counts, never message content,
and is explicit that local/self-hosted trust is a product principle ("Easy to trust and
adopt... especially for teams with security and trust constraints" — `docs/VISION.md`).
Not chat-UX per se, but consonant with the "polish and trust" bar the owner named — worth
keeping in mind if Yard ever adds any telemetry around chat usage.

---

## 3. The proposal

### 3.1 Single agent: fold the composer into the Terminal view; delete the Chat tab

**Decision: fold prompt/response into the terminal view directly.** Not a hybrid, not "a
lightweight command bar next to a separate result pane" — the result pane already exists
and is already correct (`TerminalSession`). Building a second result-display surface next
to it would recreate exactly the duplication problem in §1.3 with different styling.

Concretely:

- Remove the "Chat" tab from the mode switcher (`App.tsx:4151-4160`). The switcher becomes
  Map / Terminal (two tabs, not three).
- `TerminalSession` gains a persistent, collapsible **order composer** docked to the
  bottom of `.terminal-session` — reuse, not rebuild, the existing composer JSX/logic from
  `AgentChatWorkspace.tsx:857-937` (textarea, `⌘/Ctrl+Enter` submit at
  `handleComposerKeyDown`, `ORDER_TEMPLATES` dropdown, `sendPrompt`/retry-with-new-command
  logic, `PromptFeedback` states). None of that logic is chat-transcript logic — it's
  pure command-dispatch, and it's the part of `AgentChatWorkspace` worth keeping.
- Delivery feedback (`chat-delivery-feedback`, `:898-936`) renders as a small dismissible
  banner anchored above the composer, not as a chat bubble in a transcript. Because there's
  no fake transcript to append a "Delivered" system-message to, this becomes simpler, not
  more complex, than today.
- The terminal itself remains the one and only transcript: real bytes, real order, no
  resegmentation, no 4000-character hard-wraps, correct scrollback (`xterm.js` already
  handles this).
- `AgentChatWorkspace.tsx` is deleted once its composer logic has been extracted into the
  new docked component. `activityExcerpt` / `activitySegments` / `chunkActivitySegment`
  and `fetchAssignmentTerminalOutput`-style polling for chat purposes go away entirely for
  the single-agent path (the terminal WebSocket replaces polling as the read path).

Why not the other two options: a separate "lightweight command-bar + inline result
display" duplicates the terminal's job with an inevitably worse renderer (no ANSI, no
ordering guarantees, yet another resegmentation problem to solve). "Keep something
chat-shaped but 1:1 and minimal" is just today's component with fewer features — it still
has to solve message-boundary inference from raw output, which is the actual bug class,
not a symptom of too many features.

### 3.2 Multiple agents: iMessage-style group thread, kept as its own component

Keep `AgentGroupChat`, rebuild its transcript and identity model. Concrete UI spec:

**Per-agent identity.** Each `AgentGroupTarget` gets a deterministic color derived from a
hash of its `targetKey()` (stable across sessions — same agent, same color, every time)
plus a 1-2 letter initials avatar built from `target.label`. Render this avatar to the
left of every bubble from that agent, and reuse the same color as a left border accent on
the bubble and as the color of the agent's chip in the existing roster row
(`.group-chat-roster`, already present at `AgentGroupChat.tsx:393-399`) — the roster chip
becomes the color legend.

**Bubble alignment ("my instruction" vs "agent N's reply").** The user's own broadcast
messages render right-aligned in the accent/brand color (reuse the existing
`.command-button`/accent token), full width capped like iMessage's own-message bubble.
Every agent reply renders left-aligned, tagged with that agent's avatar+color+name header,
grouped by consecutive-same-sender the way iMessage collapses repeated avatars — i.e., if
Agent B posts two updates in a row, only the first shows the avatar/name header; the
second is visually grouped under it with tighter spacing. This one change (avatar +
alignment + grouping) is the single highest-leverage visual fix versus today's uniform
`.chat-message` card list where every kind looks like the same shape with only a color
swap (`App.css:3132-3189`).

**Delivery receipts.** Already exists functionally (`TargetDelivery`,
`AgentGroupChat.tsx:72-76`) — redesign only the presentation: instead of a separate status
list block under the composer (`:504-561`), show delivery state as a small caption line
under the user's own most recent broadcast bubble, iMessage-style: "Delivered to 3 ·
Sending to 1 · Failed: Agent D" with the existing per-agent retry buttons
(`retryFailed(false|true)`, `:338-357`) surfaced inline on tap/click of the "Failed"
segment, not as a permanently-visible block.

**Directing at one agent vs. broadcasting.** Add `@` mention support in the composer:
typing `@` opens a roster picker (reuse `stableTargets`, `:159-165`); selecting one or more
agents narrows `dispatch()`'s `commands` array (`:270-323`) to just the mentioned subset
instead of `stableTargets.map(...)` (`:330-334`) — this is a filter on an array that
already exists, not new dispatch plumbing. No `@mention` in the message = broadcast to
everyone in the roster, exactly as today. Visually, an @-mentioned message still posts
once in the shared thread (not duplicated per recipient) but shows a small "to Agent B"
tag instead of "to 4 recipients," matching how iMessage doesn't have a concept of
single-recipient targeting inside a group thread but Yard's use case genuinely needs one
(steering a single worker without derailing the others) — so this is Yard's own addition
on top of the iMessage pattern, not a literal copy.

**Threading.** Skip deep/nested threading for v1 — flat chronological like iMessage group
chat is enough once sender attribution (avatar/color/alignment) is fixed; nested threads
would be solving a problem (losing track of which reply answers which order) that
per-sender attribution already solves for the group sizes Yard deals with (a handful of
agents, not dozens).

**Coverage gap to close while rebuilding:** extend `AgentGroupTarget` to include
yard-orchestrator and coordination-node kinds (today only `assignment`/`orchestrator` are
supported, `AgentGroupChat.tsx:45-58`), matching what `AgentChatWorkspace` already supports
for 1:1. Per finding §2.2 (bb treats delegation/coordinator threads uniformly), there's no
principled reason group chat should support fewer target kinds than 1:1 does.

### 3.3 Architecture: survives vs. rebuilt, migration sequence

**Survives as-is or with minor edits:**
- `TerminalSession.tsx` and its WebSocket URL builders (`api.ts:788-849`) — unchanged,
  becomes the single-agent transcript by default instead of an alternate tab.
- Command construction/dispatch logic: `sendAssignmentPrompt`, `sendOrchestratorPrompt`,
  `sendYardOrchestratorPrompt`, `sendYardOrchestratorRoute`, `sendCoordinationNodePrompt`,
  `sendCoordinationNodeRoute` (`api.ts`) and their `Send*Input` types (`types.ts:591-735`)
  — the command/idempotency/versioning model is sound and target-agnostic; reuse verbatim
  for both the docked single-agent composer and the group composer.
- `ORDER_TEMPLATES` / `buildExecutionOrder` (`orderTemplates.ts`) and
  `promptRequiresNewCommand` (`promptPolicy.ts`) — unchanged.
- `AgentGroupChat`'s per-target delivery state machine (`TargetDelivery`, `dispatch`,
  `retryFailed`) — logic survives, only presentation and target-kind coverage change.

**Rebuilt:**
- All of `AgentChatWorkspace.tsx`'s transcript machinery: `activityExcerpt`,
  `activitySegments`, `chunkActivitySegment`, `loadActivity`, the `ChatMessage` type, and
  the polling `useEffect`s built around it. Deleted, not repaired — the terminal WebSocket
  replaces this read path entirely.
- `AgentGroupChat`'s `ThreadMessage`/`loadSnapshots` snapshot-replace model
  (`:78-84,192-237`) → replaced with an **append-only** model: track last-seen
  `revision`/byte-offset per target, and on each poll (or, better, once available, a
  WebSocket push per target analogous to `TerminalSession`'s) append only the *new* delta
  as one attributed message instead of re-fetching and re-rendering a fixed-size excerpt.
  This is what makes "per-agent reply to a specific order" attribution actually meaningful
  instead of "whatever's in the last 2200 characters right now."
- CSS: new bubble/avatar/alignment styles for group chat (`.chat-message` family,
  `App.css:3132-3189`, `.group-chat-message`, `App.css:1739-1748`) — net new visual
  language, not a retheme of the existing card list.

**Sequencing (v1 stopping point marked):**
1. Extract the composer (textarea, templates, submit, delivery-feedback UI) out of
   `AgentChatWorkspace.tsx` into a standalone `OrderComposer` component with no transcript
   dependency.
2. Dock `OrderComposer` to `TerminalSession`; remove the "Chat" tab; delete
   `AgentChatWorkspace.tsx`. **— natural v1 stopping point for the single-agent half; this
   alone eliminates the chunking bug class permanently for 1:1, independent of whatever
   narrow fix lands separately.**
3. Rebuild `AgentGroupChat`'s transcript: append-only per-target message log, avatar/color
   identity, bubble alignment + grouping, inline delivery captions.
4. Add `@mention` targeting inside group chat, narrowing `dispatch()`'s recipient list.
5. Extend `AgentGroupTarget` to cover yard-orchestrator/coordination-node kinds, closing
   the coverage gap from §3.2.
6. (Stretch, not required for v1) Replace group-chat polling with a per-target WebSocket
   push, mirroring `TerminalSession`'s frame protocol, so append-only messages arrive in
   real time instead of on a refresh click.

### 3.4 The "Apple-level polish and trust" bar, made concrete

Not "make it feel premium" — specific, checkable tenets this redesign should be held to:

- **Motion is a state transition, not a wallpaper.** A new bubble in group chat should
  animate in with a short (~150ms), single easing curve slide/fade — same curve used for
  the delivery-caption update, so a message "arriving" and its receipt "confirming" read
  as one coherent event, not two unrelated micro-animations. No motion on refresh of
  content that hasn't changed (today's full snapshot-replace, §1.2, would re-animate
  everything on every poll — the append-only rebuild fixes this as a side effect).
- **Spacing encodes hierarchy, not just padding.** Grouped consecutive messages from the
  same agent get tight inter-message spacing (avatar/name suppressed); a change of sender
  gets a full spacing break plus the avatar/name reappearing — spacing itself is the
  signal for "new speaker," the way iMessage and Apple's Messages/Mail both use whitespace
  instead of dividers.
- **Typography: one voice for the system, one for people, one for agents.** System/receipt
  text (delivery captions, "Agent B joined the roster") stays small-caps-adjacent
  secondary weight and never competes visually with actual message content — today
  `chat-message[data-kind="system"]` and `[data-kind="error"]` are styled but only by
  color (`App.css:3147-3156`), not by weight/size, so a delivery receipt currently reads
  with the same visual weight as a real reply. Fix that distinction explicitly.
- **Empty states are written, not defaulted.** Today's fallback is the string `'No recent
  agent output.'` (`AgentChatWorkspace.tsx:341`) shown as a full chat bubble — a real empty
  state for a fresh group thread should say what to do next ("Send the first order to
  start this thread"), not describe an absence.
- **Error states name the failure and the fix in the same breath.** The existing
  `requiresNewCommand` branch (`:915-931`) already models this correctly — "this command ID
  cannot be retried safely, send as new command" is a good pattern (explains why, offers
  the one correct action) and should be the template for every other error surface in the
  redesign, not just prompt delivery.
- **Never block on data that's merely stale.** The terminal WebSocket already reconnects
  and shows connection state without freezing input (`ConnectionState`,
  `TerminalSession.tsx:23-27`) — the group composer should adopt the same rule: a slow or
  failed poll for one agent's messages never disables sending to the others.

---

## Key files referenced

- `/home/arv/repos/yard/web/src/AgentChatWorkspace.tsx`
- `/home/arv/repos/yard/web/src/AgentGroupChat.tsx`
- `/home/arv/repos/yard/web/src/TerminalSession.tsx`
- `/home/arv/repos/yard/web/src/api.ts`
- `/home/arv/repos/yard/web/src/types.ts`
- `/home/arv/repos/yard/web/src/orderTemplates.ts`
- `/home/arv/repos/yard/web/src/promptPolicy.ts`
- `/home/arv/repos/yard/web/src/App.tsx` (mode switcher: lines ~4142-4171; target wiring: ~2740-2784)
- `/home/arv/repos/yard/web/src/App.css` (`.chat-*`, `.group-chat-*` rules)
